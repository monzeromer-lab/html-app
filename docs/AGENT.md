# AGENT.md

Instructions for coding agents working on this repository.

[← Docs index](README.md) · [Getting started](getting-started.md) ·
[Document format](document-format.md) · [Bridge](bridge.md) · [API](api-reference.md) ·
[Security](security.md) · [Architecture](architecture.md) · [Building](building.md) ·
[Contributing](contributing.md) · **AGENT.md** · [LLM.txt](LLM.txt)

---

## What this project is

A Rust runtime that turns one `.hta` file — HTML with a JSON manifest in a `<script>` block — into a
Linux desktop application, with a capability model that makes that safe. 11 crates, ~22,600 lines,
130 tests, zero warnings.

The single most important property: **a document gets no native APIs unless its manifest asks for
them and the user agrees.** Almost every design decision follows from that. If a change would make
it easier for a document to reach something it was not granted, it is wrong even if it compiles and
the tests pass.

---

## Before you change anything

```sh
cargo build --workspace 2>&1 | grep -c warning   # must be 0
cargo test --workspace                            # must be 130 passing
```

The zero-warning gate is real. Do not add `#[allow(dead_code)]` to silence something — either use it
or delete it. The exception is genuinely feature-gated code, where a `#[cfg_attr(not(feature =
"x"), allow(dead_code))]` is correct because the item exists for the other build.

---

## Invariants — do not break these

1. **`htmlapp-caps` and `htmlapp-api` never import `gpui`, `wry`, or anything windowing.** The
   security model must stay testable without a display server. If a change needs that, the change
   is in the wrong crate.

2. **Ungranted modules are absent, not disabled.** `htmlapp.fs` must be `undefined`, never a stub
   that throws. `typeof htmlapp.fs === "undefined"` is a documented, tested feature test.

3. **The gate is applied twice, on purpose.** `register_all` bounds what handler exists;
   `Dispatcher::resolve` independently bounds what is reachable. Do not "simplify" by removing
   either. They are not redundant.

4. **Path checks return the resolved path, and callers use it.** Re-resolving between the check and
   the open is a TOCTOU bug. `ApiContext::check_read` returns a `PathBuf` for this reason — use it,
   do not discard it and re-open `params.path`.

5. **The catalog is the single source of truth for the API surface.** Adding a method to a module
   without adding it to `crates/htmlapp-bridge/src/catalog.rs` means it is invisible to TypeScript
   and to the JS shim. A test asserts the emitted `.d.ts` covers every catalog entry.

6. **Consent is keyed on `sha256(file)`, never on the path.** A stored record may only *confirm* a
   grant. If the record's permissions no longer equal what the file currently asks for, it must
   re-prompt.

7. **Host→page payloads are JSON string literals, never interpolated as source.** `shim::json_string`
   escapes `<`, `>`, `&`, U+2028, U+2029. There is a regression test.

8. **Errors say what to do.** `"layer windows cannot run here: this build was compiled without the
   layer-shell feature. Install libgtk-layer-shell-dev and rebuild with --features layer-shell"`, not
   `"unsupported"`.

---

## File map

Start here for any given question:

| Question | File |
|---|---|
| What may a document do? | `crates/htmlapp-caps/src/permissions.rs` |
| How is a manifest parsed? | `crates/htmlapp-caps/src/manifest.rs` |
| How is consent stored and diffed? | `crates/htmlapp-caps/src/consent.rs` |
| Is *this call* within the grant? | `crates/htmlapp-api/src/context.rs` |
| What is the API surface? | `crates/htmlapp-bridge/src/catalog.rs` |
| How is a call routed and gated? | `crates/htmlapp-bridge/src/dispatch.rs` |
| What does the page see? | `crates/htmlapp-bridge/js/shim.js` |
| How is the engine abstracted? | `crates/htmlapp-engine/src/lib.rs` |
| How does the host paint over the page? | `crates/htmlapp-engine/src/occlusion.rs` |
| What does `htmlapp://app/` serve? | `crates/htmlapp-engine/src/origin.rs` |
| How does a document start? | `crates/htmlapp-runtime/src/session.rs` |
| How does the window loop run? | `crates/htmlapp-runtime/src/app.rs` |
| Where do host-only modules live? | `crates/htmlapp-runtime/src/host.rs` |
| The launcher UI | `crates/htmlapp-shell/src/launcher.rs` |
| CLI entry point | `crates/htmlapp/src/main.rs` |

