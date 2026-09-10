# API reference

[← Docs index](README.md) · [Getting started](getting-started.md) ·
[Document format](document-format.md) · [Bridge](bridge.md) · **API** ·
[Security](security.md) · [Architecture](architecture.md) · [Building](building.md) ·
[Contributing](contributing.md) · [AGENT.md](AGENT.md)

---

**28 modules · 144 methods · 22 events.** Every module is gated on a manifest permission. Absent
permission means the module is not on `htmlapp` at all, so `typeof htmlapp.fs === "undefined"` is a
truthful feature test.

`→` is request/response (`await`). `⇉` is a stream (`for await`, cancellable).

The tables below are generated from
[`crates/htmlapp-bridge/src/catalog.rs`](../crates/htmlapp-bridge/src/catalog.rs), the same
declaration the JS shim and `htmlapp types` are built from. For exact payload types, run
`htmlapp types > htmlapp.d.ts`.

The tiers are a **priority order**, not a wish list. Tier 1 is what most tools need; by Tier 4 you
are into territory where you should ask whether a `.hta` is still the right shape.

---

## Tier 1 — the core

### `fs`

Filesystem access, scoped to the manifest's read and write globs. Every path is resolved through symlinks before it is checked.

| | Method | Params → returns |
|---|---|---|
| → | `read` | `{ path: string; encoding?: "utf8" \| "binary" }` → `string` |
| → | `write` | `{ path: string; contents: string; encoding?: "utf8" \| "binary" }` → `void` |
| → | `append` | `{ path: string; contents: string }` → `void` |
| → | `stat` | `{ path: string }` → `FileStat` |
| → | `list` | `{ path: string }` → `DirEntry[]` |
| → | `glob` | `{ pattern: string }` → `string[]` |
| → | `mkdir` | `{ path: string; recursive?: boolean }` → `void` |
| → | `remove` | `{ path: string; recursive?: boolean }` → `void` |
| → | `rename` | `{ from: string; to: string }` → `void` |
| → | `copy` | `{ from: string; to: string }` → `void` |
| → | `blob` | `{ path: string }` → `string` |
| → | `mmap` | `{ path: string; offset?: number; length?: number; encoding?: Encoding }` → `string` |
| ⇉ | `readStream` | `{ path: string; chunkSize?: number }` → `string` |
| ⇉ | `tail` | `{ path: string; lines?: number }` → `string` |
| ⇉ | `watch` | `{ path: string; recursive?: boolean }` → `WatchEvent` |

### `process`

Run other programs, restricted to the manifest's executable allow-list.

| | Method | Params → returns |
|---|---|---|
| → | `exec` | `{ program: string; args?: string[]; cwd?: string; env?: Record<string, string>; stdin?: string }` → `ProcessOutput` |
| → | `kill` | `{ pid: number }` → `void` |
| → | `signal` | `{ pid: number; signal: string }` → `void` |
| → | `list` | `void` → `ProcessInfo[]` |
| → | `write` | `{ pid: number; data: string }` → `void` |
| → | `resize` | `{ pid: number; cols: number; rows: number }` → `void` |
| ⇉ | `spawn` | `{ program: string; args?: string[]; cwd?: string; env?: Record<string, string> }` → `ProcessEvent` |
| ⇉ | `pty` | `{ program: string; args?: string[]; cwd?: string; cols?: number; rows?: number }` → `ProcessEvent` |

### `dialog`

The desktop's own file pickers, via xdg-desktop-portal. Files chosen here arrive pre-authorised, outside the manifest's globs.

| | Method | Params → returns |
|---|---|---|
| → | `open` | `{ title?: string; multiple?: boolean; filters?: FileFilter[]; startIn?: string }` → `string[]` |
| → | `save` | `{ title?: string; defaultName?: string; filters?: FileFilter[] }` → `string \| null` |
| → | `pickFolder` | `{ title?: string }` → `string \| null` |
| → | `message` | `{ title?: string; body: string }` → `void` |
| → | `confirm` | `{ title?: string; body: string }` → `boolean` |
| → | `prompt` | `{ title?: string; body: string; default?: string }` → `string \| null` |

