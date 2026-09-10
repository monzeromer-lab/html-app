# HTML App documentation

**A single `.hta` file, running as a real Linux desktop application.**

```sh
htmlapp dashboard.hta
```

No `npm`, no bundler, no project directory. The input is a file, not a project, and editing the file
and saving it is the entire dev loop.

---

## Start here

| If you want to… | Read |
|---|---|
| Write your first `.hta` in five minutes | **[Getting started](getting-started.md)** |
| Know exactly what goes in a document | **[Document format](document-format.md)** |
| Call native APIs from page JavaScript | **[The bridge](bridge.md)** |
| Look up a specific module or method | **[API reference](api-reference.md)** |
| Understand what a document can and cannot do | **[Security model](security.md)** |
| Understand how the runtime is put together | **[Architecture](architecture.md)** |
| Build, package, or ship it | **[Building and packaging](building.md)** |
| Work on the runtime itself | **[Contributing](contributing.md)** |

**Working with a coding agent on this repository?** Point it at
**[AGENT.md](AGENT.md)** — the conventions, invariants, and traps it needs to not break things.
There is also **[LLM.txt](LLM.txt)**, a compact machine-readable map of the whole project.

---

## The shape of it in one page

A `.hta` is an ordinary HTML file with one extra thing in it: a typed script block the host reads
**before** the page loads.

```html
<!DOCTYPE html>
<html>
<head>
<script type="application/htmlapp+json">
{
  "name": "Log Triage",
  "id": "dev.example.logtriage",
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

That block is inert to browsers, so renaming the file to `.html` opens it in Firefox — without
native APIs. Browser devtools stay usable while you write.

Three things follow from the design, and they are the three worth internalising:

1. **Deny by default.** A document with no `permissions` block gets no native APIs at all.
   `htmlapp.fs` is `undefined` — not a stub that throws. This is silent, not an error: the file is
   a page in a window, and that is a perfectly good thing to be.
2. **Consent is pinned to content.** A decision is stored against `sha256(file)`, not the path. Edit
   the file and it asks again, showing a diff of what changed in the permission set.
3. **Enforcement is in Rust.** The `htmlapp` object is frozen and every call is re-checked by the
   host. Nothing in page JavaScript can widen a grant.

---

## Project layout

```
crates/
  htmlapp/              CLI, argument parsing, mode dispatch
  htmlapp-caps/         manifest parsing, permission model, consent store   ← the policy
  htmlapp-api/          the native API modules, each gated on a permission  ← the enforcement
  htmlapp-bridge/       JSON-RPC dispatch, the JS shim, TypeScript emit
  htmlapp-engine/       the WebEngine trait and its backends
  htmlapp-engine-cef/   optional Chromium backend (designed, not implemented)
  htmlapp-runtime/      window lifecycle, consent flow, orchestration
  htmlapp-shell/        GPUI chrome: launcher, consent sheet, permissions manager
  htmlapp-views/        native views placed inline in HTML layout
  htmlapp-wayland/      layer-shell translation, output enumeration
  htmlapp-build/        stapling, .desktop, MIME, AppImage, Flatpak
examples/               one .hta per reference use case
packaging/              .deb, AUR, Flatpak, AppImage
```

`htmlapp-caps` decides what a document *may* do; `htmlapp-api` decides whether a given call is
within it. Neither touches an engine or a window, so the whole security model is testable — and
auditable — without starting a browser. [Architecture](architecture.md) goes into why.

---

## Status

Everything in the design is implemented. Two things are gated on system libraries rather than on
code, and one is a deliberate future addition:

| | |
|---|---|
| Layer-shell window modes (bars, docks, overlays) | Needs `libgtk-layer-shell-dev`; build with `--features layer-shell` |
| Zero-copy DMA-BUF rendering | Needs the WPE backend; the current backend composites via X11 SHAPE instead |
| Chromium parity (WebGPU, WebCodecs) | The [CEF backend](architecture.md#the-cef-backend) is scaffolded, not implemented |

See [Architecture → Rendering](architecture.md#rendering-and-compositing) for what that actually
costs you in practice, which is less than it sounds.

---

## Licence

Apache-2.0. See [LICENSE](../LICENSE).
