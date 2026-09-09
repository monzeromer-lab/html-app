//! The API catalog (PRD §9.3), as data.
//!
//! Declaring the surface once, here, means the JS shim and the TypeScript declarations emitted by
//! `htmlapp types` cannot drift apart — §9.2 promises single-file authors completion in their
//! editor without a build step, and that promise is only as good as the two staying in sync.
//!
//! The tiers are a priority order, not a wish list (§16 R8).

/// §9.3 groups the surface into four tiers by how central it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// The core: fs, process, dialog, http, sql, store.
    Core = 1,
    /// Linux desktop integration: portal, dbus, tray, notify, secrets, systemd, udev.
    Desktop = 2,
    /// Windowing and shell: window, layer, menu, palette, shortcut, clipboard, dnd.
    Shell = 3,
    /// Reach: sockets, devices, os, shell, ffi, plugin.
    Reach = 4,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Core => "Tier 1 — the core",
            Tier::Desktop => "Tier 2 — Linux desktop integration",
            Tier::Shell => "Tier 3 — windowing and shell",
            Tier::Reach => "Tier 4 — reach",
        }
    }
}

/// Which of the three §9.1 message shapes a method uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    /// `await htmlapp.invoke(...)` — resolves once.
    Invoke,
    /// `for await (... of htmlapp.stream(...))` — yields many, then ends.
    Stream,
}

#[derive(Debug, Clone, Copy)]
pub struct ApiMethod {
    pub name: &'static str,
    pub kind: MethodKind,
    /// TypeScript type of the single params object, or `"void"` for no arguments.
    pub params: &'static str,
    /// TypeScript type of the resolved value, or of one stream chunk.
    pub returns: &'static str,
    pub summary: &'static str,
}

