# HTML App — Product Requirements Document

**A single `.hta` file, running as a real Linux desktop application.**

| | |
|---|---|
| Project | HTML App |
| Binary | `htmlapp` |
| Owner | monzeromer-lab |
| Status | Draft v0.2 |
| Date | 2026-09-10 |
| License | Apache-2.0 (proposed) |
| Repo | `monzeromer-lab/htmlapp` (proposed) |

---

## 1. The name

**HTML App** is what an HTA already is. Microsoft's own expansion of the acronym is "HTML Application", so the name states the category the project belongs to without inventing a metaphor for it.

It deliberately does not say "HTA" in the name. Naming it `openhta` or similar would promise `mshta.exe` compatibility, and this project is not compatible: the manifest format, the API surface, the permission model, and the window modes are all different by design. The extension is inherited; the semantics are not.

The name also survives scope growth. When a `.hta` becomes a Wayland panel or hosts a native terminal view, "HTML App" is still accurate in a way that a format-specific name would not be.

Naming conventions this document assumes:

- Display name: **HTML App**
- CLI: `htmlapp`
- Library crate: `htmlapp-runtime`
- File extension: **`.hta`** — HTML syntax, HTA extension, exactly as on Windows
- MIME type: `application/hta` (Linux registration `application/x-hta` as alias)
- Manifest block: `<script type="application/htmlapp+json">`
- JS global: `htmlapp`
- Custom scheme served to the page: `htmlapp://app/`
- User paths: `~/.config/htmlapp/`, `~/.cache/htmlapp/`, `~/.local/share/htmlapp/`

Check `crates.io/crates/htmlapp` before the first publish. Avoid shipping an `ha` shell alias by default, since it collides with the Home Assistant CLI on many Linux machines.

---

## 2. Problem

Windows HTAs let anyone build a real desktop tool out of one HTML file. No build step, no bundler, no packaging, no `node_modules`. You wrote a file, you double-clicked it, and you had an application with filesystem access. That workflow died with MSHTML, and nothing replaced it.

What exists today instead:

- **Electron / Tauri** solve a different problem. They are frameworks for shipping products, with project scaffolding, a build pipeline, and a bundling step. Overkill for a 200-line internal tool.
- **A browser tab** has none of the capabilities. No filesystem, no subprocesses, no native windowing, no tray, no system integration.
- **Native toolkits** (GTK, Qt, GPUI directly) require a compile cycle for every change and a language most tool authors don't want to write.

On Linux specifically there is a second gap. There is no good way to write a Wayland panel, a desktop widget, a status bar, or a launcher using web technology. Those are exactly the small, personal, disposable tools people most want to build quickly.

**HTML App is the missing runtime**: a single native binary that turns a `.hta` file into a first-class Linux application, with a capability model that makes that safe.

---

## 3. Vision

> Write one `.hta` file. Run it. It's an app.

The finished product is a ~30 MB binary with three ways in: point it at a file, double-click a `.hta` in your file manager, or launch it from the application menu and pick one.

```sh
htmlapp dashboard.hta
```

A native window opens, with a GPUI-rendered title bar, the page composited inside it as a texture, and a set of native APIs available to the page's JavaScript, scoped to exactly what the file declared it needs. Native GPUI widgets (a real terminal, a virtualized table, a code editor) can be placed inline in the HTML layout. Modals, menus, and popovers painted by GPUI appear *over* the page, not behind it.

The same file can also be a Wayland panel, a tray applet, a lock screen, or a headless CLI filter, depending on one line of manifest.

Launched from the application menu with no document, it opens a native launcher: what this is, a button to open a `.hta`, your recent documents, and the bundled examples. See §7.

---

## 4. Goals and non-goals

### 4.1 Goals

**G1. Zero-build.** The input is a file, not a project. No `npm`, no bundler, no config directory, no scaffolding command. Editing the file and saving it is the entire dev loop.

**G2. Correct compositing.** The page is a texture inside GPUI's scene graph, not a native child window layered on top. Native UI paints over the page. Clipping, rounded corners, opacity, shadows, and transforms all apply to it.

**G3. Linux-first, Wayland-native.** Wayland is the primary target and X11 is the fallback, not the other way round. Layer-shell, portals, and D-Bus are first-class, not afterthoughts.

**G4. Capability-scoped by default.** A `.hta` file with no manifest gets no native APIs at all. Every capability is declared, reviewed by the user once, and pinned to the file's hash.

**G5. Deep system integration.** Portals, D-Bus, PTY, PipeWire, tray, global shortcuts, secrets, udev. The things that make a tool feel native on Linux rather than like a page in a frame.

**G6. Single-file distribution.** `htmlapp build tool.hta` produces a standalone executable, plus a `.desktop` entry, plus an AppImage. The user of the tool never needs to know HTML App exists.

**G7. No wrong way in.** Opening a `.hta` from a file manager, the application menu, a shell, or a drag-and-drop all work and all reach the same place. Launching with no document is a normal launch, not a usage error.

### 4.2 Non-goals

**N1. Not a framework.** No component library, no reactivity system, no router, no CLI scaffolding. HTML App has no opinion about what's inside the HTML.

