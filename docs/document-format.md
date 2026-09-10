# The document format

[← Docs index](README.md) · [Getting started](getting-started.md) · **Document format** ·
[Bridge](bridge.md) · [API](api-reference.md) · [Security](security.md) ·
[Architecture](architecture.md) · [Building](building.md) · [Contributing](contributing.md) ·
[AGENT.md](AGENT.md)

---

## Design principle

Configuration lives **in the document**. A `.hta` is one file, and everything about how it runs —
its window, its permissions, its imports — travels with it.

A `<meta>` tag is too weak to carry a permission list, so the manifest is a typed script block:

```html
<script type="application/htmlapp+json"> … </script>
```

Two properties follow, and both are load-bearing:

- **The host parses it before the engine sees the file.** The permission decision is made against
  bytes on disk, not against something a running page produced.
- **It is inert to browsers.** Renaming a `.hta` to `.html` opens it in Firefox, without native
  APIs. Browser devtools stay usable while you write, which is the whole authoring loop.

The host uses a small purpose-built scanner rather than an HTML tree builder, so the security
decision does not depend on a parser the engine never sees. Anything it cannot read confidently is
treated as "no manifest", which is the safe outcome.

---

## Manifest

Every field is optional. A document with no manifest at all is valid — it runs as a page in a
window with no native APIs.

```html
<!DOCTYPE html>
<html>
<head>
<script type="application/htmlapp+json">
{
  "name": "Log Triage",
  "id": "dev.example.logtriage",
  "version": "1.2.0",
  "icon": "data:image/svg+xml;base64,…",

  "window": {
    "title": "Log Triage",
    "width": 1200, "height": 800,
    "min_width": 640,
    "titlebar": "native",
    "background": "transparent",
    "resizable": true
  },

  "permissions": {
    "fs":      { "read": ["~/logs/**", "/var/log/nginx/*.log"],
                 "write": ["~/.local/share/logtriage/**"] },
    "process": { "exec": ["rg", "journalctl"], "pty": false },
    "net":     { "fetch": ["https://alerts.internal.corp/*"] },
    "sql":     { "databases": ["~/.local/share/logtriage/index.db"] },
    "clipboard": ["read", "write"],
    "notifications": true
  }
}
</script>
</head>
<body>…</body>
</html>
```

### Identity

| Field | Type | Notes |
|---|---|---|
| `name` | string | Shown in the titlebar, the launcher's recents, and the consent sheet. |
| `id` | string | Reverse-DNS. Scopes `store`, `secrets`, and the built `.desktop`. |
| `version` | string | Informational. |
| `icon` | string | A `data:` URI. Extracted by `htmlapp build`; a remote URL is **not** fetched. |

`id` becomes a filesystem path component, so it is validated rather than trusted: ASCII
alphanumerics, `.`, `-`, `_` only, and no leading, trailing, or consecutive dots. A document with no
`id` gets storage scoped to `unnamed-<short hash>`, so it still has a private namespace.

### Window

