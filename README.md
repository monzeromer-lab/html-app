# HTML App

**A single `.hta` file, running as a real Linux desktop application.**

```sh
htmlapp dashboard.hta
```

No `npm`, no bundler, no config directory, no scaffolding command. The input is a file, not a
project. Editing the file and saving it is the entire dev loop.

This repository implements the design in [`docs/htmlapp-prd.md`](docs/htmlapp-prd.md). For exactly
which parts are done, which are partial, and which are blocked — and on what — see
**[`docs/STATUS.md`](docs/STATUS.md)**.

---

## Quickstart

```sh
cargo build --release

# Open the launcher: recents, examples, diagnostics.
./target/release/htmlapp

# Run a document.
./target/release/htmlapp examples/hello.hta

# Run one in a shell pipeline, with no window at all.
printf 'name,score\nada,91\ngrace,88\n' | ./target/release/htmlapp examples/csv-report.hta --headless

# See what a document asks for, without running it.
./target/release/htmlapp inspect examples/log-triage.hta

# Register the .desktop entry and MIME type, so double-clicking a .hta works.
./target/release/htmlapp install
```

### Build dependencies

Ubuntu / Debian:

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
                 libxkbcommon-dev libwayland-dev libvulkan-dev libssl-dev pkg-config
```

Rust 1.90 or newer.

---

## What a `.hta` looks like

Configuration lives in the document, in a typed script block the host parses **before** the file
reaches the engine. The block is inert to browsers, so renaming a `.hta` to `.html` opens it in any
browser — without native APIs. Browser devtools stay usable while you write.

```html
<!DOCTYPE html>
<html>
<head>
<script type="application/htmlapp+json">
{
  "name": "Log Triage",
  "id": "dev.monzer.logtriage",
  "window": { "title": "Log Triage", "width": 1200, "height": 800 },
  "permissions": {
    "fs":      { "read": ["~/logs/**", "/var/log/nginx/*.log"] },
    "process": { "exec": ["rg", "journalctl"] }
  }
}
</script>
</head>
<body>
  <script type="module">
    // A module only exists if the manifest was granted it.
    if (htmlapp.fs) {
      for await (const line of htmlapp.fs.tail({ path: '/var/log/syslog' })) {
        console.log(line);
      }
    }
  </script>
</body>
</html>
```

---

## The security model

HTA became a malware vector because opening a file *was* granting full trust. This is structurally
different, and the difference is enforced in Rust rather than promised in documentation:

| | |
|---|---|
| **Deny by default** | No manifest, or no `permissions` block, means no native APIs. `htmlapp.fs` is `undefined` — not a stub that throws. The file is a page in a window, and that is silent, not an error. |
| **Declarative, host-enforced** | Permissions live in the manifest and are checked in Rust on every call. The `htmlapp` object is frozen; nothing in JS can widen a grant. |
| **Hash-pinned consent** | A decision is stored against `sha256(file)`, not the path. Editing the file re-prompts, showing a diff of what changed in the permission set. |
| **Portals over prompts** | File choosing goes through `xdg-desktop-portal`, so the picker is your desktop's own — and a file you pick is authorised by that act, regardless of the manifest's globs. |
| **Symlink-resolved paths** | Globs are checked against canonical paths, so `~/logs/link-to-etc` cannot escape. The resolved path is also the one opened, closing the TOCTOU gap. |
| **Revocable** | `htmlapp permissions list` / `revoke`, or the native settings window. |
| **No wildcard origins** | `"net": { "fetch": ["*"] }` fails to parse. `*.example.com` does not match `notexample.com`. |

You can see all of this from the outside:

```console
$ htmlapp probe.hta --headless        # no consent recorded yet
{ "granted": [], "fs": "undefined" }

$ htmlapp permissions allow probe.hta
Permission Probe
  sha256  9993faab…
    [Medium] Read files on this computer — Can read: /tmp/htatest/logs/**.
Granted. This is pinned to the file's current contents; editing it asks again.

$ htmlapp probe.hta --headless
{ "granted": ["fs"],
  "inScope":  "READ: INFO ok",
  "outScope": "permissionDenied: access to /tmp/htatest/secret.txt …" }
```

---

## The bridge

Three message shapes, not one — because forcing every bulk payload into a single base64 blob makes
tailing a log or reading a 2 GB file fall apart.

```js
await htmlapp.invoke(method, params)            // one result
for await (const chunk of htmlapp.stream(m, p))  // many, cancellable
htmlapp.on(event, handler)                       // returns unsubscribe
```

`htmlapp types > htmlapp.d.ts` emits TypeScript declarations generated from the same catalog the
runtime is built from, so editor completion describes the runtime you actually get — with no build
step. The generated file type-checks clean under `tsc --strict`.

---

## Layout

```
crates/
  htmlapp/            CLI, arg parsing, mode dispatch
  htmlapp-caps/       manifest parsing, permission model, consent store   ← the policy
  htmlapp-api/        the native API modules, each gated on a permission  ← the enforcement
  htmlapp-bridge/     JSON-RPC dispatch, JS shim, TypeScript emit
  htmlapp-engine/     WebEngine trait + the wry backend
  htmlapp-runtime/    window lifecycle, consent flow, orchestration
  htmlapp-shell/      GPUI chrome: launcher, consent sheet, permissions manager
  htmlapp-views/      native views placed inline in HTML layout
  htmlapp-wayland/    layer-shell translation, output enumeration
  htmlapp-build/      stapling, .desktop, MIME, AppImage, Flatpak
examples/             one .hta per reference use case
```

`htmlapp-caps` decides what a document *may* do; `htmlapp-api` decides whether a given call is
within it. Neither touches an engine or a window, so the whole security model is testable — and
auditable — without starting a browser.

---

## Distribution

```sh
htmlapp build tool.hta
# → tool                    standalone binary (document stapled to a copy of the runtime)
# → tool.desktop            with the icon extracted from the manifest
# → tool.AppDir             ready for appimagetool
# → com.example.tool.json   Flatpak manifest
```

The end user gets one file and never learns the word "HTML App".

---

## Known limitations

These are properties of the engine backend that ships today, not oversights. Each is explained in
full where it lives in the code, and [`docs/STATUS.md`](docs/STATUS.md) has the complete list.

- **Document windows run on X11 / XWayland.** wry embeds a native child surface, and
  `build_as_child` is X11-only on Linux. The launcher itself is pure GPUI and runs Wayland-native.
- **The page paints above the app chrome.** A native child surface is not composited into GPUI's
  scene, so modals cannot yet paint over the page and `rounded()` does not clip it. This is the
  single thing PRD §6.3's offscreen backend exists to fix.
- **`mode: "layer"` and `mode: "lock"` refuse to run**, with an explanation that distinguishes "your
  compositor lacks the protocol" from "this backend cannot render into one".

---

## Testing

```sh
cargo test --workspace     # 105 tests
```

The suite concentrates on the security boundary: symlink escapes, domain-suffix confusion, consent
invalidation on edit, ungranted-module absence, and shell-injection-shaped inputs to the JS shim.

## Licence

Apache-2.0.