### `http`

A fetch that ignores CORS, restricted to the manifest's origin allow-list.

| | Method | Params → returns |
|---|---|---|
| → | `fetch` | `HttpRequest` → `HttpResponse` |
| ⇉ | `stream` | `HttpRequest` → `string` |

### `sql`

Bundled SQLite, restricted to the databases named in the manifest.

| | Method | Params → returns |
|---|---|---|
| → | `open` | `{ database: string }` → `void` |
| → | `execute` | `{ database: string; sql: string; params?: SqlValue[] }` → `SqlExecuteResult` |
| → | `query` | `{ database: string; sql: string; params?: SqlValue[] }` → `SqlRow[]` |
| → | `transaction` | `{ database: string; statements: SqlStatement[] }` → `void` |
| → | `close` | `{ database: string }` → `void` |
| ⇉ | `stream` | `{ database: string; sql: string; params?: SqlValue[] }` → `SqlRow` |

### `store`

A persistent key-value store scoped to this app's id.

| | Method | Params → returns |
|---|---|---|
| → | `get` | `{ key: string }` → `unknown` |
| → | `set` | `{ key: string; value: unknown }` → `void` |
| → | `delete` | `{ key: string }` → `void` |
| → | `keys` | `void` → `string[]` |
| → | `clear` | `void` → `void` |

## Tier 2 — Linux desktop integration

### `portal`

xdg-desktop-portal. The prompts the user sees are their own desktop's, and Flatpak confinement comes free.

| | Method | Params → returns |
|---|---|---|
| → | `screenshot` | `{ interactive?: boolean }` → `string` |
| → | `pickColor` | `void` → `Color` |
| → | `openUri` | `{ uri: string; writable?: boolean }` → `void` |
| → | `inhibit` | `{ reason: string; flags?: string[] }` → `number` |
| → | `uninhibit` | `{ handle: number }` → `void` |
| → | `requestBackground` | `{ reason: string; autostart?: boolean }` → `boolean` |
| → | `setWallpaper` | `{ uri: string; target?: "background" \| "lockscreen" \| "both" }` → `void` |
| → | `location` | `void` → `Location` |
| ⇉ | `screenCast` | `{ multiple?: boolean; cursor?: "hidden" \| "embedded" \| "metadata" }` → `ScreenCastFrame` |

### `dbus`

Session and system bus. The highest-leverage API on Linux: NetworkManager, UPower, logind, BlueZ, and MPRIS without wrapping each one.

| | Method | Params → returns |
|---|---|---|
| → | `call` | `DbusCall` → `unknown` |
| → | `get` | `{ bus?: DbusBus; destination: string; path: string; iface: string; property: string }` → `unknown` |
| → | `set` | `{ bus?: DbusBus; destination: string; path: string; iface: string; property: string; value: unknown }` → `void` |
| → | `ownName` | `{ bus?: DbusBus; name: string }` → `void` |
| → | `introspect` | `{ bus?: DbusBus; destination: string; path: string }` → `string` |
| ⇉ | `subscribe` | `{ bus?: DbusBus; destination?: string; path?: string; iface?: string; member?: string }` → `DbusSignal` |

### `tray`

StatusNotifierItem, via ksni.

| | Method | Params → returns |
|---|---|---|
| → | `set` | `{ icon?: string; title?: string; tooltip?: string; menu?: MenuItem[] }` → `void` |
| → | `remove` | `void` → `void` |

Events:

- `tray:activate` → `{ x: number; y: number }` — The tray icon was clicked.
- `tray:menu` → `{ id: string }` — A tray menu item was chosen.

### `notify`

Desktop notifications with actions, hints, replace-id, and progress.

| | Method | Params → returns |
|---|---|---|
| → | `send` | `NotificationSpec` → `number` |
| → | `close` | `{ id: number }` → `void` |

Events:

- `notify:action` → `{ id: number; action: string }` — The user chose a notification action.
- `notify:closed` → `{ id: number; reason: string }` — A notification was dismissed.

### `secrets`

Secret Service. Credentials never touch the page's own storage.