| Field | Default | Notes |
|---|---|---|
| `title` | the `name` | |
| `width` / `height` | `1024` × `768` | Logical pixels. |
| `min_width` / `min_height` | none | |
| `titlebar` | `"native"` | `"none"` lets the page draw its own. |
| `background` | `"opaque"` | `"transparent"` needs an alpha-capable surface. |
| `resizable` | `true` | |
| `mode` | `"window"` | See [Window modes](#window-modes). |

### Assets

```json
{ "assets": "sibling" }
```

The default is `"none"`: the document is the whole app, and nothing else on disk is served over its
origin. `"sibling"` makes files in the document's own directory resolve relative to it. Traversal
out of that directory is refused, including through a symlink.

### Imports

```json
{ "imports": {
    "d3": { "url": "https://esm.sh/d3@7", "integrity": "sha384-…" }
} }
```

Remote ES modules, fetched once and cached under `~/.cache/htmlapp/modules/`, then served from the
document's own origin forever after. This keeps the zero-build promise while letting a single file
use real libraries.

Rules, all enforced:

- **`integrity` is required.** A manifest with an import that has no hash fails to parse. An
  unpinned import is a supply-chain hole.
- **`sha256`, `sha384`, or `sha512`.** Anything else is refused.
- **`https` only.** Modules are code.
- **Never revalidated.** A cached module is the module, permanently. Revalidation would make the
  same file behave differently on different days. `htmlapp cache purge` is the explicit escape.
- **Served from `htmlapp://app/__modules__/…`**, so the page never reaches the network for them and
  `connect-src` stays pinned to what the manifest declared.

### CSP

```json
{ "csp": "default-src 'self'; script-src 'self'" }
```

The runtime injects a restrictive policy unless the manifest overrides it. Overriding is a
deliberate act, recorded in the document. See
[Security → Content Security Policy](security.md#content-security-policy) for what the default
does and, more importantly, what it deliberately does not try to do.

---

## Window modes

`window.mode` decides what kind of surface the document becomes.

| Mode | Surface | Use |
|---|---|---|
| `window` *(default)* | `xdg_toplevel` | An ordinary application |
| `layer` | `wlr-layer-shell` | Bars, docks, widgets, overlays, launchers |
| `lock` | overlay layer, exclusive focus | Lock screens |
| `tray` | none until clicked | Applets and menu-bar tools |
| `headless` | none | CLI filters, `stdin` → `stdout` |

### Layer mode

```json
{ "window": {
    "mode": "layer",
    "layer": "top",
    "anchor": ["top", "left", "right"],
    "exclusive_zone": 34,
    "keyboard_interactivity": "on-demand",
    "output": "primary",
    "margin": { "top": 4, "left": 8 }
} }
```

| Field | Values |
|---|---|
| `layer` | `background`, `bottom`, `top` *(default)*, `overlay` |
| `anchor` | any of `top`, `bottom`, `left`, `right` |
| `exclusive_zone` | pixels of space to reserve so other windows do not cover the surface |
| `keyboard_interactivity` | `none` *(default)*, `on-demand`, `exclusive` |
| `output` | an output name, or `"primary"` |
| `margin` | per-edge offsets |

A surface anchored to opposite edges is sized by the compositor along that axis; the manifest's
dimension only applies to the axis it is free on.

Layer mode needs a build with the `layer-shell` feature and a compositor that implements
`wlr-layer-shell` (wlroots, KWin, Hyprland, Sway, Niri — not GNOME). Without it the runtime refuses
with a message that distinguishes the two causes, because only one of them is something you can act
on.

**Layer mode has no GPUI scene**, so `menu`, `palette`, `dialog`, and native views report
unsupported. For a bar, that is the right trade: the page is the whole UI.

### Lock mode

`mode: "lock"` gets an overlay-layer surface with exclusive keyboard focus.

> **This is not a security boundary.** True `ext-session-lock` is a different protocol, and it is
> what actually survives a compositor crash or a VT switch. An overlay does not. Use this for the
> *shape* of a lock screen; do not rely on it to keep anyone out. The runtime logs a warning saying
> so on every launch.

### Headless mode

No surface. `htmlapp.stdin`, `htmlapp.stdout`, `htmlapp.stderr`, and `htmlapp.exit()` are
available, plus `htmlapp.format` carrying `--format`. See [The bridge](bridge.md#headless-mode).

A headless document that needs permissions cannot be asked — there is no window to attach a sheet
to. It runs powerless and says so. Grant it deliberately instead:

```sh
htmlapp permissions allow report.hta
```

---

## Origin and loading

The document is served over **`htmlapp://app/`**, not `file://`.

That matters more than it looks. A `file://` origin is opaque, so `localStorage`, IndexedDB, service
workers, ES module imports, and a meaningful CSP all silently degrade or fail outright. A real,
stable origin makes all of them work.

| Path | Serves |
|---|---|
| `/` and `/index.html` | the document |
| `/__modules__/<hash>.js` | a cached pinned import |
| `/__blob__/<token>` | a file minted by `fs.blob` |
| anything else | a sibling asset, if `"assets": "sibling"`; otherwise `403` |

Off-origin navigation is blocked unless the manifest's `net.fetch` covers it — this is a runtime,
not a browser. Wildcard subdomains match at a label boundary, so `*.example.com` covers
`api.example.com` but not `notexample.com` and not the bare apex.

---

## The full permission vocabulary

Each key gates one module. Absent means the module does not exist on `htmlapp` at all.

```json
{
  "permissions": {
    "fs":        { "read": ["glob"], "write": ["glob"], "watch": true },
    "process":   { "exec": ["program"], "pty": false },
    "dialog":    true,
    "net":       { "fetch": ["https://host/path*"] },
    "sql":       { "databases": ["path"] },
    "store":     true,

    "portal":    ["screenshot", "screenCast", "globalShortcuts", "inhibit",
                  "openUri", "background", "notification", "wallpaper", "location",
                  "fileChooser"],
    "dbus":      { "session": true, "system": false,
                   "destinations": ["org.freedesktop.UPower"], "own": ["com.example.Me"] },
    "tray":          true,
    "notifications": true,
    "secrets":   true,
    "systemd":   true,
    "udev":      true,

    "window":    true,
    "layer":     true,
    "menu":      true,
    "palette":   true,
    "shortcut":  ["ctrl+space"],
    "clipboard": ["read", "write"],
    "dnd":       true,

    "sockets":   { "connect": ["host:port"], "listen": ["127.0.0.1:8080"] },
    "serial":    ["/dev/ttyUSB*"],
    "usb":       true,
    "bluetooth": true,
    "os":        true,
    "shell":     true,
    "ffi":       ["libfoo.so.1"],
    "plugin":    ["~/.local/share/myapp/*.wasm"]
  }
}
```

Notes that will save you a debugging session:

- **`fs.read` does not imply `fs.write`.** `rename` needs write on *both* ends, because a rename
  removes the source as surely as a delete does.
- **`process.exec: []` means unrestricted**, and the consent sheet flags it as `Extreme`. A bare
  name like `rg` matches only a bare name — `/tmp/rg` will not satisfy it.
- **`net.fetch` rejects wildcards at parse time.** `"*"`, `"https://*"`, and friends fail. An
  allow-list with a wildcard in it is not an allow-list.
- **`dbus.own` is separate from `dbus.destinations`.** Being able to *call* NetworkManager must not
  imply being able to *impersonate* it.
- **`ffi` is the escape hatch that voids the model.** Prefer `plugin`, which is sandboxed.

Unknown keys are a parse error, not a warning. A typo in a permission name would otherwise silently
grant nothing while the author believes it granted something.

---

## Reference

Field-by-field types live in [`crates/htmlapp-caps/src/manifest.rs`](../crates/htmlapp-caps/src/manifest.rs)
and [`permissions.rs`](../crates/htmlapp-caps/src/permissions.rs). Those are the source of truth;
this page is the readable version of them.

---

[← Getting started](getting-started.md) · **Document format** · [The bridge →](bridge.md)