**Every file opens with a module comment explaining why it is shaped the way it is.** Read it before
changing the file — it usually contains the constraint you are about to trip over.

---

## Common tasks

### Adding an API method

```
1. crates/htmlapp-bridge/src/catalog.rs   declare it  ← easy to forget, breaks TS silently
2. crates/htmlapp-api/src/<module>.rs     implement it, going through ApiContext
3. crates/htmlapp-api/tests/enforcement.rs  test the REFUSAL, not just the success
4. cargo test -p htmlapp-bridge           asserts the .d.ts covers it
```

### Adding a module

The above, plus:

```
5. crates/htmlapp-caps/src/permissions.rs   add the field, add it to granted_modules(),
                                            add a describe() entry if it is risk-bearing
6. crates/htmlapp-api/src/lib.rs            register it in register_all(), gated
```

A declared-but-unregistered module is a bug the test suite catches.

### Adding an engine backend

Implement `WebEngine` in `htmlapp-engine`. **Nothing above it should need changes.** If it does, the
trait is wrong and that is the thing to fix. `crates/htmlapp-engine-cef/src/lib.rs` is a scaffold
showing the shape.

---

## Traps

These are real, cost hours, and are not visible from reading the code.

### GPUI

- **`Window::request_animation_frame` panics outside a layout or paint phase.** It resolves the
  current view through `Window::current_view`, which asserts. The frame loop runs from
  `on_next_frame`, which is outside all of them — use `cx.notify()` to schedule the next frame
  instead.
- **`gpui::Window` does not implement `HasWindowHandle` on X11.** It is `unimplemented!()` and
  panics; the window id is `pub(crate)`. `crates/htmlapp-engine/src/x11_window.rs` recovers it from
  the X server via `_NET_WM_PID`. Do not call `window.window_handle()`.
- **`.when()` needs `FluentBuilder`, which is not re-exported.** Use a plain `if`.
- **`.id()` requires `InteractiveElement` in scope**, and must come before the styling calls.
- **`uniform_list` needs a `Context`.** The native views render without one — see
  `TableView::element`, which slices to the viewport by hand.

### The engine

- **`wry`'s `build_as_child` is X11-only on Linux.** Document windows run on X11/XWayland.
- **Clearing `WAYLAND_DISPLAY` is not enough** to make GTK use X11: `wl_display_connect(NULL)` falls
  back to `$XDG_RUNTIME_DIR/wayland-0`. `GDK_BACKEND=x11` must be set too. Both happen in
  `wry_backend::force_x11_session`, before any window exists.
- **The GTK main context must be pumped from GPUI's frame loop**, or WebKit never paints, never runs
  a timer, and never sees input. `wry_backend::pump_events`, bounded at 64 iterations.
- **`evaluate_script` must run on the engine's thread.** Bridge calls arrive on Tokio workers, so
  outbound messages queue in `QueueTransport` and the frame loop drains them.

### Dependencies

- **`ashpd` must match the feature set `gpui` pins** (`default-features = false, features =
  ["async-std"]`). Cargo unifies features across the graph, and ashpd has a `compile_error!` for
  having both async backends on. The same applies to `zbus` and `ksni`, which is why ksni uses
  `async-io` rather than its default `tokio`.
- **`bluer` needs its `bluetoothd` feature** or `Session`, `Adapter`, and `Device` do not exist.
- **`rusb` is built with `vendored`** so it does not need a `libusb-1.0-dev` package on the host.

### Async

- **Never hold a `parking_lot` guard across an `.await`.** It makes the future `!Send` and the
  dispatcher cannot box it. Resolve the lock to a value in its own scope first — see
  `NetModule::send` for the pattern.
