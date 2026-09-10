# Architecture

[← Docs index](README.md) · [Getting started](getting-started.md) ·
[Document format](document-format.md) · [Bridge](bridge.md) · [API](api-reference.md) ·
[Security](security.md) · **Architecture** · [Building](building.md) ·
[Contributing](contributing.md) · [AGENT.md](AGENT.md)

---

## The problem being solved

Windows HTAs let anyone build a real desktop tool out of one HTML file. No build step, no bundler,
no `node_modules`. You wrote a file, double-clicked it, and had an application with filesystem
access. That workflow died with MSHTML and nothing replaced it.

What exists instead:

- **Electron and Tauri** solve a different problem. They are frameworks for shipping products, with
  scaffolding, a build pipeline, and a bundling step. Overkill for a 200-line internal tool.
- **A browser tab** has none of the capabilities: no filesystem, no subprocesses, no native
  windowing, no tray, no system integration.
- **Native toolkits** require a compile cycle for every change and a language most tool authors do
  not want to write.

On Linux there is a second gap: there is no good way to write a Wayland panel, a desktop widget, or
a launcher using web technology. Those are exactly the small, personal, disposable tools people most
want to build quickly.

HTML App is the missing runtime — plus a capability model that makes it safe, which is the part the
original HTA got catastrophically wrong.

---

## Crate layout

```
htmlapp            CLI, argument parsing, mode dispatch
├── htmlapp-runtime      window lifecycle, consent flow, orchestration
│   ├── htmlapp-shell        GPUI chrome: launcher, consent sheet, permissions manager
│   ├── htmlapp-views        native views placed inline in HTML layout
│   ├── htmlapp-wayland      layer-shell translation, output enumeration
│   ├── htmlapp-engine       the WebEngine trait and its backends
│   │   └── htmlapp-engine-cef   optional Chromium backend (scaffolded)
│   └── htmlapp-api          the native API modules            ← mechanism
│       ├── htmlapp-bridge       JSON-RPC, the JS shim, TypeScript emit
│       └── htmlapp-caps         manifest, permissions, consent ← policy
└── htmlapp-build       stapling, .desktop, MIME, AppImage, Flatpak
```

The split that matters most is **`htmlapp-caps` versus `htmlapp-api`**:

- `htmlapp-caps` decides what a document *may* do. It parses manifests, compiles globs, stores
  consent, computes permission diffs. It has no idea what a filesystem is.
- `htmlapp-api` decides whether *this call* is within that. Every module holds an `ApiContext` and
  every path, program, origin, database, and bus name goes through it.

Neither touches an engine or a window. That is what lets the entire security model be tested — and
audited — without starting a browser, which is worth more than it sounds: 26 policy tests and 17
enforcement tests run in under a second with no display server.

---

## Rendering and compositing

This is the part with real trade-offs, so it is worth setting out properly.

### The ideal

The page is rendered **offscreen** and composited **into** GPUI's scene as a texture, rather than
being a native surface layered on top. That single choice buys four things:

1. Native UI paints *over* the page — modals, popovers, native views.
2. The parent's `rounded()`, `overflow_hidden()`, opacity, and transforms all apply to it.
3. It works on Wayland, where there is no cross-toplevel reparenting.
4. No hide-the-webview dance when a dialog opens.

Getting there needs an engine that renders offscreen. WPE WebKit is the right one — it is WebKit's
embedded port, has no GTK dependency, and hands rendered frames to the embedder as buffers.

### What ships today

**`wry` (WebKitGTK) as a native child surface of the GPUI window.** WPE is not packaged on most
distributions, which makes it a hard build dependency for anyone who wants to compile this.

A native child surface inverts all four properties above. So the runtime buys three of them back
with a different mechanism.

### X11 SHAPE: getting compositing back

`wry` puts the page in an X11 child window of the host window. X11's SHAPE extension can subtract
rectangles from a window's **bounding** region (what is drawn) and its **input** region (what is
clickable). Subtract a rectangle from both, and in that area the *parent* shows through and receives
the input.

The parent is the GPUI window. So:

```
page reports a rect over the bridge  →  host subtracts it from the page's window
                                    →  GPUI paints there instead
                                    →  input in that region goes to GPUI
```