**N2. Not a browser.** No tabs, no address bar, no history UI, no arbitrary web browsing. Navigation off-origin is blocked unless declared.

**N3. Not a general Electron replacement.** Large multi-page applications with a build pipeline are better served by Tauri. HTML App optimizes for the single-file case and accepts the tradeoffs.

**N4. Not cross-platform at v1.** macOS and Windows are architecturally reachable and explicitly designed for, but out of scope for 1.0. See §12None.

**N5. No hidden network access.** The runtime never phones home, never auto-updates, never fetches a remote runtime.

---

## 5. Users and use cases

### 5.1 Primary personas

**The internal tooling author.** Writes a log triage viewer, a deploy dashboard, or a database browser for their team. Today they either build a web app with a backend, or they don't build it. They want a file they can drop in Slack.

**The Linux desktop tinkerer.** Runs Hyprland, Sway, or Niri. Wants a custom bar, a workspace switcher, a notification center, or a clipboard manager. Today the options are AGS/Astal, eww, or writing GTK. They want to write CSS.

**The data analyst.** Has a CSV, wants a chart and a filterable table, wants to hand the result to a colleague as one file.

**The tool author who ships.** Builds something small and useful, wants to distribute it without asking users to install a runtime.

### 5.2 Reference use cases

| # | Use case | Key capabilities exercised |
|---|---|---|
| UC1 | Log triage viewer with live tail and regex filter | `fs.watch`, `fs.readStream`, `process.spawn` (ripgrep), native table view |
| UC2 | Wayland top bar with workspaces, clock, tray | layer-shell, D-Bus, StatusNotifierItem, exclusive zone |
| UC3 | Git repo browser with an embedded terminal | `process.pty`, native terminal view, `fs` |
| UC4 | SQLite / DuckDB query notebook | `sql`, native table view, `dialog.save` |
| UC5 | Screenshot annotator | portal Screenshot, clipboard images, canvas |
| UC6 | CSV → chart report, piped in a shell script | headless mode, stdin/stdout |
| UC7 | Serial device console for embedded work | `serial`, `process`, streaming bridge |
| UC8 | Personal launcher / command palette overlay | layer-shell overlay, global shortcut via portal, `shell.exec` |

---

## 6. Architecture

### 6.1 The central decision

The page is rendered **offscreen** and composited **into** GPUI's scene, rather than being a native surface layered on top of the GPUI window.

This is the single most important architectural commitment in the project, and it is a deliberate reversal of the approach taken in the predecessor project `wf-studio`, which used `gpui-component`'s `WebView` (the same code shipped standalone as `gpui-wry`, wrapping `wry`).

That approach layers a native child window over the GPUI window. Its consequences, observed directly in `wf-studio`:

- The webview paints over everything GPUI draws, so any modal or popover forces the webview to be **removed from the element tree entirely** while the dialog is open.
- The parent's `rounded()` and `overflow_hidden()` do not clip it.
- Shadows, borders, and overlays cannot sit on top of it.
- It is macOS and Windows only. `wry` on Linux is WebKitGTK, which requires a GTK widget hierarchy and GTK main loop; GPUI on Linux uses raw `wayland-client` / `xcb` with a Blade/Vulkan renderer. The two windowing stacks cannot be composed in one process on Wayland, where there is no cross-toplevel reparenting.

Offscreen rendering eliminates all four. There is no `mount_webview` flag, no hide-on-modal dance, and no platform exclusion.

### 6.2 Engine choice: WPE WebKit

**WPE WebKit** is WebKit's embedded port. It has no GTK dependency, is designed for embedders who own their own compositor, and exposes rendered frames as buffers the host can consume.

The integration targets the **WPEPlatform** API, not the older libwpe / WPEBackend-fdo / Cog stack. Cog is at end of life; Igalia has been directing embedders to WPEPlatform, which was slated to be declared stable in the 2.54 release series and which provides zero-copy DMA-BUF buffer sharing across the graphics and multimedia layers. WPE 2.54 also brings damage tracking, which lets the runtime re-upload only changed regions.

Precedent: Avalonia ships WPE as its default Linux `NativeWebView` backend, rendering offscreen and compositing into the app's visual tree, with a software (Shm) mode as its default rendering path. The approach is proven in production.

**Rejected alternatives:**

| Engine | Why not (for v1) |
|---|---|
| `wry` / WebKitGTK | Cannot composite. Cannot work on Wayland inside a GPUI window. This is the thing being replaced. |
| CEF (offscreen) | Works, and gives the full Chromium feature set including WebGPU and WebCodecs. Costs 150–250 MB shipped and a multi-process bootstrap where the binary re-execs itself as renderer and GPU subprocesses. Kept as a **future optional backend** behind a feature flag for users who need Chromium parity. |
| Servo (`libservo`) | The most elegant fit, Rust-native, composites through surfman. Web compatibility is not yet sufficient for running arbitrary third-party HTML. Revisit in 2027. |

The backend is abstracted behind a `WebEngine` trait from day one so CEF and Servo can be added without touching the API layer.

### 6.3 Render pipeline, in two stages