| | Method | Params → returns |
|---|---|---|
| → | `get` | `{ key: string }` → `string \| null` |
| → | `set` | `{ key: string; value: string; label?: string }` → `void` |
| → | `delete` | `{ key: string }` → `void` |
| → | `search` | `{ attributes: Record<string, string> }` → `string[]` |

### `systemd`

Journal reading and user unit control.

| | Method | Params → returns |
|---|---|---|
| → | `status` | `{ unit: string }` → `UnitStatus` |
| → | `start` | `{ unit: string }` → `void` |
| → | `stop` | `{ unit: string }` → `void` |
| → | `restart` | `{ unit: string }` → `void` |
| → | `run` | `{ program: string; args?: string[]; properties?: Record<string, string> }` → `string` |
| ⇉ | `journal` | `{ unit?: string; since?: string; priority?: number; follow?: boolean }` → `JournalEntry` |

### `udev`

Device enumeration and hotplug events.

| | Method | Params → returns |
|---|---|---|
| → | `list` | `{ subsystem?: string }` → `DeviceInfo[]` |
| ⇉ | `monitor` | `{ subsystem?: string }` → `DeviceEvent` |

## Tier 3 — windowing and shell

### `window`

Control this document's own window.

| | Method | Params → returns |
|---|---|---|
| → | `setTitle` | `{ title: string }` → `void` |
| → | `resize` | `{ width: number; height: number }` → `void` |
| → | `move` | `{ x: number; y: number }` → `void` |
| → | `fullscreen` | `{ enabled: boolean }` → `void` |
| → | `minimize` | `void` → `void` |
| → | `maximize` | `{ enabled?: boolean }` → `void` |
| → | `setOpacity` | `{ opacity: number }` → `void` |
| → | `setInputRegion` | `{ rects: Rect[] \| null }` → `void` |
| → | `setAlwaysOnTop` | `{ enabled: boolean }` → `void` |
| → | `open` | `{ path?: string; title?: string; width?: number; height?: number }` → `number` |
| → | `close` | `void` → `void` |

Events:

- `window:resize` → `{ width: number; height: number; scale: number }` — The window was resized.
- `window:focus` → `{ focused: boolean }` — Focus entered or left the window.
- `window:state` → `{ maximized: boolean; fullscreen: boolean; minimized: boolean }` — The window state changed.
- `window:close` → `void` — A close was requested.

### `layer`

wlr-layer-shell control, for documents running with window.mode = "layer".

| | Method | Params → returns |
|---|---|---|
| → | `setAnchor` | `{ anchor: Anchor[] }` → `void` |
| → | `setExclusiveZone` | `{ zone: number }` → `void` |
| → | `setLayer` | `{ layer: LayerName }` → `void` |
| → | `setKeyboardInteractivity` | `{ mode: "none" \| "on-demand" \| "exclusive" }` → `void` |
| → | `setMargin` | `{ top?: number; right?: number; bottom?: number; left?: number }` → `void` |
| → | `setOutput` | `{ output: string }` → `void` |
| → | `listOutputs` | `void` → `OutputInfo[]` |

### `menu`

Application and context menus, defined as JSON and painted by GPUI.

| | Method | Params → returns |
|---|---|---|
| → | `setApplicationMenu` | `{ items: MenuItem[] }` → `void` |
| → | `popup` | `{ items: MenuItem[]; x?: number; y?: number }` → `string \| null` |

### `palette`

A native command palette overlay. Cheap in GPUI, and what makes an HTML App feel like an editor rather than a page in a frame.

| | Method | Params → returns |
|---|---|---|
| → | `register` | `{ commands: Command[] }` → `void` |
| → | `open` | `{ query?: string }` → `void` |
| → | `close` | `void` → `void` |

### `shortcut`

Global hotkeys, via the GlobalShortcuts portal on Wayland with an X11 grab fallback.

| | Method | Params → returns |
|---|---|---|
| → | `register` | `{ id: string; accelerator: string; description?: string }` → `void` |
| → | `unregister` | `{ id: string }` → `void` |
| → | `list` | `void` → `ShortcutInfo[]` |

