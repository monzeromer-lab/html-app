# Getting started

[← Docs index](README.md) · **Getting started** · [Document format](document-format.md) ·
[Bridge](bridge.md) · [API](api-reference.md) · [Security](security.md) ·
[Architecture](architecture.md) · [Building](building.md) · [Contributing](contributing.md) ·
[AGENT.md](AGENT.md)

---

## Install

### From source

```sh
# Ubuntu / Debian
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
                 libxkbcommon-dev libwayland-dev libvulkan-dev libssl-dev pkg-config

# Fedora
sudo dnf install webkit2gtk4.1-devel gtk3-devel libsoup3-devel \
                 libxkbcommon-devel wayland-devel vulkan-loader-devel openssl-devel

# Arch
sudo pacman -S webkit2gtk-4.1 gtk3 libsoup3 libxkbcommon wayland vulkan-icd-loader openssl

cargo build --release
./target/release/htmlapp install    # register the .hta file type
```

Rust 1.90 or newer. The `install` step is not optional if you want double-clicking a `.hta` to
work — it writes the `.desktop` entry and the MIME package, then refreshes both databases.

### Other routes

`curl -fsSL https://htmlapp.dev/install.sh | sh`, or the `.deb`, AUR, Nix flake, Flatpak, and
AppImage recipes in [`packaging/`](../packaging). See [Building and packaging](building.md).

---

## Your first document

Save this as `hello.hta`:

```html
<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<script type="application/htmlapp+json">
{
  "name": "Hello",
  "id": "dev.example.hello",
  "window": { "title": "Hello", "width": 720, "height": 520 }
}
</script>
</head>
<body>
  <h1>Hello from HTML App</h1>
  <p id="out"></p>
  <script type="module">
    document.getElementById('out').textContent =
      `Runtime ${htmlapp.version}. Frozen: ${Object.isFrozen(htmlapp)}.`;
  </script>
</body>
</html>
```

```sh
htmlapp hello.hta
```

A window opens with a native titlebar and your page inside it. There is no `permissions` block, so
the document has no native APIs — `htmlapp.fs` is `undefined`. That is the default, and it is
silent.

Rename it to `hello.html` and open it in a browser: it still works, because the manifest block is
inert to browsers. That is the intended way to iterate on layout and CSS.

---

## Adding a capability

Say you want to read log files. Add a `permissions` block:

```json
{
  "name": "Log Peek",
  "id": "dev.example.logpeek",
  "window": { "title": "Log Peek", "width": 900, "height": 600 },
  "permissions": {
    "fs": { "read": ["/var/log/**"] }
  }
}
```

```js
// htmlapp.fs now exists — but only because the manifest asked for it.
const text = await htmlapp.fs.read({ path: '/var/log/syslog' });

// Anything outside the granted globs is refused, with a specific error.
try {
  await htmlapp.fs.read({ path: '/etc/shadow' });
} catch (e) {
  console.log(e.code);     // "permissionDenied"
}
```

The first time you run it, a consent sheet appears listing exactly what was requested, in plain
language, with the file's path and content hash. Your answer is remembered against
`sha256(file)` — so editing the file asks again, showing what changed.

Check what a document wants without running it:

```sh
htmlapp inspect log-peek.hta
```

```
Log Peek
  path      log-peek.hta
  sha256    9993faabc04531bf208fb57df154939512dadef64107747f6e1882b0d73b467b
  id        dev.example.logpeek
  mode      window
  window    900×600

  Requests: fs
    [Medium] Read files on this computer — Can read: /var/log/**.

  Consent:  will prompt on first run
```

---

## Editor completion

The bridge's TypeScript declarations are generated from the same catalog the runtime is built from:

```sh
htmlapp types > htmlapp.d.ts
```

```html
<script type="module">
  /// <reference path="./htmlapp.d.ts" />

  // The compiler knows fs is optional, because at runtime it might not exist.
  if (htmlapp.fs) {
    const stat = await htmlapp.fs.stat({ path: '/var/log/syslog' });
    console.log(stat.size);
  }
</script>
```

Still no build step — the declarations are for your editor, not for a compiler in your pipeline.

---

## In a shell pipeline

Set `window.mode` to `"headless"` and the document runs with no surface at all:

```html
<script type="application/htmlapp+json">
{ "name": "Report", "window": { "mode": "headless" } }
</script>
<script type="module">
  const input = await htmlapp.stdin.read();
  await htmlapp.stdout.write(JSON.stringify({ lines: input.split('\n').length }) + '\n');
  await htmlapp.exit(0);
</script>
```

```sh
cat access.log | htmlapp report.hta --headless --format json > summary.json
```

The process exits when the page calls `htmlapp.exit(code)`. An uncaught error exits `1` with the
message on stderr, so a broken document fails the pipeline instead of hanging it.

---

## Shipping it

```sh
htmlapp build tool.hta
```

```
binary     tool                     standalone; the document is stapled to a copy of the runtime
desktop    tool.desktop             with the icon extracted from the manifest
appdir     tool.AppDir              ready for appimagetool
flatpak    com.example.tool.json    Flatpak manifest
```

`./tool` runs its own embedded document with no arguments. The person you give it to never learns
the word "HTML App".

---

## Learning from the examples

Ten documents in [`examples/`](../examples), covering every reference use case. They are the tutorial:

| File | What it demonstrates |
|---|---|
| `hello.hta` | The minimum — a page in a window, no permissions |
| `native-view.hta` | A GPUI table with 100,000 rows, rendered *inside* HTML layout |
| `log-triage.hta` | Live tail with a regex filter — `fs.tail`, `clipboard`, `store` |
| `csv-report.hta` | A headless shell filter — `stdin` → `stdout` → `exit` |
| `sql-notebook.hta` | Bundled SQLite with prepared statements and transactions |
| `git-terminal.hta` | A real PTY in an `<htmlapp-view kind="terminal">` |
| `screenshot-annotator.hta` | Portal screenshot, canvas annotation, clipboard, save dialog |
| `serial-console.hta` | Serial ports for embedded work — streamed reads |
| `command-palette.hta` | A layer-shell overlay with a global hotkey |
| `wayland-bar.hta` | A top bar: anchors, exclusive zone, D-Bus |

The launcher lists them: run `htmlapp` with no arguments.

---

## Where to go next

- **[Document format](document-format.md)** — every manifest field, all five window modes.
- **[The bridge](bridge.md)** — `invoke`, `stream`, `on`, and when to use each.
- **[Security model](security.md)** — what a grant actually means, and what it does not.

---

[← Docs index](README.md) · **Getting started** · [Document format →](document-format.md)