- **`ApiHandler` futures must be `Send`.** Things that are not (a `udev::MonitorSocket`, a
  `serialport` handle) go on a dedicated thread with a channel back.

### Everything else

- **`/dev/urandom` has no EOF.** `std::fs::read` on it never returns. Use `read_exact` into a fixed
  buffer. *(This shipped and hung.)*
- **`globset`'s `*` crosses `/` by default.** `literal_separator(true)` is required, or
  `/var/log/*.log` matches at any depth. This is a security bug, not a cosmetic one.
- **Domain suffix matching needs a label boundary.** `url_host.ends_with(suffix)` lets
  `notexample.com` satisfy `*.example.com`. Compare against `format!(".{suffix}")`.
- **A headless document that throws must exit**, or it hangs the shell pipeline it is in. The shim
  installs `error` and `unhandledrejection` handlers that report on stderr and exit 1.

---

## Testing expectations

Assert the **refusal**, not just the success. Every security test in this repo is written that way,
and six real bugs were found by them:

```rust
#[test]
fn symlink_cannot_escape_the_granted_scope() {
    // grant ~/logs/**, plant ~/logs/link-to-secrets → ~/secrets
    assert!(!scope.allows(link.join("passwd")));
}
```

Test counts by crate, so you can tell if you broke something:

| Crate | Tests |
|---|---|
| `htmlapp-api` | 29 |
| `htmlapp-caps` | 26 |
| `htmlapp-engine` | 23 |
| `htmlapp-bridge` | 15 |
| `htmlapp-views` | 10 |
| `htmlapp-wayland` | 10 |
| `htmlapp-build` | 9 |
| `htmlapp-runtime` | 8 |
| **Total** | **130** |

Some failures only appear when you run the thing. Both the GPUI frame-loop panic and the GDK backend
mismatch did:

```sh
./target/debug/htmlapp examples/hello.hta -v
./target/debug/htmlapp examples/native-view.hta       # occlusion, visibly
printf 'a,b\n1,2\n' | ./target/debug/htmlapp examples/csv-report.hta --headless
HTMLAPP_LOG=debug ./target/debug/htmlapp doc.hta
```

---

## Style

- **rustfmt defaults.** Zero warnings.
- **Comments explain *why*.** Not what — the code says what. Comment non-obvious constraints,
  deliberate trade-offs (especially where you chose the looser option), and every `unsafe` block.
  Do not narrate control flow.
- **Match the surrounding density.** These files are lightly commented with substantial module-level
  headers. Do not carpet a function in line comments.
- **Prose in docs and comments is plain.** No marketing register, no "simply", no "just".
- **Error messages are read by someone debugging at 1am.** Say what was refused, what was granted,
  and what to do about it.

---

## What not to do

- Do not weaken a permission check to make a feature work. Ask instead.
- Do not add a dependency without checking it does not conflict with the `gpui`/`ashpd`/`zbus`
  feature unification described above.
- Do not put windowing code in `htmlapp-caps` or `htmlapp-api`.
- Do not silence a warning with `allow`; fix it.
- Do not add a method to a module without adding it to the catalog.
- Do not claim something works without running it. The examples exist for this.

---

## Current known gaps

Everything in the design is implemented except:

| | Why |
|---|---|
| `dnd.startDrag` | Files are prepared and put on the clipboard; the XDND *source* protocol is not implemented |
| `portal.screenCast` | Returns a PipeWire node id; consuming it needs the client that ships with the `video` view |
| `editor`, `video`, `canvas3d` views | Not implemented; `view.create` reports `unsupported` |
| Layer-shell modes | Code complete, gated on `libgtk-layer-shell-dev` at build time |
| Zero-copy DMA-BUF | Needs the WPE backend |
| CEF backend | Scaffolded, not implemented |

Do not describe any of these as working.

---

[← Contributing](contributing.md) · **AGENT.md** · [LLM.txt →](LLM.txt)
