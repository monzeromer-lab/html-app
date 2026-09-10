# Contributing

[← Docs index](README.md) · [Getting started](getting-started.md) ·
[Document format](document-format.md) · [Bridge](bridge.md) · [API](api-reference.md) ·
[Security](security.md) · [Architecture](architecture.md) · [Building](building.md) ·
**Contributing** · [AGENT.md](AGENT.md)

> Working with a coding agent? **[AGENT.md](AGENT.md)** has the same material in the form an agent
> needs: invariants, file map, common tasks, and the traps that are not obvious from reading the
> code.

---

## Getting set up

```sh
git clone https://github.com/monzeromer-lab/htmlapp
cd htmlapp
cargo build
cargo test --workspace
./target/debug/htmlapp examples/hello.hta
```

Dependencies are listed in [Building](building.md#system-dependencies). `nix develop` gives you all
of them.

---

## The one thing to understand first

**`htmlapp-caps` is policy. `htmlapp-api` is mechanism.**

- `htmlapp-caps` decides what a document *may* do — manifests, globs, consent, diffs. It has no idea
  what a filesystem is.
- `htmlapp-api` decides whether *this call* is within that. Every module holds an `ApiContext`; every
  path, program, origin, database, and bus name goes through it.

Neither touches an engine or a window, which is why the whole security model tests in under a second
with no display server. Keep it that way. If you find yourself wanting to import `gpui` into either,
something has gone wrong.

---

## Layout

| Crate | Owns | `unsafe` |
|---|---|---|
| `htmlapp-caps` | Manifest, permissions, consent store | forbidden |
| `htmlapp-bridge` | Protocol, dispatch, JS shim, TypeScript emit | forbidden |
| `htmlapp-api` | The 28 native modules | denied, except `ffi` |
| `htmlapp-engine` | `WebEngine` trait, wry backend, occlusion, origin | denied, audited |
| `htmlapp-engine-cef` | Scaffolded Chromium backend | forbidden |
| `htmlapp-runtime` | Session, window lifecycle, consent flow, sandbox | forbidden |
| `htmlapp-shell` | GPUI launcher, consent sheet, permissions manager | forbidden |
| `htmlapp-views` | Terminal and table native views | forbidden |
| `htmlapp-wayland` | Layer-shell translation, outputs | forbidden |
| `htmlapp-build` | Stapling, desktop files, packaging | forbidden |
| `htmlapp` | CLI | forbidden |

Every `unsafe` block carries a comment explaining what makes it sound. There are four in the whole
workspace: two environment writes during startup, one WebKit version query, and `ffi` — which is
unsound by construction and documented as the escape hatch that voids the model.

---

## Common tasks

### Adding an API method

Four steps, in order. Missing the first is the usual mistake — the method will work at runtime but
be invisible to TypeScript and to the shim.

1. **Declare it** in [`crates/htmlapp-bridge/src/catalog.rs`](../crates/htmlapp-bridge/src/catalog.rs):

   ```rust
   invoke("chmod", "{ path: string; mode: number }", "void", "Change a file's permission bits."),
   ```

   The catalog is the single source of truth. The JS shim and the emitted `.d.ts` are both generated
   from it, so they cannot drift.

2. **Implement it** in the module's `invoke` (or `open_stream`) match arm, going through
   `ApiContext` for anything scoped.

3. **Test the enforcement**, not just the happy path. What does it do with a path outside the grant?
   A symlink pointing out of it? A program name that is a path?

4. **Check the shape**: `cargo test -p htmlapp-bridge` asserts the emitted TypeScript covers every
   catalog entry.

### Adding a whole module

Also register it in `register_all` in
[`crates/htmlapp-api/src/lib.rs`](../crates/htmlapp-api/src/lib.rs), gated on the right permission,
and add that permission to `Permissions` and `granted_modules()` in
[`htmlapp-caps/src/permissions.rs`](../crates/htmlapp-caps/src/permissions.rs). If it is
risk-bearing, add a `describe()` entry so it shows up on the consent sheet with an honest risk
level.

A module that is declared but not registered is a bug. There is an audit for it:

```sh
cargo test --workspace
```

### Adding an engine backend

Implement `WebEngine` in `htmlapp-engine`. Do not touch anything above it — the bridge, the
capability model, the API modules, and the shell are all written against the trait and should need
no changes. If they do, the trait is wrong and that is the thing to fix.

See [`htmlapp-engine-cef`](../crates/htmlapp-engine-cef/src/lib.rs) for a scaffold, and
[Architecture → Rendering](architecture.md#rendering-and-compositing) for what a compositing backend
would let the runtime stop doing.

---

## Testing

```sh
cargo test --workspace                          # 130 tests, no display server needed
cargo test -p htmlapp-caps --test security      # the policy half
cargo test -p htmlapp-api  --test enforcement   # the mechanism half
```

The suite is deliberately lopsided: it concentrates on the security boundary rather than spreading
evenly.

| Area | Tests | What they pin down |
|---|---|---|
| `htmlapp-caps` | 26 | Symlink and `..` escapes; wildcard-origin refusal; consent invalidation on edit; a stored record being unable to widen a grant |
| `htmlapp-engine` | 23 | Asset-root escapes; CSP following the manifest; domain-suffix confusion; integrity-mismatched modules; blob tokens; occlusion merging |
| `htmlapp-api` | 17 | Read not implying write; `rename` needing both ends; `fs.list` omitting ungranted entries; `/tmp/echo` not satisfying a grant of `echo` |
| `htmlapp-bridge` | 15 | Ungranted modules refused even when the handler exists; streams terminating rather than hanging; JS string escaping |
| `htmlapp-views` | 10 | Real escape-sequence handling; table streaming and column invalidation |
| `htmlapp-wayland` | 10 | Anchor bitmasks; protocol numbering; misconfiguration warnings |
| `htmlapp-build` | 9 | Staple round-trip; magic bytes in the payload not confusing extraction; `.html` not hijacked |
| `htmlapp-runtime` | 8 | The sandbox binding granted paths and nothing more; config parsing |
| inline | 12 | HTTP head parsing, WebSocket framing, accelerator normalisation |

### What a good test looks like here

Assert the *refusal*, not just the success. These are the tests that have actually caught things:

```rust
/// The escape the model exists to prevent.
#[test]
fn symlink_cannot_escape_the_granted_scope() {
    // …grant ~/logs/**, plant ~/logs/link-to-secrets → ~/secrets…
    assert!(!scope.allows(link.join("passwd")));
}
```

Six real bugs were found this way, each now with a regression test:

1. `globset`'s `*` crossed `/`, so `/var/log/nginx/*.log` matched `nginx/deep/access.log`.
2. `*.example.com` matched `notexample.com` — domain-suffix confusion.
3. `serde_json` escaping alone let `</script>` through into an injected script string.
4. `random_token` read `/dev/urandom` **to EOF** — a device with no end. Minting a blob URL hung
   forever and allocated without bound.
5. A headless document that threw never called `exit`, hanging whatever pipeline it was in.
6. The sandbox bound `/tmp` when a granted write path did not exist yet — an over-bind.

None of those were found by reading the code.

### Running the real thing

Some things only fail when you run them — the GPUI frame-loop panic and the GDK backend mismatch
both did:

```sh
./target/debug/htmlapp examples/hello.hta -v          # a document window
./target/debug/htmlapp examples/native-view.hta       # occlusion, visibly
./target/debug/htmlapp                                # the launcher
printf 'a,b\n1,2\n' | ./target/debug/htmlapp examples/csv-report.hta --headless
HTMLAPP_LOG=debug ./target/debug/htmlapp doc.hta      # full tracing
```

---

## Code style

`rustfmt` defaults, and the CI gate is **zero warnings**:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace 2>&1 | grep -c warning   # expect 0
```

### Comments

Comment density is low and deliberate. Write a comment when the code cannot say it itself:

- **Why, not what.** `// Bounded so a page generating events faster than they can be processed
  cannot starve the host's own frame.`
- **Non-obvious constraints.** `// `wl_display_connect(NULL)` falls back to
  `$XDG_RUNTIME_DIR/wayland-0`, so clearing the variable is not enough.`
- **Deliberate trade-offs**, especially security ones. If you chose the looser option, say why.
- **Every `unsafe` block**, without exception.

Do not narrate control flow. `// loop over the items` earns nothing.

Each file opens with a module comment explaining what it is for and why it is shaped that way. Match
that.

### Errors

Error messages are read by people writing a document at 1am. Say what was refused, what was granted,
and what to do:

```
access to /tmp/secret.txt (resolved to /tmp/secret.txt) is not granted by this document's manifest
`layer` windows cannot run here: this build was compiled without the `layer-shell` feature.
Install `libgtk-layer-shell-dev` and rebuild with `--features layer-shell`
```

Not `PermissionError` or `operation failed`.

---

## Pull requests

- One change per PR.
- Tests for anything security-relevant — the refusal case, not just the success case.
- Update the docs in the same PR. [`docs/`](.) is wired together; if you add a page, add it to the
  nav strip on every sibling and to [README.md](README.md).
- Zero warnings.
- If you change the API catalog, run `cargo run -p htmlapp -- types` and sanity-check the output.

Commits: imperative mood, explain *why* in the body if it is not obvious.

```
Reject unknown config keys

A typo like `launchre = false` previously parsed fine and left the launcher
enabled while the user believed they had turned it off.
```

---

## Good first issues

- **Implement `dnd.startDrag` properly.** The files are prepared; what is missing is the XDND
  *source* protocol — selection ownership and answering `XdndPosition` until a target accepts.
- **Implement `portal.screenCast`.** It hands back a PipeWire node id; consuming it needs a PipeWire
  client, which belongs with the `video` native view.
- **Implement the `editor` view.** GPUI plus tree-sitter. The plumbing and the trait already exist.
- **Upstream `HasWindowHandle` for GPUI's `X11Window`.** That deletes
  [`x11_window.rs`](../crates/htmlapp-engine/src/x11_window.rs) entirely.
- **The WPE backend.** The largest and highest-value piece: it is what replaces the SHAPE workaround
  with real compositing and makes document windows Wayland-native.

---

## Licence

Apache-2.0. By contributing you agree your contributions are licensed under it.

---

[← Building and packaging](building.md) · **Contributing** · [AGENT.md →](AGENT.md)