GPUI's Blade renderer does not currently expose a public primitive for compositing an externally-owned GPU texture. Rather than block on that, the project ships in two stages.

**Stage 1 — Shm upload (v0.1 through v0.5).**

```
WPE (Shm mode) → CPU buffer → gpui::RenderImage → img() element
```

No GPUI patches required. Every compositing behaviour the project promises works immediately: modals over the page, clipping, opacity, transforms. Throughput is the only thing left on the table. WPE's damage tracking keeps the upload cost proportional to what actually changed, which for typical tool UIs is small. This stage exists to de-risk everything above the renderer: input routing, focus, IME, resize, DPI, the bridge, and the permission model.

**Stage 2 — Zero-copy DMA-BUF (v0.6+).**

```
WPE (dmabuf mode) → dmabuf fd → VK_EXT_external_memory_dma_buf → Vulkan image → GPUI external texture primitive
```

Requires adding an `external_texture` primitive to GPUI's Blade renderer. This is an upstreamable contribution to Zed and should be pursued as one rather than maintained as a fork. If upstream declines, a patched GPUI is vendored.

The `WebEngine` trait exposes both paths; the runtime negotiates at startup and falls back to Shm.

### 6.4 Process model

```
htmlapp (main process)
├── GPUI main thread — window, event loop, scene, native views
├── Tokio runtime — bridge dispatch, fs, process, net, sql
├── WPE UIProcess (in-process, GLib main context on its own thread)
│   └── WPEWebProcess (separate OS process, sandboxed by WebKit)
│       └── WPENetworkProcess (separate OS process)
└── optional: bubblewrap jail wrapping the whole tree (--sandbox)
```

WebKit's own multi-process sandbox is retained. The page's renderer never has direct access to the host's capabilities; everything goes through the bridge, which enforces the manifest.

### 6.5 Crate layout

```
htmlapp/
├── crates/
│   ├── htmlapp/              # binary: CLI, arg parsing, mode dispatch
│   ├── htmlapp-runtime/      # library: window lifecycle, app orchestration
│   ├── htmlapp-engine/       # WebEngine trait + WPE implementation
│   ├── htmlapp-engine-cef/   # optional backend, feature-gated
│   ├── htmlapp-bridge/       # JSON-RPC dispatch, JS shim, codegen for TS types
│   ├── htmlapp-caps/         # manifest parsing, permission model, consent store
│   ├── htmlapp-api/          # the native API modules (fs, process, dbus, ...)
│   ├── htmlapp-views/        # native GPUI views: terminal, table, editor
│   ├── htmlapp-shell/        # GPUI chrome: titlebar, menus, palette, dialogs
│   ├── htmlapp-wayland/      # layer-shell, session-lock, output management
│   └── htmlapp-build/        # `htmlapp build`: stapling, .desktop, AppImage, Flatpak
├── examples/                # one .hta per reference use case from §5.2
└── docs/
```

### 6.6 What carries over from wf-studio

The ergonomics, not the backend. These survive the engine swap unchanged and should be preserved:

- A `WebView` GPUI entity constructed with `cx.new(|cx| WebView::new(..., window, cx))`
- It implements `IntoElement`, so it drops into a normal tree via `.child(app.preview.clone())`
- `show()` / `hide()` on the entity
- Ordinary GPUI layout around it (`max_w`, `border_1`, `rounded`, `shadow`)

What goes away:

```rust
// no longer needed — modals composite over the page
let mount_webview = has_page && app.modal.is_none();
```

---

## 7. Entry points

### 7.1 Launch modes

The CLI is one way in, not the way in. A bare invocation with no document is a normal, expected launch, not a usage error.

| How it starts | What happens |
|---|---|
| Application menu, dock, or `htmlapp` with no args | **Launcher window** (§7.2) |
| `htmlapp path.hta` | Runs that document directly |
| Double-click a `.hta` in Nautilus / Dolphin / Thunar | Runs it, via `Exec=htmlapp %f` |
| "Open With → HTML App" | Runs it |
| `xdg-open notes.hta` | Runs it |
| Drag a `.hta` onto the launcher window | Runs it, in a new process |
| A stapled binary (`./tool`) | Runs its embedded document. The launcher never appears. |
| `htmlapp --headless report.hta` | No surface. stdin → stdout (§9.4) |
| `htmlapp permissions`, `htmlapp build`, `htmlapp types` | CLI subcommands, no window |

### 7.2 The launcher

The launcher is a native GPUI window with no webview in it at all. That is a deliberate side benefit: it exercises the shell, theming, dialog, and drag-and-drop layers independently of the engine, so it can ship in M1 before WPE is wired up, and it keeps working if the engine fails to initialize.

**Contents**

