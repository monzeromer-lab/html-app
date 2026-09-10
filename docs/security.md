# Security model

[← Docs index](README.md) · [Getting started](getting-started.md) ·
[Document format](document-format.md) · [Bridge](bridge.md) · [API](api-reference.md) ·
**Security** · [Architecture](architecture.md) · [Building](building.md) ·
[Contributing](contributing.md) · [AGENT.md](AGENT.md)

---

## The failure this exists to avoid

Windows HTAs became a malware delivery vector for one reason: **opening a file was granting full
trust.** There was no manifest, no prompt, no scoping. Double-clicking a `.hta` gave it everything
you had.

HTML App inherits the extension. It has to be obviously and structurally different, or it deserves
the same reputation. Everything below is that difference, and it is enforced in Rust rather than
promised in documentation.

---

## The model, in eight rules

### 1. Deny by default

No manifest, or no `permissions` block, means **no native APIs at all**. The document is a page in a
window.

This is **silent, not an error**. A `.hta` with no manifest is a perfectly good thing to be, and
making that a warning would train people to ignore warnings.

```console
$ htmlapp probe.hta --headless
{ "granted": [], "fs": "undefined" }
```

`htmlapp.fs` is `undefined` — not a stub that throws. The module is never constructed, never
registered, and never written onto the global.

### 2. Declarative, host-enforced

Permissions live in the manifest and are checked in Rust, at the point each capability is actually
exercised. Nothing in page JavaScript can widen them:

- `htmlapp` is `Object.freeze`d and installed as a non-configurable property.
- Every call is re-checked by the host. The shim is a convenience, not a boundary.
- Ungranted modules are absent, so there is nothing to monkey-patch.

The gate is applied **twice**, on purpose. `register_all` decides what handler exists at all; the
dispatcher independently refuses a module the manifest did not grant, even if one were somehow
registered. Neither check is redundant — one bounds what exists, the other bounds what is reachable.

### 3. Hash-pinned consent

On first run of unknown content, a native sheet lists exactly what was requested, in plain language,
with the source path and content hash. The decision is stored against **`sha256(file)`**, not the
path.

Editing the file invalidates consent and re-prompts, showing a **diff** of what changed:

```console
$ htmlapp inspect probe.hta
  Requests: fs, process
    [Extreme] Run other programs — Can run any program on this system.
    [Medium] Read files on this computer — Can read: /**.

  Consent:  will re-prompt — the file changed (added: process, changed: fs, removed: none)
```

This is the mitigation for the trojan-update threat: a file you trusted last week cannot quietly
grow a `process` grant this week.

A stored record can only ever *confirm* a grant. If the record's permissions no longer match what
the file currently asks for, the hash match is treated as not meaningful and the user is asked
again — so tampering with the consent store, or a hash collision, cannot shortcut the prompt.

The default button on the sheet is **Open without permissions**, not Allow. Rule 1 makes a powerless
document a working document, so the safe choice is also a useful one.

### 4. Portals over prompts

Where `xdg-desktop-portal` covers a capability — file choosing, screenshots, screencast, global
shortcuts, background running — HTML App uses it. You see your desktop's own dialog, and the runtime
never holds a broad grant it does not need.

A file you pick in your own file chooser is **authorised by that act**, regardless of the manifest's
globs. Choosing it *is* the grant. Only the exact file, never its directory. The same applies to a
file you drag onto the window.

### 5. Path scoping with symlink resolution

Globs are checked against **canonical** paths, and the canonical path is what gets opened:

```
~/logs/**  granted
~/logs/link-to-etc/passwd  →  resolves to /etc/passwd  →  refused
```

Two details that matter:

- A write target that does not exist yet is still checked, by canonicalising the deepest existing
  ancestor and folding `.` and `..` off the remainder. So a not-yet-created file behind an escaping
  symlink is refused too.
- The resolved path is the one operated on. Re-resolving between the check and the open would leave
  a window in which a symlink could be swapped underneath the decision.

`*` does not cross `/`; `**` does. `/var/log/nginx/*.log` does not match
`/var/log/nginx/deep/access.log`. *(This was a real bug, caught by a test.)*

### 6. Revocable

```sh
htmlapp permissions list                 # every stored decision
htmlapp permissions revoke tool.hta      # by path and by current hash
htmlapp permissions revoke-all
htmlapp permissions allow tool.hta       # deliberate grant, prints what it is granting
htmlapp permissions deny tool.hta
```

`htmlapp --permissions-ui` opens a native settings window that does the same. Revocation is
immediate and unconfirmed on purpose: taking a permission *away* is the safe direction, and a
confirmation step there would train people to click through the dialogs that actually matter.

The store is a single JSON file at `~/.local/share/htmlapp/consent.json`, deliberately
hand-inspectable — "what has this granted?" is a question you should be able to answer without
tooling.

### 7. Defence in depth

```sh
htmlapp --sandbox tool.hta
```

Re-execs the whole tree under **bubblewrap** with only the granted paths bind-mounted. WebKit's own
multi-process renderer sandbox is retained underneath.

The jail is built *after* the manifest is read, from what that document actually asked for, and
*before* any of it runs. `$HOME` is not bound. A grant of `~/logs/**` binds `~/logs`, not `~`.

Its real limits, stated plainly: the display server socket and the session bus have to come through
or nothing renders. A document that can talk to the X server can talk to other X clients. This is
defence in depth, not the primary control — the manifest and the bridge are.

*(A test caught this binding `/tmp` when a granted write path did not exist yet. It now binds the
exact path with `--bind-try`, which tolerates a missing source.)*