### `clipboard`

Text, HTML, images, and file lists, both directions.

| | Method | Params → returns |
|---|---|---|
| → | `readText` | `void` → `string` |
| → | `writeText` | `{ text: string }` → `void` |
| → | `readHtml` | `void` → `string` |
| → | `writeHtml` | `{ html: string; text?: string }` → `void` |
| → | `readImage` | `void` → `string` |
| → | `writeImage` | `{ data: string }` → `void` |
| → | `readFiles` | `void` → `string[]` |
| → | `writeFiles` | `{ paths: string[] }` → `void` |

### `dnd`

Drag and drop of files in and out, including drag-out-to-file.

| | Method | Params → returns |
|---|---|---|

Events:

- `dnd:enter` → `{ paths: string[]; x: number; y: number }` — A drag entered the window.
- `dnd:over` → `{ x: number; y: number }` — A drag moved over the window.
- `dnd:drop` → `{ paths: string[]; x: number; y: number }` — Files were dropped.
- `dnd:leave` → `void` — A drag left the window.

## Tier 4 — reach

### `net`

Raw sockets and an embedded server, so an HTML App tool can be something other clients talk to.

| | Method | Params → returns |
|---|---|---|
| → | `connect` | `{ kind: "tcp" \| "udp" \| "unix"; address: string }` → `number` |
| → | `send` | `{ handle: number; data: string }` → `void` |
| → | `close` | `{ handle: number }` → `void` |
| → | `serve` | `{ kind: "http" \| "ws" \| "tcp"; address: string }` → `number` |
| → | `respond` | `{ requestId: number; status?: number; headers?: Record<string, string>; body?: string }` → `void` |
| ⇉ | `receive` | `{ handle: number }` → `string` |
| ⇉ | `accept` | `{ handle: number }` → `ServerEvent` |

### `serial`

Serial ports, for hardware and embedded tooling.

| | Method | Params → returns |
|---|---|---|
| → | `list` | `void` → `SerialPortInfo[]` |
| → | `open` | `{ port: string; baudRate?: number; dataBits?: number; parity?: string; stopBits?: number }` → `number` |
| → | `write` | `{ handle: number; data: string }` → `void` |
| → | `close` | `{ handle: number }` → `void` |
| ⇉ | `read` | `{ handle: number }` → `string` |

### `usb`

USB device access.

| | Method | Params → returns |
|---|---|---|
| → | `list` | `void` → `UsbDeviceInfo[]` |
| → | `open` | `{ vendorId: number; productId: number }` → `number` |
| → | `transfer` | `{ handle: number; endpoint: number; data?: string; length?: number }` → `string` |
| → | `close` | `{ handle: number }` → `void` |

### `bluetooth`

Bluetooth adapters and devices, via BlueZ.

| | Method | Params → returns |
|---|---|---|
| → | `adapters` | `void` → `BluetoothAdapter[]` |
| → | `connect` | `{ address: string }` → `void` |
| → | `disconnect` | `{ address: string }` → `void` |
| ⇉ | `discover` | `{ timeout?: number }` → `BluetoothDevice` |

### `os`

Platform, distro, XDG paths, battery, theme, locale, and idle time.

| | Method | Params → returns |
|---|---|---|
| → | `info` | `void` → `OsInfo` |
| → | `env` | `{ name: string }` → `string \| null` |
| → | `paths` | `void` → `XdgPaths` |
| → | `battery` | `void` → `BatteryInfo \| null` |
| → | `theme` | `void` → `ThemeInfo` |
| → | `locale` | `void` → `string` |
| → | `idleTime` | `void` → `number` |
| → | `uptime` | `void` → `number` |

Events:

- `os:theme` → `ThemeInfo` — The system colour scheme changed.
- `os:battery` → `BatteryInfo` — Battery state changed.

### `shell`

Hand things to the desktop: default handlers, the trash, the file manager.

| | Method | Params → returns |
|---|---|---|
| → | `open` | `{ target: string }` → `void` |
| → | `trash` | `{ path: string }` → `void` |
| → | `showInFileManager` | `{ path: string }` → `void` |

### `ffi`

