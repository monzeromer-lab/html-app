# Status against the PRD

What is implemented, what is partial, and what is blocked — mapped onto
[`htmlapp-prd.md`](htmlapp-prd.md). Written to be read by someone deciding what to pick up next, so
it is specific about *why* things are where they are rather than just listing them.

Legend: **✅ done** · **◐ partial** · **⛔ blocked** · **○ not started**

---

## 1. The one thing to understand first

The PRD's central architectural commitment (§6.1) is that **the page is rendered offscreen and
composited *into* GPUI's scene**, rather than being a native surface layered on top. Everything in
§4.2 G2 — modals over the page, clipping, rounded corners, opacity, transforms — follows from that,
and so does Wayland support.

**The backend that ships here does the opposite.** It embeds `wry` (WebKitGTK) as a native child
surface of the GPUI window. That was a deliberate, requested choice, and it works — but it inherits
exactly the four consequences §6.1 lists:

| | Offscreen (§6.3, target) | Native child (today) |
|---|---|---|
| Native UI paints over the page | Yes | **No** |
| Parent `rounded()` / `overflow_hidden()` clips it | Yes | **No** |
| Wayland | Yes | **X11 / XWayland only** |
| Modals need the page hidden | No | **Yes** |

Everything above the engine is written against the `WebEngine` trait and does not know which backend
is in use, so §6.3's Stage 1 (WPE + Shm) is an addition to `htmlapp-engine`, not a rewrite anywhere
else. That is the single highest-value piece of remaining work.

---

## 2. Milestones (§14)

| | Milestone | State | Notes |
|---|---|---|---|
| **M0** | Spike | ◐ | Frames reach the screen and input works — via a native child surface rather than the Shm upload §6.3 specifies. Input, focus, and IME are WebKit's own, so §16 R7 is untested rather than solved. |
| **M1** | Runtime | ✅ | Manifest parsing, `htmlapp://` scheme, CSP, `window` + `headless` modes, native titlebar, launcher v1, `.desktop` + MIME registration, bundled examples. |
| **M2** | Bridge | ✅ | JSON-RPC, streams, events, `fs`, `process` (incl. PTY), `dialog` via portal, `store`, TypeScript emit, `blob:` URLs minted and served. |
| **M3** | Capabilities | ◐ | Permission model, consent sheet, hash pinning, `htmlapp permissions` — all done. `--sandbox` (bubblewrap) is not. |
| **M4** | Linux native | ◐ | `dbus`, `notify`, `clipboard`, `os`, `shell` work. `tray`, `secrets`, `systemd`, `udev`, `portal` beyond FileChooser, `shortcut` are not implemented. Layer-shell is ⛔ — see below. |
| **M5** | Native views | ◐ | `terminal` (real `alacritty_terminal`) and `table` (virtualized) are implemented and tested, and the `<htmlapp-view>` element plumbing is complete. They cannot be *seen*, because a native child surface paints over them. |
| **M6** | Distribution | ✅ | `htmlapp build`, stapling, per-app `.desktop` and icon, AppDir, Flatpak manifest. `.AppImage` needs `appimagetool` on PATH and says so when it is missing. |
| **M7** | Zero-copy | ○ | Requires M0/§6.3 Stage 1 first. |
| **M8** | Reach | ◐ | `sql` (bundled SQLite) is done. `net` sockets, `serial`/`usb`/`bluetooth`, `plugin` (wasmtime), `ffi`, `editor` and `video` views are not. |

---

## 3. The API catalog (§9.3)

Every module below is declared in the catalog, appears in the emitted TypeScript, and is gated on a
manifest permission. "Implemented" means the native side actually does the work.

### Tier 1 — the core