- **Identity.** Icon, "HTML App", version, a one-line description, and links to the repository, docs, and license.
- **Primary action.** A prominent **"Open an .hta file…"** button. It goes through the `xdg-desktop-portal` FileChooser, so the chosen file arrives pre-authorized for read regardless of the manifest's declared globs.
- **Drop target.** The entire window accepts a dragged `.hta`.
- **Recents.** The last ten documents, showing the name from each manifest, the path, when it was last opened, and a compact summary of what it was granted. Enter or click runs it. Context menu offers *Reveal in file manager*, *Revoke permissions*, and *Remove from recents*.
- **Examples.** The bundled documents from `/usr/share/htmlapp/examples`, one per reference use case in §5.2. One click runs them. These are the tutorial; there is no other onboarding.
- **New blank app.** Writes a minimal `.hta` containing a commented manifest and a hello-world body to a location the user picks, then hands it to the system's default text handler. This removes the blank-page problem without introducing a scaffolding command, which §4.2 N1 rules out.
- **Permissions…** Opens the consent manager (§11.2).
- **Diagnostics strip.** Session type (Wayland or X11), compositor, WPE version, active render path (dmabuf or Shm), and GPU. Copyable as a single block, so bug reports arrive with it.

**Keyboard.** `Ctrl+O` open, `Ctrl+N` new, `Enter` runs the selected recent, `Delete` removes it from the list.

The launcher respects the system color scheme and follows the same GPUI theme as the app chrome, so it reads as part of the desktop rather than as a splash screen.

### 7.3 Desktop integration

A single `.desktop` file serves both paths. `%f` is the whole mechanism: the application menu passes no argument and the launcher appears, while a file manager passes a path and the document runs.

```ini
[Desktop Entry]
Type=Application
Name=HTML App
GenericName=HTML Application Runtime
Comment=Run a single .hta file as a desktop application
Exec=htmlapp %f
Icon=htmlapp
Terminal=false
Categories=Development;Utility;
MimeType=application/hta;application/x-hta;
StartupWMClass=htmlapp
Actions=OpenFile;Permissions;

[Desktop Action OpenFile]
Name=Open an .hta file…
Exec=htmlapp --open

[Desktop Action Permissions]
Name=Manage permissions
Exec=htmlapp --permissions-ui
```

Shipped alongside it: `~/.local/share/mime/packages/htmlapp.xml` (or the system path for a package install) declaring `application/hta`, an icon set at the standard hicolor sizes, and a `.hta` file-type icon so documents are visually distinguishable in a file manager.

### 7.4 Process and instance model

**Each document runs in its own process.** Isolation is a security property, not an implementation detail, so two open `.hta` files never share a bridge, a permission grant, or an address space.

**The launcher is single-instance.** Opening a file from it spawns a detached child; the launcher stays up. Closing the last document does not close the launcher, and closing the launcher does not kill running documents.

A document launched directly, from the CLI or a file manager, never shows the launcher.

Apps built with `htmlapp build` (§13) get their own `.desktop` entry and their own icon, appear in the application menu under their own name, and never route through any of this.

---

## 8. The document format

### 8.1 Design principle

Config lives in the document, as it did in HTA's `<HTA:APPLICATION>` tag. A `<meta>` tag is too weak to carry a permission list, so HTML App uses a typed script block that the host parses **before** handing the file to the engine. The block is inert to browsers, so the document is still valid HTML: renaming a `.hta` to `.html` opens it in any browser, without native APIs. That makes browser devtools a usable fallback while authoring.

### 8.2 Manifest

```html
<!DOCTYPE html>
<!-- saved as: log-triage.hta -->
<html>
<head>
<script type="application/htmlapp+json">
{
  "name": "Log Triage",
  "id": "dev.monzer.logtriage",
  "version": "1.2.0",
  "icon": "data:image/svg+xml;base64,...",

  "window": {
    "title": "Log Triage",
    "width": 1200, "height": 800,
    "min_width": 640,
    "titlebar": "native",
    "background": "transparent",
    "resizable": true
  },

  "permissions": {
    "fs": {
      "read":  ["~/logs/**", "/var/log/nginx/*.log"],
      "write": ["~/.local/share/logtriage/**"]
    },
    "process": { "exec": ["rg", "journalctl"], "pty": false },
    "net": { "fetch": ["https://alerts.internal.corp/*"] },
    "sql": { "databases": ["~/.local/share/logtriage/index.db"] },
    "clipboard": ["read", "write"],
    "notifications": true
  }
}
</script>
</head>
<body>...</body>
</html>
```

### 8.3 Window modes

The `window.mode` field selects what kind of surface the file becomes.

```json
{ "window": { "mode": "layer",
              "layer": "top",
              "anchor": ["top", "left", "right"],
              "exclusive_zone": 34,
              "keyboard_interactivity": "on-demand",
              "output": "primary" } }
```

| Mode | Surface | Use |
|---|---|---|
| `window` (default) | `xdg_toplevel` | Ordinary application |
| `layer` | `wlr-layer-shell` | Bars, docks, widgets, overlays, launchers |
| `lock` | `ext-session-lock` | Lock screens |
| `tray` | No surface until clicked | Applets and menu-bar tools |
| `headless` | None | CLI filters, stdin → stdout |

### 8.4 Origin and loading

The document is served over `htmlapp://app/` rather than `file://`. This gives it a real, stable origin, so `localStorage`, IndexedDB, service workers, ES module imports, and a meaningful CSP all work. Sibling assets resolve relative to the source file's directory, but only if the manifest opts into `"assets": "sibling"`; the default is strictly single-file.