### 8. Content Security Policy

A restrictive policy is injected unless the manifest overrides it:

```
default-src htmlapp://app;
script-src  htmlapp://app 'unsafe-inline' 'unsafe-eval' blob:;
style-src   htmlapp://app 'unsafe-inline';
connect-src htmlapp://app blob: data: <the manifest's net.fetch origins>;
frame-src 'none'; object-src 'none'; base-uri 'none';
form-action 'none'; frame-ancestors 'none'
```

`'unsafe-inline'` is not an oversight. A single-file app's script *is* inline; a nonce-based policy
would mean rewriting the author's document, which the zero-build promise rules out.

The control that actually matters here is **`connect-src`**, pinned to the manifest's `net.fetch`
allow-list and nothing else. A page can run whatever it likes; it cannot phone anywhere it was not
granted.

Off-origin navigation is blocked unless declared. This is a runtime, not a browser: there are no
tabs, no address bar, and no arbitrary browsing.

---

## Threat model

| Threat | Mitigation |
|---|---|
| Malicious file from Slack or email | Deny by default; consent sheet; hash pinning |
| Supply chain via `imports` | Integrity hashes required at parse time; cached and pinned on first fetch; never revalidated |
| Privilege escalation from the renderer | WebKit's multi-process sandbox; the bridge is the only channel; optional bubblewrap |
| Exfiltration via `http` | Origin allow-list, enforced per request and per redirect; no wildcard accepted |
| Exfiltration via page `fetch()` | `connect-src` pinned to the same allow-list |
| Symlink escape from `fs` | Canonical-path checks; the resolved path is the one opened |
| Domain-suffix confusion | `*.example.com` requires a label boundary — `notexample.com` does not match |
| Script injection into the bridge | Payloads are JSON string literals, never interpolated as source; `<`, `>`, `&`, U+2028/9 escaped |
| Trojan update to a trusted file | Hash change re-prompts with a permission diff |
| Consent fatigue | Portals for common cases; few, meaningful, plain-language prompts |
| Credential theft via `os.env` | Variables matching credential patterns are withheld |
| Blob URL forgery | 128 unguessable bits from `/dev/urandom`, minted only for an already-checked path |
| `ffi` | **Exempt from none of this.** It is documented as the escape hatch that voids the model. |

---

## What a grant actually means

Worth being precise, because the intuitive reading is sometimes wrong:

- **`fs.read: ["~/**"]`** — every file in your home directory, including `.ssh`, `.aws`, and browser
  profiles. It is a very large grant. The consent sheet lists the globs verbatim rather than
  summarising them, for exactly this reason.
- **`process.exec: ["sh"]`** — effectively unrestricted, because `sh` runs anything. The sheet
  reports the allow-list, not an assessment of what is on it; you have to read it.
- **`dbus`** with no `destinations` — every service on the bus, which on a typical desktop includes
  logind, NetworkManager, and the keyring.
- **`ffi`** — arbitrary native code. There is no meaningful bound.

Risk levels shown on the consent sheet, most alarming first:

| Level | Examples |
|---|---|
| `Extreme` | any `ffi`; `process` with an empty allow-list |
| `High` | `fs.write`; `process` with a list; `secrets`; `dbus` |
| `Medium` | `fs.read`; `http`; `sql`; clipboard read; devices |
| `Low` | notifications; `store`; clipboard write |

---

## Process and instance model

**Each document runs in its own process.** Isolation is a security property, not an implementation
detail: two open `.hta` files never share a bridge, a permission grant, or an address space.

The launcher is single-instance and spawns documents as detached children. Closing it does not kill
running documents. A document launched from the CLI or a file manager never shows the launcher.

Subprocesses a document started are killed when it closes, so a page cannot outlive its window.

---

## Reporting a vulnerability

Email **support@alhakeem.app**. Please do not open a public issue for anything that lets a document
exceed its manifest.

Particularly interested in: a path that escapes its scope, a way to reach an ungranted module, a
consent decision that survives an edit, or anything that gets script into the bridge's dispatch
path.

---

## Where the code is

| | |
|---|---|
| Permission model, path scoping | [`crates/htmlapp-caps/src/permissions.rs`](../crates/htmlapp-caps/src/permissions.rs) |
| Consent store, hash pinning, diffs | [`crates/htmlapp-caps/src/consent.rs`](../crates/htmlapp-caps/src/consent.rs) |
| Manifest parsing and validation | [`crates/htmlapp-caps/src/manifest.rs`](../crates/htmlapp-caps/src/manifest.rs) |
| Per-call enforcement | [`crates/htmlapp-api/src/context.rs`](../crates/htmlapp-api/src/context.rs) |
| Module gating | [`crates/htmlapp-bridge/src/dispatch.rs`](../crates/htmlapp-bridge/src/dispatch.rs) |
| CSP and navigation policy | [`crates/htmlapp-engine/src/origin.rs`](../crates/htmlapp-engine/src/origin.rs) |
| bubblewrap jail | [`crates/htmlapp-runtime/src/sandbox.rs`](../crates/htmlapp-runtime/src/sandbox.rs) |

The security tests are the ones to read first — they encode the intent more precisely than prose
can:

```sh
cargo test -p htmlapp-caps --test security      # policy
cargo test -p htmlapp-api  --test enforcement   # mechanism
cargo test -p htmlapp-engine --test origin      # origin, CSP, navigation
cargo test -p htmlapp-runtime --test sandbox    # the jail
```

---

[← API reference](api-reference.md) · **Security model** · [Architecture →](architecture.md)