That is how a native view appears *inside* HTML layout, and how a modal paints *over* the page —
without hiding the webview, and without a `mount_webview` flag.

Run `htmlapp examples/native-view.hta` to see it: a GPUI table with 100,000 rows rendering inside an
HTML border, with the heading and status line above and below it as ordinary DOM.

**What it does not give you.** SHAPE regions are sets of rectangles. There is no per-pixel alpha and
no anti-aliasing, so a rounded corner is a staircase unless you approximate it with enough
rectangles, and a drop shadow that fades over the page cannot be expressed at all. Those need the
offscreen path. What SHAPE does give is the load-bearing part: **native UI over the page, with
input.**

**The Wayland cost.** `wry`'s `build_as_child` is X11-only on Linux — its own documentation says so.
So document windows run on X11 or XWayland. The runtime handles this by clearing `WAYLAND_DISPLAY`
*and* setting `GDK_BACKEND=x11` before any window exists. Both are needed: clearing the variable
alone is not enough, because `wl_display_connect(NULL)` falls back to `$XDG_RUNTIME_DIR/wayland-0`,
so GDK would still open a Wayland display and the webview would fail to attach.

The launcher, the consent sheet, and the permissions manager are pure GPUI with no webview, so they
run Wayland-native.

**Layer-shell mode is Wayland-native too**, by a different route: it skips GPUI entirely and puts
the page in a GTK window with `gtk-layer-shell` applied. A bar has no titlebar, no consent sheet,
and no native views — the page *is* the whole UI, so there is nothing to composite. See
[Document format → Layer mode](document-format.md#layer-mode).

### One workaround worth deleting

`gpui` 0.2.2 does not implement `HasWindowHandle` for its X11 window:

```rust
impl rwh::HasWindowHandle for X11Window {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        unimplemented!()
    }
}
```

Calling it panics, and the window id it wraps is `pub(crate)`.
[`crates/htmlapp-engine/src/x11_window.rs`](../crates/htmlapp-engine/src/x11_window.rs) therefore
recovers the id from the X server via `_NET_WM_PID`, retried across frames so it never blocks the
window from mapping. It works and is bounded, but it is a workaround for a gap in one dependency
version. Implementing that trait upstream deletes the file.

### The CEF backend

[`htmlapp-engine-cef`](../crates/htmlapp-engine-cef) is a real crate that implements nothing. It
exists so the seam is real rather than hypothetical, for the case where a document genuinely needs
Chromium — WebGPU, WebCodecs, newer CSS.

Implementing it means a multi-process bootstrap (CEF re-execs the host binary as renderer and GPU
subprocesses, dispatching on `--type=` before `main` does any work, which interacts with stapled
binary detection), offscreen rendering via `OnPaint`, and 150–250 MB shipped. Notably it would be
the *first* backend to satisfy the compositing goal properly, rather than through SHAPE.

Nothing above `WebEngine` changes. That is the entire reason the trait exists.

---

## Process model

```
htmlapp (one process per document)
├── GPUI main thread — window, event loop, scene, native views, GTK pump
├── Tokio runtime — bridge dispatch, fs, process, net, sql
├── WebKitGTK UIProcess (in-process, GLib main context)
│   └── WebProcess (separate OS process, sandboxed by WebKit)
│       └── NetworkProcess (separate OS process)
└── optional: bubblewrap jail wrapping the whole tree (--sandbox)
```

GPUI owns the main thread and runs its own event loop. WebKit needs its GLib context pumped to
paint, run timers, and see input — GPUI knows nothing about GLib, so the frame loop drains it once
per frame, bounded at 64 iterations so a page generating events faster than they can be processed
cannot starve the host's own frame.

Keeping that loop alive needs care: `Window::request_animation_frame` resolves the current view
through `Window::current_view`, which asserts it is inside a layout or paint phase. The tick runs
from an `on_next_frame` callback, which is outside all of them, so the loop is driven with
`cx.notify()` instead. *(This was a panic, found by running the thing.)*

Bridge calls arrive on Tokio workers but `evaluate_script` must run on the engine's thread, so
outbound messages queue in a `QueueTransport` and the frame loop drains them.

---

## The consent flow

Consent and the document window share **one** GPUI application, because the engine cannot be
constructed until the answer is known — a document must not run a single line of script before its
permissions are settled.

```
load document → read manifest → decide against the consent store
                                     │
        ┌────────────────────────────┼────────────────────────────┐
   NotRequired                  NeedsPrompt                 AlreadyGranted
        │                            │                            │
        │                     consent sheet                       │
        │                    ╱      │      ╲                      │
        │                Allow  Powerless  Cancel                 │
        │                  │        │        │                    │
        └──────────────────┴────────┘      quit ──────────────────┘
                           │
              register modules → start engine → open window
```

"Open without permissions" is deliberately *not* recorded. Running once without permissions is not
the same as refusing forever, so the next run asks again.

---

## Data flow of one call

```
page:   await htmlapp.fs.read({ path: '~/notes.md' })
   │
   ├─ shim assigns id, posts {"t":"invoke","id":7,"method":"fs.read",…}
   │      over the engine's single IPC channel
   │
   ├─ engine callback → Tokio: dispatcher.handle_raw(raw)
   │
   ├─ Dispatcher::resolve — is `fs` in the manifest's granted set?      ← gate 1
   │      no  → RpcError::denied, no handler reached
   │
   ├─ FsModule::read → ApiContext::check_read(path)                    ← gate 2
   │      canonicalise → match against compiled globs → return the resolved path
   │
   ├─ tokio::fs::read(resolved)      ← the resolved path, not the requested one
   │
   ├─ Ok(value) → QueueTransport
   │
   └─ frame loop → engine.evaluate("window.__htmlapp_dispatch(\"…\")")
          → shim resolves the promise for id 7
```

---

## Where the interesting code is

| Question | File |
|---|---|
| What may a document do? | [`htmlapp-caps/src/permissions.rs`](../crates/htmlapp-caps/src/permissions.rs) |
| Is *this call* within that? | [`htmlapp-api/src/context.rs`](../crates/htmlapp-api/src/context.rs) |
| What is the API surface? | [`htmlapp-bridge/src/catalog.rs`](../crates/htmlapp-bridge/src/catalog.rs) |
| How does a call get routed? | [`htmlapp-bridge/src/dispatch.rs`](../crates/htmlapp-bridge/src/dispatch.rs) |
| What does the page see? | [`htmlapp-bridge/js/shim.js`](../crates/htmlapp-bridge/js/shim.js) |
| How is the engine abstracted? | [`htmlapp-engine/src/lib.rs`](../crates/htmlapp-engine/src/lib.rs) |
| How does compositing work? | [`htmlapp-engine/src/occlusion.rs`](../crates/htmlapp-engine/src/occlusion.rs) |
| How does a document start? | [`htmlapp-runtime/src/session.rs`](../crates/htmlapp-runtime/src/session.rs) |
| How does the window run? | [`htmlapp-runtime/src/app.rs`](../crates/htmlapp-runtime/src/app.rs) |

Every one of those files opens with a comment explaining *why* it is shaped the way it is. That is
where the reasoning lives; this page is the map.

---

## Non-goals

- **Not a framework.** No component library, no reactivity, no router, no scaffolding command. HTML
  App has no opinion about what is inside the HTML.
- **Not a browser.** No tabs, no address bar, no history UI, no arbitrary browsing.
- **Not an Electron replacement.** Large multi-page applications with a build pipeline are better
  served by Tauri. This optimises for the single-file case and accepts the trade-offs.
- **Not cross-platform yet.** macOS needs WKWebView + CALayer interop or the CEF backend; Windows
  needs WebView2 visual hosting into a DirectComposition tree. Both are genuinely reachable — the
  `WebEngine` trait and everything above it are written platform-agnostically — but neither is done.
- **No hidden network access.** The runtime never phones home, never auto-updates, never fetches a
  remote runtime. The only network access it performs on its own behalf is fetching a pinned import
  that is not already cached, and that is refused unless the hash matches.

---

[← Security model](security.md) · **Architecture** · [Building and packaging →](building.md)