### 8.5 Multi-file, without a build step

A file may declare an import map. HTML App resolves and caches remote modules on first run into `~/.cache/htmlapp/modules/<sha256>`, pinned by integrity hash, then serves them from cache offline forever after.

```json
{ "imports": {
    "d3": { "url": "https://esm.sh/d3@7", "integrity": "sha384-..." }
} }
```

This keeps the zero-build promise while letting single files use real libraries.

---

## 9. The bridge

### 9.1 Transport

A JSON-RPC dispatcher over WPE's script message handlers, with an initialization script injected at document-start. Three message shapes, not one.

| Shape | JS surface | Use |
|---|---|---|
| Request/response | `await htmlapp.invoke(m, p)` | Ordinary calls |
| Stream | `for await (const c of htmlapp.stream(m, p))` | Tail, spawn output, fs watch, query results |
| Event | `htmlapp.on(evt, cb)` | Theme change, window state, D-Bus signals, hotkeys |

```js
// injected at document-start, before any page script runs
const htmlapp = {
  invoke(method, params) { /* Promise, correlated by id */ },
  stream(method, params) { /* AsyncIterable + .cancel() */ },
  on(event, handler)     { /* returns unsubscribe fn */ },
  version: "1.0.0",
  permissions: { /* frozen copy of what was granted */ }
};
Object.freeze(htmlapp);
```

**Streaming is not optional.** Tailing a log, reading a 2 GB file, or piping a subprocess falls apart if every payload must be one base64 blob. For genuine bulk transfer the host mints a `blob://` URL the page fetches directly, so bytes never pass through JSON.

### 9.2 Type safety

`htmlapp types > htmlapp.d.ts` emits TypeScript declarations generated from the Rust API definitions, so single-file authors get completion in their editor without a build step.

### 9.3 API catalog

Every module is gated on a corresponding manifest permission. Absent permission, the module is not injected at all — the property does not exist, so feature detection works naturally.

#### Tier 1 — the core

**`fs`** — `read`, `write`, `append`, `stat`, `list`, `glob`, `mkdir`, `remove`, `rename`, `copy`, `readStream`, `writeStream`, `watch` (inotify), `mmap`. Every path checked against the granted globs, with symlink resolution before the check.

**`process`** — `spawn` with streamed stdin/stdout/stderr, `exec` for one-shots, `pty` via `portable-pty` for terminal-backed tools, `kill`, `signal`, `list`. Executable allow-list from the manifest.

**`dialog`** — `open`, `save`, `pickFolder`, `message`, `confirm`, `prompt`. Routed through the `xdg-desktop-portal` FileChooser, so the picker is the system's own and files arrive pre-authorized outside the granted globs.

**`http`** — a fetch that ignores CORS, supports arbitrary methods, streaming bodies, client certificates, and proxies. Origin allow-list from the manifest.

**`sql`** — bundled SQLite, with DuckDB behind a feature flag. Prepared statements, streamed result sets, transactions.

**`store`** — a simple persistent key-value store scoped to the app id, for the 80% case that doesn't need SQL.

#### Tier 2 — Linux desktop integration

**`portal`** — the `xdg-desktop-portal` surface: `Screenshot`, `ScreenCast` (PipeWire), `GlobalShortcuts`, `Inhibit`, `OpenURI`, `Background`, `Notification`, `Wallpaper`, `Location`. Using portals means HTML App's prompts are the system's prompts, and Flatpak confinement comes free.

**`dbus`** — session and system bus. `call`, `signal`, `subscribe`, `own_name`, `export_object`. This is the single highest-leverage API on Linux: it gives a page access to NetworkManager, UPower, logind, BlueZ, MPRIS, and everything else on the bus without HTML App wrapping each one.

**`tray`** — StatusNotifierItem via `ksni`. Icon, tooltip, native menu, activation callbacks.

**`notify`** — `org.freedesktop.Notifications` with actions, hints, replace-id, and progress.

**`secrets`** — Secret Service / libsecret. `get`, `set`, `delete`, `search`. Credentials never touch the page's storage.

**`systemd`** — journal reading with filters and follow, user unit `start`/`stop`/`status`, `systemd-run` for scoped transient units.

**`udev`** — device enumeration and hotplug events.

#### Tier 3 — windowing and shell

**`window`** — `setTitle`, `resize`, `move`, `fullscreen`, `minimize`, `maximize`, `setOpacity`, `setInputRegion` (click-through), `setAlwaysOnTop`, `open` (additional windows from the same file), `close`.

**`layer`** — for `mode: "layer"`: `setAnchor`, `setExclusiveZone`, `setLayer`, `setKeyboardInteractivity`, `setMargin`, `setOutput`, plus `outputs.list()` and `outputs.on("change")`.

**`menu`** — application menu bar and context menus, defined as JSON from JS, rendered by GPUI, with callbacks routed back over the bridge.

**`palette`** — a native command palette overlay. `register(commands)`, `open()`, `close()`. Cheap in GPUI, and it is what makes an HTML App feel like Zed rather than a web page in a frame.