| Module | State | Notes |
|---|---|---|
| `fs` | ✅ | `read`, `write`, `append`, `stat`, `list`, `glob`, `mkdir`, `remove`, `rename`, `copy`, `readStream`, `tail`, `watch` (inotify). `mmap` not implemented. |
| `process` | ✅ | `exec`, `spawn` (streamed), `pty` (`portable-pty`), `kill`, `signal`, `list`, `write`, `resize`. |
| `dialog` | ◐ | `open`, `save`, `pickFolder` via `xdg-desktop-portal`. `message`/`confirm`/`prompt` are queued to the shell but do not return a result yet. |
| `http` | ✅ | Origin allow-list enforced per request **and per redirect hop**. |
| `sql` | ✅ | Bundled SQLite, prepared statements, transactions, streamed rows. DuckDB not wired. |
| `store` | ✅ | Scoped to the app id, written atomically. |

### Tier 2 — Linux desktop integration

| Module | State | Notes |
|---|---|---|
| `portal` | ◐ | FileChooser only. Screenshot, ScreenCast, GlobalShortcuts, Inhibit, Background, Wallpaper, Location not implemented. |
| `dbus` | ◐ | `call`, `get`, `introspect`, `subscribe`. `set` and `own_name` not implemented; argument marshalling is string-only, because inferring a D-Bus signature from JSON would silently send the wrong type. |
| `tray` | ○ | `ksni` is a dependency; nothing is wired. |
| `notify` | ✅ | Full `org.freedesktop.Notifications`: actions, hints, replace-id, progress. |
| `secrets` | ○ | |
| `systemd` | ○ | |
| `udev` | ○ | |

### Tier 3 — windowing and shell

| Module | State | Notes |
|---|---|---|
| `window` | ◐ | `setTitle`, `resize`, `move`, `fullscreen`, `minimize`, `maximize`, `setAlwaysOnTop`, `close`. `setOpacity`, `setInputRegion`, `open` not implemented. |
| `layer` | ⛔ | `listOutputs` works. Everything else needs a layer surface — see §4. |
| `menu` | ◐ | State is accepted and stored; not rendered. |
| `palette` | ◐ | Commands are registered; the overlay is not rendered. |
| `shortcut` | ○ | |
| `clipboard` | ◐ | Text, HTML, file lists, image *read*. Image write not implemented. |
| `dnd` | ◐ | The launcher accepts dropped `.hta` files. Page-level drag in/out is not wired. |

### Tier 4 — reach

`os` ✅ · `shell` ✅ · `net` ○ · `serial` ○ · `usb` ○ · `bluetooth` ○ · `ffi` ○ · `plugin` ○

`ffi` and `plugin` have manifest permissions, risk descriptions, and consent-sheet copy, but no
implementation. That ordering is deliberate: the model that governs them exists before they do.

---

## 4. Blocked, and on what

### Layer-shell and session-lock (§8.3)

A `zwlr_layer_surface_v1` is a surface the compositor positions; the client still has to render into
it. GPUI owns rendering, and `gpui` 0.2.2 exposes no way to create a window from a foreign Wayland
surface. So a bar would have a shell and nothing to draw with.

The runtime refuses these modes with a message that distinguishes the two possible causes, because
only one of them is something a user can act on:

```console
$ htmlapp examples/wayland-bar.hta
htmlapp: `layer` windows cannot run here: this compositor offers the protocol, but the engine
backend that ships today attaches to an X11 window and cannot render into a layer surface. This
mode is waiting on the offscreen backend described in PRD §6.3.
```

The manifest-to-protocol translation is implemented and tested in `htmlapp-wayland` (anchor
bitmasks, layer values, the non-obvious `keyboard_interactivity` numbering), so the remaining work
is surface creation, not semantics.

### Native views being visible (§10)

Implemented, tested, positioned, and fed by the bridge — but underneath the page, because a native
child surface paints last. Nothing about the views changes when the offscreen backend lands.

### One workaround worth deleting

`gpui` 0.2.2 does not implement `HasWindowHandle` for its X11 window:

```rust
impl rwh::HasWindowHandle for X11Window {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        unimplemented!()
    }
}
```

Calling it panics, and the window id it wraps is `pub(crate)`. `crates/htmlapp-engine/src/x11_window.rs`
therefore recovers the id from the X server via `_NET_WM_PID`. It works and is bounded, but it is a
workaround for a gap in one dependency version. Implementing that trait upstream deletes the file;
the offscreen backend deletes the need for it.

---

## 5. Smaller gaps