/// An event delivered through `htmlapp.on(...)` (§9.1).
#[derive(Debug, Clone, Copy)]
pub struct ApiEvent {
    pub name: &'static str,
    pub payload: &'static str,
    pub summary: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ApiModule {
    /// The property name on the `htmlapp` global.
    pub name: &'static str,
    pub tier: Tier,
    pub summary: &'static str,
    pub methods: &'static [ApiMethod],
    pub events: &'static [ApiEvent],
}

const fn invoke(
    name: &'static str,
    params: &'static str,
    returns: &'static str,
    summary: &'static str,
) -> ApiMethod {
    ApiMethod { name, kind: MethodKind::Invoke, params, returns, summary }
}

const fn stream(
    name: &'static str,
    params: &'static str,
    returns: &'static str,
    summary: &'static str,
) -> ApiMethod {
    ApiMethod { name, kind: MethodKind::Stream, params, returns, summary }
}

// ---------------------------------------------------------------------------
// Tier 1 — the core
// ---------------------------------------------------------------------------

const FS: ApiModule = ApiModule {
    name: "fs",
    tier: Tier::Core,
    summary: "Filesystem access, scoped to the manifest's read and write globs. Every path is \
              resolved through symlinks before it is checked.",
    methods: &[
        invoke("read", "{ path: string; encoding?: \"utf8\" | \"binary\" }", "string", "Read a whole file."),
        invoke("write", "{ path: string; contents: string; encoding?: \"utf8\" | \"binary\" }", "void", "Replace a file's contents."),
        invoke("append", "{ path: string; contents: string }", "void", "Append to a file."),
        invoke("stat", "{ path: string }", "FileStat", "Metadata for one path."),
        invoke("list", "{ path: string }", "DirEntry[]", "List a directory."),
        invoke("glob", "{ pattern: string }", "string[]", "Expand a glob within the granted scope."),
        invoke("mkdir", "{ path: string; recursive?: boolean }", "void", "Create a directory."),
        invoke("remove", "{ path: string; recursive?: boolean }", "void", "Delete a file or directory."),
        invoke("rename", "{ from: string; to: string }", "void", "Rename or move."),
        invoke("copy", "{ from: string; to: string }", "void", "Copy a file."),
        invoke("blob", "{ path: string }", "string", "Mint a blob: URL the page can fetch directly, so bulk bytes never pass through JSON."),
        stream("readStream", "{ path: string; chunkSize?: number }", "string", "Read a file in chunks."),
        stream("tail", "{ path: string; lines?: number }", "string", "Follow a file as it grows."),
        stream("watch", "{ path: string; recursive?: boolean }", "WatchEvent", "inotify watches over the granted scope."),
    ],
    events: &[],
};

const PROCESS: ApiModule = ApiModule {
    name: "process",
    tier: Tier::Core,
    summary: "Run other programs, restricted to the manifest's executable allow-list.",
    methods: &[
        invoke("exec", "{ program: string; args?: string[]; cwd?: string; env?: Record<string, string>; stdin?: string }", "ProcessOutput", "Run a program to completion."),
        invoke("kill", "{ pid: number }", "void", "Terminate a process this document started."),
        invoke("signal", "{ pid: number; signal: string }", "void", "Send a signal."),
        invoke("list", "void", "ProcessInfo[]", "Processes this document started."),
        invoke("write", "{ pid: number; data: string }", "void", "Write to a running process's stdin."),
        invoke("resize", "{ pid: number; cols: number; rows: number }", "void", "Resize a PTY."),
        stream("spawn", "{ program: string; args?: string[]; cwd?: string; env?: Record<string, string> }", "ProcessEvent", "Run a program, streaming its output."),
        stream("pty", "{ program: string; args?: string[]; cwd?: string; cols?: number; rows?: number }", "ProcessEvent", "Run a program under a pseudo-terminal."),
    ],
    events: &[],
};

const DIALOG: ApiModule = ApiModule {
    name: "dialog",
    tier: Tier::Core,
    summary: "The desktop's own file pickers, via xdg-desktop-portal. Files chosen here arrive \
              pre-authorised, outside the manifest's globs.",
    methods: &[
        invoke("open", "{ title?: string; multiple?: boolean; filters?: FileFilter[]; startIn?: string }", "string[]", "Pick one or more files."),
        invoke("save", "{ title?: string; defaultName?: string; filters?: FileFilter[] }", "string | null", "Pick a save destination."),
        invoke("pickFolder", "{ title?: string }", "string | null", "Pick a directory."),
        invoke("message", "{ title?: string; body: string }", "void", "Show a message."),
        invoke("confirm", "{ title?: string; body: string }", "boolean", "Ask a yes/no question."),
        invoke("prompt", "{ title?: string; body: string; default?: string }", "string | null", "Ask for a line of text."),
    ],
    events: &[],
};

const HTTP: ApiModule = ApiModule {
    name: "http",
    tier: Tier::Core,
    summary: "A fetch that ignores CORS, restricted to the manifest's origin allow-list.",
    methods: &[
        invoke("fetch", "HttpRequest", "HttpResponse", "Perform a request."),
        stream("stream", "HttpRequest", "string", "Perform a request, streaming the response body."),
    ],
    events: &[],
};

const SQL: ApiModule = ApiModule {
    name: "sql",
    tier: Tier::Core,
    summary: "Bundled SQLite, restricted to the databases named in the manifest.",
    methods: &[
        invoke("open", "{ database: string }", "void", "Open a database."),
        invoke("execute", "{ database: string; sql: string; params?: SqlValue[] }", "SqlExecuteResult", "Run a statement."),
        invoke("query", "{ database: string; sql: string; params?: SqlValue[] }", "SqlRow[]", "Run a query and collect every row."),
        invoke("transaction", "{ database: string; statements: SqlStatement[] }", "void", "Run statements atomically."),
        invoke("close", "{ database: string }", "void", "Close a database."),
        stream("stream", "{ database: string; sql: string; params?: SqlValue[] }", "SqlRow", "Run a query, streaming rows."),
    ],
    events: &[],
};

const STORE: ApiModule = ApiModule {
    name: "store",
    tier: Tier::Core,
    summary: "A persistent key-value store scoped to this app's id.",
    methods: &[
        invoke("get", "{ key: string }", "unknown", "Read a value."),
        invoke("set", "{ key: string; value: unknown }", "void", "Write a value."),
        invoke("delete", "{ key: string }", "void", "Remove a key."),
        invoke("keys", "void", "string[]", "List every key."),
        invoke("clear", "void", "void", "Remove everything."),
    ],
    events: &[],
};

// ---------------------------------------------------------------------------
// Tier 2 — Linux desktop integration
// ---------------------------------------------------------------------------

const PORTAL: ApiModule = ApiModule {
    name: "portal",
    tier: Tier::Desktop,
    summary: "xdg-desktop-portal. The prompts the user sees are their own desktop's, and Flatpak \
              confinement comes free.",
    methods: &[
        invoke("screenshot", "{ interactive?: boolean }", "string", "Take a screenshot; returns a file URI."),
        invoke("pickColor", "void", "Color", "Pick a colour from the screen."),
        invoke("openUri", "{ uri: string; writable?: boolean }", "void", "Hand a URI to the user's default handler."),
        invoke("inhibit", "{ reason: string; flags?: string[] }", "number", "Inhibit idle, logout, or suspend."),
        invoke("uninhibit", "{ handle: number }", "void", "Release an inhibitor."),
        invoke("requestBackground", "{ reason: string; autostart?: boolean }", "boolean", "Ask to keep running in the background."),
        invoke("setWallpaper", "{ uri: string; target?: \"background\" | \"lockscreen\" | \"both\" }", "void", "Set the desktop wallpaper."),
        invoke("location", "void", "Location", "One location fix."),
        stream("screenCast", "{ multiple?: boolean; cursor?: \"hidden\" | \"embedded\" | \"metadata\" }", "ScreenCastFrame", "Capture the screen over PipeWire."),
    ],
    events: &[],
};

const DBUS: ApiModule = ApiModule {
    name: "dbus",
    tier: Tier::Desktop,
    summary: "Session and system bus. The highest-leverage API on Linux: NetworkManager, UPower, \
              logind, BlueZ, and MPRIS without wrapping each one.",
    methods: &[
        invoke("call", "DbusCall", "unknown", "Call a method on the bus."),
        invoke("get", "{ bus?: DbusBus; destination: string; path: string; iface: string; property: string }", "unknown", "Read a property."),
        invoke("set", "{ bus?: DbusBus; destination: string; path: string; iface: string; property: string; value: unknown }", "void", "Write a property."),
        invoke("ownName", "{ bus?: DbusBus; name: string }", "void", "Take a well-known bus name."),
        invoke("introspect", "{ bus?: DbusBus; destination: string; path: string }", "string", "Introspect an object."),
        stream("subscribe", "{ bus?: DbusBus; destination?: string; path?: string; iface?: string; member?: string }", "DbusSignal", "Receive matching signals."),
    ],
    events: &[],
};

const TRAY: ApiModule = ApiModule {
    name: "tray",
    tier: Tier::Desktop,
    summary: "StatusNotifierItem, via ksni.",
    methods: &[
        invoke("set", "{ icon?: string; title?: string; tooltip?: string; menu?: MenuItem[] }", "void", "Create or update the tray item."),
        invoke("remove", "void", "void", "Remove the tray item."),
    ],
    events: &[
        ApiEvent { name: "tray:activate", payload: "{ x: number; y: number }", summary: "The tray icon was clicked." },
        ApiEvent { name: "tray:menu", payload: "{ id: string }", summary: "A tray menu item was chosen." },
    ],
};

const NOTIFY: ApiModule = ApiModule {
    name: "notify",
    tier: Tier::Desktop,
    summary: "Desktop notifications with actions, hints, replace-id, and progress.",
    methods: &[
        invoke("send", "NotificationSpec", "number", "Post a notification; returns its id."),
        invoke("close", "{ id: number }", "void", "Withdraw a notification."),
    ],
    events: &[
        ApiEvent { name: "notify:action", payload: "{ id: number; action: string }", summary: "The user chose a notification action." },
        ApiEvent { name: "notify:closed", payload: "{ id: number; reason: string }", summary: "A notification was dismissed." },
    ],
};

const SECRETS: ApiModule = ApiModule {
    name: "secrets",
    tier: Tier::Desktop,
    summary: "Secret Service. Credentials never touch the page's own storage.",
    methods: &[
        invoke("get", "{ key: string }", "string | null", "Read a secret."),
        invoke("set", "{ key: string; value: string; label?: string }", "void", "Store a secret."),
        invoke("delete", "{ key: string }", "void", "Remove a secret."),
        invoke("search", "{ attributes: Record<string, string> }", "string[]", "Find secrets by attribute."),
    ],
    events: &[],
};

const SYSTEMD: ApiModule = ApiModule {
    name: "systemd",
    tier: Tier::Desktop,
    summary: "Journal reading and user unit control.",
    methods: &[
        invoke("status", "{ unit: string }", "UnitStatus", "Status of a user unit."),
        invoke("start", "{ unit: string }", "void", "Start a user unit."),
        invoke("stop", "{ unit: string }", "void", "Stop a user unit."),
        invoke("restart", "{ unit: string }", "void", "Restart a user unit."),
        invoke("run", "{ program: string; args?: string[]; properties?: Record<string, string> }", "string", "Run a scoped transient unit."),
        stream("journal", "{ unit?: string; since?: string; priority?: number; follow?: boolean }", "JournalEntry", "Read the journal, optionally following."),
    ],
    events: &[],
};

const UDEV: ApiModule = ApiModule {
    name: "udev",
    tier: Tier::Desktop,
    summary: "Device enumeration and hotplug events.",
    methods: &[
        invoke("list", "{ subsystem?: string }", "DeviceInfo[]", "Enumerate devices."),
        stream("monitor", "{ subsystem?: string }", "DeviceEvent", "Watch for hotplug events."),
    ],
    events: &[],
};

// ---------------------------------------------------------------------------
// Tier 3 — windowing and shell
// ---------------------------------------------------------------------------

const WINDOW: ApiModule = ApiModule {
    name: "window",
    tier: Tier::Shell,
    summary: "Control this document's own window.",
    methods: &[
        invoke("setTitle", "{ title: string }", "void", "Set the window title."),
        invoke("resize", "{ width: number; height: number }", "void", "Resize the window."),
        invoke("move", "{ x: number; y: number }", "void", "Move the window."),
        invoke("fullscreen", "{ enabled: boolean }", "void", "Enter or leave fullscreen."),
        invoke("minimize", "void", "void", "Minimise the window."),
        invoke("maximize", "{ enabled?: boolean }", "void", "Maximise or restore."),
        invoke("setOpacity", "{ opacity: number }", "void", "Set window opacity."),
        invoke("setInputRegion", "{ rects: Rect[] | null }", "void", "Restrict where clicks land, for click-through overlays."),
        invoke("setAlwaysOnTop", "{ enabled: boolean }", "void", "Keep the window above others."),
        invoke("open", "{ path?: string; title?: string; width?: number; height?: number }", "number", "Open another window from this document."),
        invoke("close", "void", "void", "Close the window."),
    ],
    events: &[
        ApiEvent { name: "window:resize", payload: "{ width: number; height: number; scale: number }", summary: "The window was resized." },
        ApiEvent { name: "window:focus", payload: "{ focused: boolean }", summary: "Focus entered or left the window." },
        ApiEvent { name: "window:state", payload: "{ maximized: boolean; fullscreen: boolean; minimized: boolean }", summary: "The window state changed." },
        ApiEvent { name: "window:close", payload: "void", summary: "A close was requested." },
    ],
};

const LAYER: ApiModule = ApiModule {
    name: "layer",
    tier: Tier::Shell,
    summary: "wlr-layer-shell control, for documents running with window.mode = \"layer\".",
    methods: &[
        invoke("setAnchor", "{ anchor: Anchor[] }", "void", "Set which edges the surface anchors to."),
        invoke("setExclusiveZone", "{ zone: number }", "void", "Reserve space other windows will not cover."),
        invoke("setLayer", "{ layer: LayerName }", "void", "Move between layers."),
        invoke("setKeyboardInteractivity", "{ mode: \"none\" | \"on-demand\" | \"exclusive\" }", "void", "Set how the surface takes keyboard focus."),
        invoke("setMargin", "{ top?: number; right?: number; bottom?: number; left?: number }", "void", "Set surface margins."),
        invoke("setOutput", "{ output: string }", "void", "Move to a named output."),
        invoke("listOutputs", "void", "OutputInfo[]", "Enumerate outputs."),
    ],
    events: &[ApiEvent {
        name: "layer:outputs",
        payload: "OutputInfo[]",
        summary: "The set of outputs changed.",
    }],
};

const MENU: ApiModule = ApiModule {
    name: "menu",
    tier: Tier::Shell,
    summary: "Application and context menus, defined as JSON and painted by GPUI.",
    methods: &[
        invoke("setApplicationMenu", "{ items: MenuItem[] }", "void", "Set the application menu bar."),
        invoke("popup", "{ items: MenuItem[]; x?: number; y?: number }", "string | null", "Show a context menu; resolves with the chosen id."),
    ],
    events: &[ApiEvent {
        name: "menu:select",
        payload: "{ id: string }",
        summary: "An application menu item was chosen.",
    }],
};

const PALETTE: ApiModule = ApiModule {
    name: "palette",
    tier: Tier::Shell,
    summary: "A native command palette overlay. Cheap in GPUI, and what makes an HTML App feel \
              like an editor rather than a page in a frame.",
    methods: &[
        invoke("register", "{ commands: Command[] }", "void", "Replace the command list."),
        invoke("open", "{ query?: string }", "void", "Open the palette."),
        invoke("close", "void", "void", "Close the palette."),
    ],
    events: &[ApiEvent {
        name: "palette:run",
        payload: "{ id: string }",
        summary: "A command was invoked from the palette.",
    }],
};

const SHORTCUT: ApiModule = ApiModule {
    name: "shortcut",
    tier: Tier::Shell,
    summary: "Global hotkeys, via the GlobalShortcuts portal on Wayland with an X11 grab fallback.",
    methods: &[
        invoke("register", "{ id: string; accelerator: string; description?: string }", "void", "Bind a global hotkey."),
        invoke("unregister", "{ id: string }", "void", "Release a hotkey."),
        invoke("list", "void", "ShortcutInfo[]", "List bound hotkeys."),
    ],
    events: &[ApiEvent {
        name: "shortcut:trigger",
        payload: "{ id: string }",
        summary: "A registered hotkey fired.",
    }],
};

const CLIPBOARD: ApiModule = ApiModule {
    name: "clipboard",
    tier: Tier::Shell,
    summary: "Text, HTML, images, and file lists, both directions.",
    methods: &[
        invoke("readText", "void", "string", "Read the clipboard as text."),
        invoke("writeText", "{ text: string }", "void", "Put text on the clipboard."),
        invoke("readHtml", "void", "string", "Read the clipboard as HTML."),
        invoke("writeHtml", "{ html: string; text?: string }", "void", "Put HTML on the clipboard."),
        invoke("readImage", "void", "string", "Read an image as a data URI."),
        invoke("writeImage", "{ data: string }", "void", "Put an image on the clipboard."),
        invoke("readFiles", "void", "string[]", "Read a copied file list."),
        invoke("writeFiles", "{ paths: string[] }", "void", "Put a file list on the clipboard."),
    ],
    events: &[ApiEvent {
        name: "clipboard:change",
        payload: "{ formats: string[] }",
        summary: "The clipboard contents changed.",
    }],
};

const DND: ApiModule = ApiModule {
    name: "dnd",
    tier: Tier::Shell,
    summary: "Drag and drop of files in and out, including drag-out-to-file.",
    methods: &[invoke(
        "startDrag",
        "{ paths?: string[]; data?: { name: string; contents: string } }",
        "void",
        "Begin dragging files out of this window.",
    )],
    events: &[
        ApiEvent { name: "dnd:enter", payload: "{ paths: string[]; x: number; y: number }", summary: "A drag entered the window." },
        ApiEvent { name: "dnd:over", payload: "{ x: number; y: number }", summary: "A drag moved over the window." },
        ApiEvent { name: "dnd:drop", payload: "{ paths: string[]; x: number; y: number }", summary: "Files were dropped." },
        ApiEvent { name: "dnd:leave", payload: "void", summary: "A drag left the window." },
    ],
};

// ---------------------------------------------------------------------------
// Tier 4 — reach
// ---------------------------------------------------------------------------

const NET: ApiModule = ApiModule {
    name: "net",
    tier: Tier::Reach,
    summary: "Raw sockets and an embedded server, so an HTML App tool can be something other \
              clients talk to.",
    methods: &[
        invoke("connect", "{ kind: \"tcp\" | \"udp\" | \"unix\"; address: string }", "number", "Open a socket; returns a handle."),
        invoke("send", "{ handle: number; data: string }", "void", "Write to a socket."),
        invoke("close", "{ handle: number }", "void", "Close a socket or server."),
        invoke("serve", "{ kind: \"http\" | \"ws\" | \"tcp\"; address: string }", "number", "Start a server; returns a handle."),
        invoke("respond", "{ requestId: number; status?: number; headers?: Record<string, string>; body?: string }", "void", "Answer an inbound HTTP request."),
        stream("receive", "{ handle: number }", "string", "Read from a socket."),
        stream("accept", "{ handle: number }", "ServerEvent", "Accept inbound connections and requests."),
    ],
    events: &[],
};

const SERIAL: ApiModule = ApiModule {
    name: "serial",
    tier: Tier::Reach,
    summary: "Serial ports, for hardware and embedded tooling.",
    methods: &[
        invoke("list", "void", "SerialPortInfo[]", "Enumerate serial ports."),
        invoke("open", "{ port: string; baudRate?: number; dataBits?: number; parity?: string; stopBits?: number }", "number", "Open a port."),
        invoke("write", "{ handle: number; data: string }", "void", "Write to a port."),
        invoke("close", "{ handle: number }", "void", "Close a port."),
        stream("read", "{ handle: number }", "string", "Read from a port."),
    ],
    events: &[],
};

const USB: ApiModule = ApiModule {
    name: "usb",
    tier: Tier::Reach,
    summary: "USB device access.",
    methods: &[
        invoke("list", "void", "UsbDeviceInfo[]", "Enumerate USB devices."),
        invoke("open", "{ vendorId: number; productId: number }", "number", "Open a device."),
        invoke("transfer", "{ handle: number; endpoint: number; data?: string; length?: number }", "string", "Perform a transfer."),
        invoke("close", "{ handle: number }", "void", "Close a device."),
    ],
    events: &[],
};

const BLUETOOTH: ApiModule = ApiModule {
    name: "bluetooth",
    tier: Tier::Reach,
    summary: "Bluetooth adapters and devices, via BlueZ.",
    methods: &[
        invoke("adapters", "void", "BluetoothAdapter[]", "List adapters."),
        invoke("connect", "{ address: string }", "void", "Connect to a device."),
        invoke("disconnect", "{ address: string }", "void", "Disconnect."),
        stream("discover", "{ timeout?: number }", "BluetoothDevice", "Scan for devices."),
    ],
    events: &[],
};

const OS: ApiModule = ApiModule {
    name: "os",
    tier: Tier::Reach,
    summary: "Platform, distro, XDG paths, battery, theme, locale, and idle time.",
    methods: &[
        invoke("info", "void", "OsInfo", "Platform, distro, kernel, and arch."),
        invoke("env", "{ name: string }", "string | null", "Read an environment variable."),
        invoke("paths", "void", "XdgPaths", "The XDG base directories."),
        invoke("battery", "void", "BatteryInfo | null", "Battery state, if any."),
        invoke("theme", "void", "ThemeInfo", "The current colour scheme and accent."),
        invoke("locale", "void", "string", "The active locale."),
        invoke("idleTime", "void", "number", "Seconds since the last input."),
        invoke("uptime", "void", "number", "Seconds since boot."),
    ],
    events: &[
        ApiEvent { name: "os:theme", payload: "ThemeInfo", summary: "The system colour scheme changed." },
        ApiEvent { name: "os:battery", payload: "BatteryInfo", summary: "Battery state changed." },
    ],
};

const SHELL: ApiModule = ApiModule {
    name: "shell",
    tier: Tier::Reach,
    summary: "Hand things to the desktop: default handlers, the trash, the file manager.",
    methods: &[
        invoke("open", "{ target: string }", "void", "Open a URL or path in the user's default handler."),
        invoke("trash", "{ path: string }", "void", "Move a file to the trash."),
        invoke("showInFileManager", "{ path: string }", "void", "Reveal a path in the file manager."),
    ],
    events: &[],
};

const FFI: ApiModule = ApiModule {
    name: "ffi",
    tier: Tier::Reach,
    summary: "Load a shared object and call into it. This is the escape hatch that voids the \
              security model; prefer `plugin`.",
    methods: &[
        invoke("open", "{ library: string }", "number", "dlopen a shared library."),
        invoke("call", "{ handle: number; symbol: string; args?: unknown[]; returns?: string }", "unknown", "Call a symbol."),
        invoke("close", "{ handle: number }", "void", "Close a library."),
    ],
    events: &[],
};

const PLUGIN: ApiModule = ApiModule {
    name: "plugin",
    tier: Tier::Reach,
    summary: "A wasmtime host for third-party extensions. Sandboxed, portable, and the \
              recommended alternative to ffi for anything redistributable.",
    methods: &[
        invoke("load", "{ path: string }", "number", "Load a WebAssembly module."),
        invoke("call", "{ handle: number; function: string; args?: unknown[] }", "unknown", "Call an exported function."),
        invoke("unload", "{ handle: number }", "void", "Unload a module."),
    ],
    events: &[],
};

// ---------------------------------------------------------------------------

/// Every module in the §9.3 catalog, in tier order.
pub const MODULES: &[ApiModule] = &[
    FS, PROCESS, DIALOG, HTTP, SQL, STORE,
    PORTAL, DBUS, TRAY, NOTIFY, SECRETS, SYSTEMD, UDEV,
    WINDOW, LAYER, MENU, PALETTE, SHORTCUT, CLIPBOARD, DND,
    NET, SERIAL, USB, BLUETOOTH, OS, SHELL, FFI, PLUGIN,
];

/// Look up a module by its property name on `htmlapp`.
pub fn module(name: &str) -> Option<&'static ApiModule> {
    MODULES.iter().find(|m| m.name == name)
}

/// Look up one method by its fully-qualified `module.method` name.
pub fn method(qualified: &str) -> Option<(&'static ApiModule, &'static ApiMethod)> {
    let (module_name, method_name) = qualified.split_once('.')?;
    let module = module(module_name)?;
    let method = module.methods.iter().find(|m| m.name == method_name)?;
    Some((module, method))
}

/// Events that are always available, regardless of what the manifest granted.
pub const AMBIENT_EVENTS: &[ApiEvent] = &[
    ApiEvent { name: "ready", payload: "void", summary: "The bridge finished initialising." },
    ApiEvent { name: "view:resize", payload: "{ id: string; width: number; height: number }", summary: "A native view was resized." },
];