Load a shared object and call into it. This is the escape hatch that voids the security model; prefer `plugin`.

| | Method | Params → returns |
|---|---|---|
| → | `open` | `{ library: string }` → `number` |
| → | `call` | `{ handle: number; symbol: string; args?: unknown[]; returns?: string }` → `unknown` |
| → | `close` | `{ handle: number }` → `void` |

### `plugin`

A wasmtime host for third-party extensions. Sandboxed, portable, and the recommended alternative to ffi for anything redistributable.

| | Method | Params → returns |
|---|---|---|
| → | `load` | `{ path: string }` → `number` |
| → | `call` | `{ handle: number; function: string; args?: unknown[] }` → `unknown` |
| → | `unload` | `{ handle: number }` → `void` |

---

## Events

Subscribe with `htmlapp.on(name, handler)`; it returns an unsubscribe function. Events are
namespaced `module:event` and the module gate applies to them, so subscribing to an ungranted
module's events does nothing.

Two events are ambient — available regardless of the manifest:

| Event | Payload | Meaning |
|---|---|---|
| `ready` | `void` | The bridge finished initialising. |
| `view:resize` | `{ id, width, height }` | A native view was resized. |

---

## Notes that will save you time

**`fs`** — `read` does not imply `write`. `rename` needs write on both ends. `glob` only reports
paths the manifest also covers, so it cannot be used to enumerate outside the grant. `list` filters
each entry individually rather than inheriting the parent's permission. Use `blob` for bulk
transfer and `mmap` for repeated windowed reads of one large file.

**`process`** — a bare name in `exec` matches only a bare name: granting `rg` does not grant
`/tmp/rg`. An empty `exec` list means unrestricted, and the consent sheet says so in those words.
`pty` needs its own flag. You can only signal processes your own document started.

**`dialog`** — `open`, `save`, and `pickFolder` go through `xdg-desktop-portal`, so the picker is
the system's own and whatever the user chooses becomes readable regardless of the manifest's globs.
`message`, `confirm`, and `prompt` are painted by the host and are modal to the document's window.

**`http`** — the origin allow-list is enforced per request **and per redirect hop**, so a permitted
origin cannot bounce you somewhere else. Wildcard origins fail at manifest parse time.

**`sql`** — one connection thread per database, so two concurrent calls cannot interleave inside a
transaction. `stream` collects on the worker thread and then yields row by row: it gives you a
streaming *interface*, not lower peak memory.

**`dbus`** — `call` marshals arguments as strings. Inferring a D-Bus signature from JSON would mean
guessing, and sending the wrong type to a system service does not fail gracefully. `set` infers from
the JSON shape for the four unambiguous cases and refuses the rest. `ownName` is gated on
`dbus.own`, separately from `destinations`.

**`net`** — `serve` gives you an HTTP or WebSocket server. An inbound request waits up to 30s for
`respond`; after that the peer gets a 503 rather than hanging. Bodies are capped at 8 MB and
WebSocket frames at 4 MB.

**`portal`** — each interface is a separate grant. `screenCast` returns `unsupported`: it hands back
a PipeWire node id, and consuming that needs the client that ships with the `video` native view.

**`ffi`** — the escape hatch that voids the model. Two calling shapes are supported: up to six
integer/pointer arguments, or up to four floating-point ones. A *mixed* signature cannot be
expressed, because integers and floats travel in different register files. Prefer `plugin`.

**`plugin`** — WebAssembly modules get **no imports at all**, so a module needing WASI will not
load. Each call is refuelled, so an infinite loop is a trapped error rather than a hung document.

---

## Unimplemented, and honest about it

| | Status |
|---|---|
| `portal.screenCast` | Needs the PipeWire consumer that ships with the `video` view |
| `dnd.startDrag` | Files are prepared and placed on the clipboard; the XDND *source* protocol is not implemented, so the OS-level drag gesture does not start |
| `editor`, `video`, `canvas3d` views | `view.create` reports `unsupported` |

Everything else in the tables above is implemented and reachable.

---

[← The bridge](bridge.md) · **API reference** · [Security model →](security.md)