- **`--sandbox`** (§11.2 rule 7) — `bwrap` is present on most systems and the granted path set is
  already computed; the re-exec is not written.
- **Accessibility** (§17 Q2) — unaddressed, as the PRD expects. AT-SPI sees the WebKit child window
  directly here, which is accidentally *better* than the offscreen backend will manage without work.
- **DPI and fractional scaling** (§17 Q3) — the page gets its own scale from GTK and the chrome gets
  its own from GPUI. Not reconciled.
- **Import revalidation** (§17 Q5) — resolved as the PRD leans: cached modules are never
  revalidated, and `htmlapp cache purge` is the explicit escape.
- **Recents privacy** (§17 Q8) — resolved as store-with-purge, plus `--private`.
- **CLI-time grants** (§17 Q6) — resolved: `htmlapp permissions allow <file>` exists, prints what it
  is granting, and is what headless and layer-shell documents are pointed at.

---

## 6. Test coverage

105 tests, concentrated on the security boundary rather than spread evenly:

| Area | Tests | What they pin down |
|---|---|---|
| `htmlapp-caps` | 26 | Manifest shapes from §8.2/§8.3 verbatim; symlink and `..` escapes; wildcard-origin refusal; consent invalidation on edit; a stored record being unable to widen a grant. |
| `htmlapp-bridge` | 15 | Ungranted modules refused even when the handler is registered; streams terminating rather than hanging; JS string escaping against `</script>` and U+2028/9; the emitted `.d.ts` covering the whole catalog. |
| `htmlapp-api` | 17 | Enforcement at the call site: symlink reads, read-not-implying-write, `rename` needing write on both ends, `fs.list` omitting ungranted entries, `/tmp/echo` not satisfying a grant of `echo`, credential-shaped env vars withheld, blob tokens only minted for granted paths. |
| `htmlapp-engine` | 18 | Asset-root escapes; CSP `connect-src` following the manifest; `*.example.com` not matching `notexample.com`; integrity-mismatched modules refused and not cached; blob tokens not traversable. |
| `htmlapp-views` | 10 | Real escape-sequence handling, wrapping, resize-from-pixels; table streaming and column invalidation. |
| `htmlapp-wayland` | 10 | Anchor bitmasks, layer values, protocol numbering; misconfiguration warnings. |
| `htmlapp-build` | 9 | Staple round-trip; magic bytes in the payload not confusing extraction; `.html` not hijacked; remote icon URLs not fetched. |

Five real bugs were found during development and are fixed, each now with a regression test:

1. `globset`'s `*` crossed `/` by default, so `/var/log/nginx/*.log` matched `nginx/deep/access.log`.
2. `*.example.com` matched `notexample.com` — domain-suffix confusion in the navigation check.
3. `serde_json` escaping alone let `</script>` through into an injected script string.
4. `random_token` used `std::fs::read("/dev/urandom")`, which reads to EOF — and that device has
   none. Minting a blob URL hung forever and allocated without bound.
5. A headless document that threw never called `htmlapp.exit`, so it hung whatever shell pipeline
   it was part of. Uncaught errors and rejections now report on stderr and exit non-zero.

---

## 7. Verified end-to-end on this machine

Ubuntu 26.04, KDE/Wayland, NVIDIA RTX 2060, WebKitGTK 4.1.

- Launcher window renders with recents, examples discovered from the checkout, and the diagnostics
  strip.
- A document window opens with a native GPUI titlebar and the page live inside it, served over
  `htmlapp://app/`, with the injected shim reporting `Frozen: yes`.
- `printf … | htmlapp csv-report.hta --headless` reads stdin, runs page script, writes stdout, and
  exits with the code the page asked for. A document that throws exits 1 with the error on stderr.
- `fs.blob` mints a token, the page `fetch()`es the URL directly and gets the bytes; an unminted
  token 404s.
- `htmlapp build` produces a stapled binary that runs its own embedded document with no arguments.
- Deny-by-default, CLI grant, in-scope read, out-of-scope refusal, and re-prompt-on-edit all confirmed
  through the real bridge rather than in isolation.