**`shortcut`** — global hotkeys via the portal `GlobalShortcuts` interface on Wayland, with an X11 grab fallback.

**`clipboard`** — text, HTML, images, and file lists, both directions. Watch for changes.

**`dnd`** — drag and drop of files in and out, including drag-out-to-file.

#### Tier 4 — reach

**`net`** — raw TCP, UDP, and Unix sockets; an embedded HTTP and WebSocket **server**, so an HTML App tool can be something other clients talk to.

**`serial`** / **`usb`** / **`bluetooth`** — `serialport`, `rusb`, `bluer`. Turns HTML App into a viable host for hardware and embedded tooling.

**`os`** — platform, distro, kernel, arch, env vars, XDG paths, uptime, battery, theme and color-scheme change events, locale, idle time.

**`shell`** — `open` (a URL or path in the user's default handler), `trash`, `showInFileManager`.

**`ffi`** — `dlopen` an arbitrary shared object and call into it. Behind the loudest permission in the system, off by default, and never granted implicitly.

**`plugin`** — a `wasmtime` host for third-party extensions. Sandboxed, portable, and the recommended alternative to `ffi` for anything redistributable.

### 9.4 Headless mode

```sh
cat access.log | htmlapp report.hta --headless --format json > summary.json
```

The document runs with no surface. `htmlapp.stdin` and `htmlapp.stdout` are available; the process exits when the page calls `htmlapp.exit(code)`. This makes a `.hta` file a legitimate participant in a shell pipeline, and it is how UC6 works.

---

## 10. Native views

Because the page is a texture, GPUI can draw on top of it. That enables something no other web-based desktop runtime offers: **real native widgets placed inline in HTML layout.**

```html
<htmlapp-view kind="terminal" id="term" style="flex:1"></htmlapp-view>
<htmlapp-view kind="table"    id="rows" style="height:400px"></htmlapp-view>
```

Mechanism: the injected shim registers a custom element. A `ResizeObserver` reports each element's viewport rect over the bridge. The host renders a GPUI element at that rect, above the page texture, and routes input events that land inside it. The HTML element itself is a transparent placeholder that participates in normal layout.

Shipped views for v1:

| `kind` | Backed by | Why it can't be HTML |
|---|---|---|
| `terminal` | `alacritty_terminal` | Real PTY, real escape handling, 60fps scrollback |
| `table` | GPUI virtualized table | Millions of rows without DOM |
| `editor` | GPUI editor + tree-sitter | Syntax highlighting, multi-cursor, LSP-ready |
| `video` | GStreamer / dmabuf | Zero-copy playback and capture |
| `canvas3d` | Direct Vulkan surface | Native GPU work without WebGL's limits |

Views are addressable from JS: `htmlapp.view("term").write("ls\n")`.

---

## 11. Security

### 11.1 The failure to avoid

HTA became a malware delivery vector because opening a file **was** granting full trust. There was no manifest, no prompt, no scoping. HTML App has to be obviously and structurally different, or it deserves the same reputation.

### 11.2 Model

1. **Deny by default.** No manifest, or no `permissions` block, means no native APIs. The file is a page in a window. This is the safe default and it is silent, not an error.
2. **Declarative, host-enforced.** Permissions live in the manifest and are enforced in Rust. Nothing in JS can widen them. The `htmlapp` object is frozen and modules are simply absent when ungranted.
3. **Hash-pinned consent.** On first run of an unknown file, a native GPUI sheet lists exactly what was requested, in plain language, with the source path. The decision is stored keyed on `sha256(file)`. Editing the file invalidates consent and re-prompts, with a diff of what changed in the permission set.
4. **Portals over prompts.** Where `xdg-desktop-portal` covers a capability (file choosing, screenshot, screencast, global shortcuts, background), HTML App uses it. The user sees their desktop's own dialog, and HTML App never holds a broad grant it doesn't need.
5. **Path scoping with symlink resolution.** `fs` globs are resolved to canonical paths before checking, so `~/logs/link-to-etc` cannot escape.
6. **Revocable.** `htmlapp permissions` lists every consented file and what it holds. `htmlapp permissions revoke <file>` clears it. A native settings window does the same.
7. **Defense in depth.** `--sandbox` re-execs the whole tree under `bubblewrap` with only the granted paths bind-mounted. WebKit's own renderer sandbox is retained underneath.
8. **CSP by default.** The runtime injects a restrictive CSP unless the manifest overrides it. Off-origin navigation is blocked unless declared.

### 11.3 Threat model, briefly

| Threat | Mitigation |
|---|---|
| Malicious file from Slack/email | Deny-by-default; consent sheet; hash pinning |
| Supply chain via `imports` | Integrity hashes required; cached and pinned on first fetch |
| Privilege escalation from renderer | WebKit multi-process sandbox; bridge is the only channel |
| Exfiltration via `http` | Origin allow-list; no wildcard `*` accepted |
| Consent fatigue | Portals for common cases; few, meaningful, plain-language prompts |
| Trojan update to a trusted file | Hash change re-prompts, showing the permission diff |

`ffi` is exempt from none of this and is documented as the escape hatch that voids the model.

---

## 12. Platform support

| Platform | v1.0 | Path |
|---|---|---|
| Linux / Wayland | **Primary** | WPE offscreen → GPUI. Full feature set. |
| Linux / X11 | **Supported** | Same pipeline. Layer-shell unavailable; global shortcuts via X grab. |
| macOS | Post-1.0 | WKWebView has no offscreen path. Requires CEF backend, or `WKWebView` + `CALayer` interop with reduced compositing. |
| Windows | Post-1.0 | WebView2 visual hosting (`CreateCoreWebView2CompositionController`) into a DirectComposition tree composites correctly with GPUI's DX backend. Genuinely viable, just not first. |

The `WebEngine` trait and the entire API layer above it are written platform-agnostically from the start so these are additions, not rewrites.

Runtime dependencies on Linux: `libwpe`, `wpewebkit-2.0`, `glib`, `vulkan-loader`, `libxkbcommon`, `wayland-client`. Ubuntu packaging may require Igalia's builds or a bundled WPE in the AppImage; this is tracked as R3.

---

## 13. Packaging and distribution

**For HTML App itself:** static-ish binary, `.deb`, AUR, Nix flake, AppImage, and a Flatpak. `curl | sh` installer as a convenience.

**For apps built with HTML App:**

```sh
htmlapp build tool.hta
# → tool                    (standalone binary, HTML stapled into a copy of the runtime)
# → tool.desktop            (with icon extracted from the manifest)
# → tool-x86_64.AppImage
# → com.example.tool.json   (Flatpak manifest)
```

Stapling appends the document to a copy of the runtime with a trailer containing offset and length, which the runtime detects at startup. The end user gets one file and never learns the word "HTML App".

**File association.** Installing HTML App registers the `application/hta` MIME type and takes ownership of the `.hta` extension, so double-clicking a `.hta` launches it exactly the way double-clicking one on Windows launches `mshta.exe`. Plain `.html` is deliberately **not** hijacked: the extension is the signal that a file expects a runtime, and hijacking `.html` would break every browser on the system. The `.desktop` entry, MIME registration, and icons are specified in §7.3.

Every installation method (`.deb`, AUR, Nix, Flatpak, AppImage via `appimaged`, and the `curl | sh` installer) must perform this registration and run `update-desktop-database` and `update-mime-database`. An install that leaves double-click broken is a failed install.

---

## 14. Roadmap

| Milestone | Target | Contents |
|---|---|---|
| **M0 — Spike** | 2 weeks | WPE Shm frames uploaded into a `RenderImage`, painted in a GPUI window. Input routing. Proves §6.3 stage 1. |
| **M1 — Runtime** | +4 weeks | Manifest parsing, `htmlapp://` scheme, CSP, window modes `window` and `headless`, resize/DPI/IME, native titlebar. **Launcher v1** (identity, Open button, drop target, diagnostics), `.desktop` + MIME registration, bundled examples. |
| **M2 — Bridge** | +4 weeks | JSON-RPC, streams, events, `blob://`, `fs`, `process` (incl. PTY), `dialog` via portal, `store`. TypeScript emit. |
| **M3 — Capabilities** | +3 weeks | Permission model, consent sheet, hash pinning, `htmlapp permissions`, `--sandbox`. **Launcher v2**: recents with permission summaries, consent manager UI, New blank app. **First public release, v0.1.** |
| **M4 — Linux native** | +5 weeks | `dbus`, `portal`, `tray`, `notify`, `secrets`, layer-shell mode, `shortcut`, `clipboard`. UC2 ships as an example. |
| **M5 — Native views** | +5 weeks | `htmlapp-view` custom element, `terminal` and `table` views. UC1 and UC3 ship. |
| **M6 — Distribution** | +3 weeks | `htmlapp build`, stapling, per-app `.desktop` and icons, AppImage, Flatpak. **v0.5.** |
| **M7 — Zero-copy** | +6 weeks | dmabuf path, Vulkan import, GPUI `external_texture` primitive upstreamed to Zed. **v0.6.** |
| **M8 — Reach** | ongoing | `sql`, `net`, `serial`/`usb`/`bluetooth`, `plugin` (wasmtime), `editor` and `video` views. **v1.0.** |

M0 through M3 is the critical path to something usable and honest. Everything after is additive.

---

## 15. Success metrics

| | 6 months | 12 months |
|---|---|---|
| GitHub stars | 500 | 3,000 |
| Third-party apps published | 20 | 200 |
| Time from empty file to running window (new user) | < 5 min | < 2 min |
| New user reaches a running example without touching a terminal | Yes | Yes |
| Cold start to first paint | < 400 ms | < 250 ms |
| Idle memory, trivial app | < 120 MB | < 90 MB |
| Binary size (runtime, excl. WPE) | < 40 MB | < 30 MB |
| Frame time, stage 2, 1080p | — | < 4 ms |

Qualitative bar for v1.0: someone unfamiliar with the project can write a useful tool in a single file, in an afternoon, without reading past the quickstart.

---

## 16. Risks

| # | Risk | Severity | Mitigation |
|---|---|---|---|
| R1 | GPUI has no external-texture primitive; upstream may decline | High | Stage 1 Shm path ships without it. Stage 2 is an optimization, not a gate. Vendor a patched GPUI if needed. |
| R2 | WPEPlatform API churns before 2.54 stabilizes | Medium | Pin a version; isolate all WPE contact inside `htmlapp-engine`; the `WebEngine` trait keeps the blast radius small. |
| R3 | WPE packaging on Ubuntu is immature | Medium | Bundle WPE in the AppImage and Flatpak; document building from Igalia's repos; consider vendoring. |
| R4 | WebKit lags Chromium (WebGPU, WebCodecs, newer CSS) | Medium | Document the gap honestly. CEF backend behind a feature flag for users who need parity. |
| R5 | GPUI is pre-1.0 and moves fast | Medium | Pin exact versions; keep GPUI contact confined to `htmlapp-shell` and `htmlapp-views`. |
| R6 | Security reputation, "HTA all over again" | High | Lead with the capability model in all messaging. Deny-by-default. Third-party audit before v1.0. |
| R7 | Input, focus, and IME in an offscreen webview are notoriously fiddly | High | Front-loaded into M0/M1 deliberately. This is the real hard part, not the rendering. |
| R8 | Scope creep across a very large API surface | Medium | Tiers in §9.3 are a priority order, not a wish list. M3 ships with Tier 1 only. |
| R9 | `.hta` is heavily filtered: Gmail, Outlook, and most mail gateways block the extension outright, and AV heuristics treat it as hostile | High | Accept it as the cost of the format. `htmlapp build` produces a stapled binary and an AppImage for distribution, which is the recommended path for sharing. Document `.hta.txt` / archive workarounds for casual sharing. Revisit an alias extension if it proves fatal. |
| R10 | Windows dual-meaning: a `.hta` written for HTML App will silently open in `mshta.exe` on Windows and behave differently | Medium | The manifest block is inert to MSHTML, so an HTML App file degrades to a plain page rather than misbehaving. Document the collision. Post-1.0 Windows support should register as the handler explicitly. |

---

## 17. Open questions

1. **Input and IME.** Offscreen rendering means synthesizing every event. How well do WPEPlatform's input APIs handle IME preedit, dead keys, and Arabic/RTL text entry? This needs a spike in M0, not a decision on paper.
2. **Accessibility.** An offscreen webview breaks AT-SPI, because there is no native widget tree for a screen reader to walk. WPE exposes an accessibility bus; bridging it into GPUI's accessibility layer is unsolved and possibly significant work. Must be scoped before v1.0.
3. **DPI and fractional scaling.** Which layer owns scale: WPE's device pixel ratio, GPUI's, or both? Getting this wrong produces blurry text, which would undermine the whole premise.
4. **Native view input z-order.** When a `htmlapp-view` overlaps a page element with a `pointer-events` handler, who wins? Needs a documented rule before authors depend on the ambiguity.
5. **Import map caching semantics.** Should a cached module ever be revalidated? Leaning no, for determinism, with an explicit `htmlapp cache purge`.
6. **Consent UX for layer-shell.** A bar or a lock screen asking for permission on first launch is awkward, because there may be nothing on screen to attach a dialog to. Possibly a CLI-time grant for these modes.
7. **Name.** Confirm `crates.io/crates/htmlapp` availability before the first publish. Fallbacks: Quire, Hearth.
8. **Recents privacy.** The launcher's recents list stores paths, which may themselves be sensitive. Options: store them, store them with an easy purge, or make the list opt-in on first use. Leaning toward store-with-purge plus honoring a `--private` flag.
9. **Launcher suppression.** Some users will only ever use the CLI and will not want a window on a bare invocation. A `launcher = false` config key that makes `htmlapp` with no args print help instead. Low cost, probably worth it.
10. **Extension reuse.** Taking `.hta` inherits both HTA's recognition and its reputation (see R9, R10). Worth deciding early whether HTML App leans into the lineage in its messaging or treats the extension as a neutral technical choice.

---

## 18. Prior art

| | Single file | Composited | Linux-native | Capability model |
|---|---|---|---|---|
| HTA (`mshta.exe`) | Yes | N/A | No | None |
| Electron | No | Yes (Chromium) | Partial | None |
| Tauri | No | No (native webview overlay) | Partial | Allowlist |
| Neutralino | No | No | Partial | Partial |
| `eww` / AGS / Astal | No (config tree) | N/A | Yes | None |
| Cog / WPE launchers | Yes | N/A | Yes | None |
| **HTML App** | **Yes** | **Yes** | **Yes** | **Yes** |

The row that has never existed is the one with all four checked. That is the product.

---

## 19. Licensing

Apache-2.0 for HTML App itself. Dependency licenses are compatible: WPE WebKit is LGPL-2.1 plus BSD (dynamically linked, so no copyleft obligation on HTML App), GPUI is Apache-2.0, CEF is BSD-3-Clause. If DuckDB is bundled, note its MIT license in the notice file.

A `NOTICE` file and an SBOM ship with every release.
