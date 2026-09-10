# The bridge

[← Docs index](README.md) · [Getting started](getting-started.md) ·
[Document format](document-format.md) · **Bridge** · [API](api-reference.md) ·
[Security](security.md) · [Architecture](architecture.md) · [Building](building.md) ·
[Contributing](contributing.md) · [AGENT.md](AGENT.md)

---

## Three shapes, not one

```js
await htmlapp.invoke(method, params)             // one result
for await (const chunk of htmlapp.stream(m, p))  // many, cancellable
htmlapp.on(event, handler)                       // returns an unsubscribe function
```

Most bridges have only the first. That falls apart the moment you tail a log, read a 2 GB file, or
pipe a subprocess: every payload has to become one base64 blob, held entirely in memory on both
sides. So streams are first-class here, and for genuinely large transfers the bytes escape JSON
altogether — see [blob URLs](#blob-urls).

---

## The global

Injected at document-start, before any page script runs:

```js
const htmlapp = {
  invoke(method, params),      // Promise
  stream(method, params),      // AsyncIterable, with .cancel()
  on(event, handler),          // returns () => void
  once(event, handler),
  version: "0.1.0",
  permissions: { … },          // a frozen copy of what was granted
  view(id),                    // address a native view (§ Native views)
  // headless only:
  stdin, stdout, stderr, exit(code), format
};
Object.freeze(htmlapp);
```

`Object.freeze` and a non-configurable `window.htmlapp` are both deliberate. Page script cannot add
a module it was not granted, and cannot swap `invoke` for something that lies to the rest of the
page about what it called.

### Modules are absent, not disabled

```js
if (htmlapp.fs) {
  await htmlapp.fs.read({ path: '/tmp/x' });
}
```

A module the manifest did not grant is **not on the object**. `typeof htmlapp.fs === "undefined"` is
a truthful feature test, which is why the generated TypeScript marks every module optional — the
compiler makes you check exactly where the runtime would otherwise throw.

---

## Request / response

```js
const text = await htmlapp.fs.read({ path: '~/notes.md' });
const out  = await htmlapp.process.exec({ program: 'rg', args: ['-n', 'TODO'] });
```

Every method takes a single object. Failures reject with an `Error` carrying a machine-readable
code:

```js
try {
  await htmlapp.fs.read({ path: '/etc/shadow' });
} catch (e) {
  e.name;     // "HtmlAppError"
  e.code;     // "permissionDenied"
  e.message;  // "access to /etc/shadow (resolved to /etc/shadow) is not granted by …"
  e.data;     // { path, resolved, granted: [...], mode: "read" }
}
```

| Code | Means |
|---|---|
| `permissionDenied` | The manifest did not grant this. |
| `methodNotFound` | No such method. |
| `invalidParams` | Well-formed call, wrong arguments. |
| `operationFailed` | Permitted and well-formed, but it failed — `ENOENT`, a refused connection. |
| `unsupported` | Valid elsewhere: layer-shell under X11, a feature this build lacks. |
| `cancelled` | The user dismissed a dialog, or a stream was cancelled. |
| `internal` | A bug. Please report it. |

`permissionDenied` and `methodNotFound` are deliberately not distinguished for an ungranted module:
a page should not be able to enumerate the runtime's capabilities by probing for error codes.

---

## Streams

```js
const tail = htmlapp.fs.tail({ path: '/var/log/syslog', lines: 100 });

for await (const line of tail) {
  console.log(line);
  if (line.includes('FATAL')) break;   // breaking cancels the stream
}

tail.cancel();   // or cancel explicitly
```

Chunks queue rather than being dropped if they arrive faster than the loop body runs. Cancelling —
by `break`, by `.cancel()`, or by throwing — tells the host to stop producing, which is what makes a
cancelled `fs.watch` actually release its inotify watch instead of leaking it for the session.

Streaming methods across the API:

| Module | Streams |
|---|---|
| `fs` | `readStream`, `tail`, `watch` |
| `process` | `spawn`, `pty` |
| `http` | `stream` |
| `sql` | `stream` |
| `dbus` | `subscribe` |
| `systemd` | `journal` |
| `udev` | `monitor` |
| `net` | `receive`, `accept` |
| `serial` | `read` |
| `bluetooth` | `discover` |
| `portal` | `screenCast` |

---

## Events

```js
const off = htmlapp.on('os:theme', theme => {
  document.body.dataset.scheme = theme.colorScheme;
});

off();   // unsubscribe

htmlapp.once('window:close', () => save());
```

Events are namespaced `module:event`, and the module gate applies to them: subscribing to
`tray:activate` without `tray` in the manifest does nothing. An event with no listener is never
serialised, so subscribing to nothing costs nothing.

Full list in the [API reference](api-reference.md#events).

---

## Blob URLs

For bulk transfer, the host mints an unguessable URL the page fetches directly. The bytes never pass
through JSON:

```js
const url = await htmlapp.fs.blob({ path: '~/video.mp4' });
// "htmlapp://app/__blob__/b36453d1c098b9a3104f8804f4a41a05"

document.querySelector('video').src = url;
const response = await fetch(url);
```

A blob URL is a capability in its own right. It is only minted for a path that already passed the
scope check, and the token is 128 bits from `/dev/urandom` — not derivable from the path, so page
script cannot forge one for a file it was never granted.

For very large files that you want to read a window at a time, `fs.mmap` maps once and reads
windows out of the map, rather than re-reading on every call.

---

## Native views

Because the host can paint over the page, real native widgets participate in ordinary HTML layout:

```html
<htmlapp-view kind="terminal" id="term" style="flex:1"></htmlapp-view>
<htmlapp-view kind="table"    id="rows" style="height:400px"></htmlapp-view>
```

```js
const term = htmlapp.view('term');
await term.write('ls\n');

const rows = htmlapp.view('rows');
await rows.call('setColumns', { columns: ['id', 'level', 'message'] });
await rows.call('append', { rows: batch });
```

The element itself is a transparent placeholder that takes part in normal flow. A `ResizeObserver`
reports its rect over the bridge; the host punches a hole in the page's own window at that rect and
draws a GPUI element there. See
[Architecture → Rendering](architecture.md#rendering-and-compositing) for the mechanism.

| `kind` | Backed by | Why it cannot be HTML |
|---|---|---|
| `terminal` | `alacritty_terminal` | A real PTY and real escape handling; a DOM terminal looks right until the first curses program runs in it |
| `table` | GPUI, virtualised | Millions of rows at one row of elements per visible line |
| `editor`, `video`, `canvas3d` | — | Not implemented; `view.create` reports `unsupported` |

Batch your appends. A hundred thousand rows in one `call` is a ten-megabyte JSON string, which is
exactly what the three message shapes exist to avoid. `examples/native-view.hta` appends in chunks
of two thousand.

---

## Headless mode

```html
<script type="application/htmlapp+json">
{ "name": "Report", "window": { "mode": "headless" } }
</script>
```

```js
const input = await htmlapp.stdin.read();          // or: htmlapp.stdin.lines()
await htmlapp.stdout.write(JSON.stringify(result) + '\n');
await htmlapp.stderr.write('warning: …\n');
htmlapp.format;                                     // whatever --format was given
await htmlapp.exit(0);
```

```sh
cat access.log | htmlapp report.hta --headless --format json > summary.json
```

`--headless` means no window, not no display: the page still runs in a real WebKitGTK webview, so
GTK needs a display server even though nothing is drawn. On a machine without one — a CI runner, or
a server over ssh — run it under `xvfb-run`.

The process exits when the page calls `htmlapp.exit(code)`. **An uncaught error or unhandled
rejection exits `1`** with the message on stderr — a document that throws must not hang the pipeline
it is part of.

`stdout` is flushed on every write, so a consumer downstream sees output as it is produced rather
than when the process happens to end.

---

## TypeScript

```sh
htmlapp types > htmlapp.d.ts
```

840 lines of declarations, generated from the same catalog the runtime is built from — so completion
in your editor describes the runtime you actually get. It type-checks clean under `tsc --strict`.

```ts
/// <reference path="./htmlapp.d.ts" />

// Every module is optional, because a grant decides whether it exists.
if (htmlapp.fs) {
  const stat: FileStat = await htmlapp.fs.stat({ path: '/tmp/x' });
}

// Events are typed by name.
htmlapp.on('window:resize', p => console.log(p.width, p.height));

// Streams are streams, not promises.
const watcher: HtmlAppStream<WatchEvent> = htmlapp.fs!.watch({ path: '/tmp' });
```

Still no build step. The declarations are for your editor.

---

## Wire protocol

You do not need this to write a document — it is here for anyone working on the runtime, or
debugging with `HTMLAPP_LOG=debug`.

**Page → host**, over the engine's single IPC channel, JSON:

```json
{ "t": "invoke",       "id": 1, "method": "fs.read", "params": { "path": "/tmp/x" } }
{ "t": "streamStart",  "id": 2, "method": "fs.tail", "params": { "path": "/var/log/syslog" } }
{ "t": "streamCancel", "id": 2 }
{ "t": "subscribe",    "id": 3, "event": "os:theme" }
{ "t": "unsubscribe",  "id": 3 }
```

**Host → page**, evaluated in the page as a call to `window.__htmlapp_dispatch`:

```json
{ "t": "result",      "id": 1, "value": "file contents" }
{ "t": "error",       "id": 1, "error": { "code": "permissionDenied", "message": "…" } }
{ "t": "chunk",       "id": 2, "value": "a log line" }
{ "t": "end",         "id": 2 }
{ "t": "streamError", "id": 2, "error": { … } }
{ "t": "event",       "event": "os:theme", "payload": { "colorScheme": "dark" } }
```

The payload is embedded as a JSON **string literal** and parsed on the other side, never
interpolated as source. `<`, `>`, `&`, U+2028, and U+2029 are `\u`-escaped, so a filename containing
`</script>` cannot break out. There is a regression test for exactly that.

---

## Where the code is

| | |
|---|---|
| Protocol types | [`crates/htmlapp-bridge/src/protocol.rs`](../crates/htmlapp-bridge/src/protocol.rs) |
| The injected shim | [`crates/htmlapp-bridge/js/shim.js`](../crates/htmlapp-bridge/js/shim.js) |
| The API catalog | [`crates/htmlapp-bridge/src/catalog.rs`](../crates/htmlapp-bridge/src/catalog.rs) |
| Dispatch and gating | [`crates/htmlapp-bridge/src/dispatch.rs`](../crates/htmlapp-bridge/src/dispatch.rs) |
| TypeScript emit | [`crates/htmlapp-bridge/src/typescript.rs`](../crates/htmlapp-bridge/src/typescript.rs) |

The catalog is the single source of truth: both the shim and the `.d.ts` are generated from it, so
they cannot drift apart. Adding a method means editing the catalog *and* the module — see
[AGENT.md](AGENT.md#adding-an-api-method).

---

[← Document format](document-format.md) · **The bridge** · [API reference →](api-reference.md)
