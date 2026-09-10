//! The bridge's transport, gating, and codegen (docs/bridge.md).

use std::sync::Arc;

use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use htmlapp_bridge::{
    ClientMessage, Dispatcher, ErrorCode, HostMessage, RpcError, ShimConfig, Transport,
    emit_typescript, render_shim,
};
use htmlapp_caps::Permissions;
use parking_lot::Mutex;
use serde_json::{Value, json};

/// Collects host→page messages so a test can assert on what the page would have seen.
#[derive(Default)]
struct Recorder(Mutex<Vec<HostMessage>>);

impl Transport for Recorder {
    fn send(&self, message: HostMessage) {
        self.0.lock().push(message);
    }
}

impl Recorder {
    fn messages(&self) -> Vec<HostMessage> {
        self.0.lock().clone()
    }
}

/// A stand-in `fs` module. Registered but not necessarily granted, which is the case that matters.
struct FakeFs;

impl ApiHandler for FakeFs {
    fn name(&self) -> &'static str {
        "fs"
    }

    fn invoke<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "read" => Ok(json!(format!(
                    "contents of {}",
                    params["path"].as_str().unwrap_or("?")
                ))),
                _ => Err(RpcError::not_found(method)),
            }
        })
    }

    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            match method {
                "readStream" => {
                    let chunks = vec![Ok(json!("one")), Ok(json!("two")), Ok(json!("three"))];
                    Ok(Box::pin(futures::stream::iter(chunks)) as ValueStream)
                }
                _ => Err(RpcError::not_found(method)),
            }
        })
    }
}

fn permissions(json: &str) -> Permissions {
    serde_json::from_str(json).expect("test permissions must parse")
}

fn dispatcher(granted: Option<Permissions>) -> (Arc<Dispatcher>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let mut dispatcher =
        Dispatcher::new(Arc::clone(&recorder) as Arc<dyn Transport>, granted, false);
    dispatcher.register(Arc::new(FakeFs));
    (Arc::new(dispatcher), recorder)
}

// --- the bridge transport ---

#[tokio::test]
async fn invoke_round_trips() {
    let (dispatcher, recorder) = dispatcher(Some(permissions(r#"{"fs":{"read":["/**"]}}"#)));

    dispatcher
        .handle(ClientMessage::Invoke {
            id: 7,
            method: "fs.read".into(),
            params: json!({ "path": "/tmp/a" }),
        })
        .await;

    match &recorder.messages()[..] {
        [HostMessage::Result { id: 7, value }] => {
            assert_eq!(value, &json!("contents of /tmp/a"));
        }
        other => panic!("unexpected: {other:?}"),
    }
}

/// The bridge transport: "Streaming is not optional."
#[tokio::test]
async fn stream_yields_chunks_then_ends() {
    let (dispatcher, recorder) = dispatcher(Some(permissions(r#"{"fs":{"read":["/**"]}}"#)));

    dispatcher
        .handle(ClientMessage::StreamStart {
            id: 3,
            method: "fs.readStream".into(),
            params: json!({ "path": "/tmp/a" }),
        })
        .await;

    // The pump runs on a spawned task.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let messages = recorder.messages();
    let chunks: Vec<_> = messages
        .iter()
        .filter_map(|m| match m {
            HostMessage::Chunk { value, .. } => value.as_str(),
            _ => None,
        })
        .collect();
    assert_eq!(chunks, ["one", "two", "three"]);
    assert!(
        matches!(messages.last(), Some(HostMessage::End { id: 3 })),
        "a stream must terminate: {messages:?}"
    );
}

// --- the security model, rule 2: nothing in JS can widen the grant ---

/// The central gating test: the handler is registered, but the manifest did not grant it.
#[tokio::test]
async fn ungranted_module_is_refused_even_though_the_handler_exists() {
    let (dispatcher, recorder) = dispatcher(Some(permissions(r#"{"store":true}"#)));

    dispatcher
        .handle(ClientMessage::Invoke {
            id: 1,
            method: "fs.read".into(),
            params: json!({ "path": "/etc/passwd" }),
        })
        .await;

    match &recorder.messages()[..] {
        [HostMessage::Error { id: 1, error }] => {
            assert_eq!(error.code, ErrorCode::PermissionDenied);
        }
        other => panic!("ungranted fs.read must be denied, got {other:?}"),
    }
}

/// The security model, rule 1: a document with no manifest reaches nothing.
#[tokio::test]
async fn document_without_manifest_reaches_nothing() {
    let (dispatcher, recorder) = dispatcher(None);

    dispatcher
        .handle(ClientMessage::Invoke {
            id: 1,
            method: "fs.read".into(),
            params: json!({ "path": "/etc/passwd" }),
        })
        .await;

    assert!(matches!(
        &recorder.messages()[..],
        [HostMessage::Error { error, .. }] if error.code == ErrorCode::PermissionDenied
    ));
}

/// A denied stream must fail the `for await`, not hang it.
#[tokio::test]
async fn ungranted_stream_fails_rather_than_hanging() {
    let (dispatcher, recorder) = dispatcher(None);

    dispatcher
        .handle(ClientMessage::StreamStart {
            id: 2,
            method: "fs.readStream".into(),
            params: Value::Null,
        })
        .await;

    assert!(matches!(
        &recorder.messages()[..],
        [HostMessage::StreamError { id: 2, error }] if error.code == ErrorCode::PermissionDenied
    ));
}

/// Headless mode: stdio exists only in headless mode.
#[tokio::test]
async fn stdio_is_headless_only() {
    let recorder = Arc::new(Recorder::default());
    let windowed = Dispatcher::new(Arc::clone(&recorder) as Arc<dyn Transport>, None, false);
    assert!(!windowed.is_granted("stdio"));

    let headless = Dispatcher::new(Arc::clone(&recorder) as Arc<dyn Transport>, None, true);
    assert!(headless.is_granted("stdio"));
}

#[tokio::test]
async fn malformed_messages_do_not_panic() {
    let (dispatcher, recorder) = dispatcher(None);
    dispatcher.handle_raw("{ not json").await;
    dispatcher.handle_raw(r#"{"t":"nonsense"}"#).await;
    assert_eq!(
        recorder.messages().len(),
        2,
        "each bad message gets one error"
    );
}

#[test]
fn protocol_round_trips_through_json() {
    let messages = [
        ClientMessage::Invoke {
            id: 1,
            method: "fs.read".into(),
            params: json!({"path":"/a"}),
        },
        ClientMessage::StreamStart {
            id: 2,
            method: "fs.watch".into(),
            params: Value::Null,
        },
        ClientMessage::StreamCancel { id: 2 },
        ClientMessage::Subscribe {
            id: 3,
            event: "os:theme".into(),
        },
        ClientMessage::Unsubscribe { id: 3 },
    ];
    for message in messages {
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&json).unwrap(),
            message
        );
    }
}

// --- the bridge transport the shim ---

/// The API catalog: "Absent permission, the module is not injected at all."
#[test]
fn shim_injects_only_granted_modules() {
    let granted = permissions(r#"{"fs":{"read":["~/x/**"]},"notifications":true}"#);
    let shim = render_shim(&ShimConfig::for_permissions("0.1.0", Some(&granted), false));

    // The module table is what the shim builds the global from.
    let table = shim
        .lines()
        .find(|l| l.contains("var MODULES ="))
        .expect("module table present");
    assert!(
        table.contains("\"fs\""),
        "granted fs must be present: {table}"
    );
    assert!(
        table.contains("\"notify\""),
        "granted notify must be present"
    );
    assert!(
        !table.contains("\"process\""),
        "ungranted process must be absent"
    );
    assert!(!table.contains("\"ffi\""), "ungranted ffi must be absent");
}

#[test]
fn shim_for_powerless_document_has_no_modules() {
    let shim = render_shim(&ShimConfig::for_permissions("0.1.0", None, false));
    let table = shim.lines().find(|l| l.contains("var MODULES =")).unwrap();
    assert!(
        table.contains("{}"),
        "expected an empty module table, got: {table}"
    );
    assert!(
        shim.contains("Object.freeze(htmlapp)"),
        "the bridge transport requires the global be frozen"
    );
}

#[test]
fn shim_marks_stream_methods_distinctly() {
    let granted = permissions(r#"{"fs":{"read":["~/x/**"]}}"#);
    let shim = render_shim(&ShimConfig::for_permissions("0.1.0", Some(&granted), false));
    let table = shim.lines().find(|l| l.contains("var MODULES =")).unwrap();
    assert!(
        table.contains(r#""read":"invoke""#),
        "fs.read is request/response"
    );
    assert!(
        table.contains(r#""watch":"stream""#),
        "fs.watch is a stream"
    );
}

#[test]
fn shim_substitutes_every_placeholder() {
    let shim = render_shim(&ShimConfig::for_permissions("1.2.3", None, true));
    assert!(
        !shim.contains("__HTMLAPP_"),
        "a placeholder survived rendering"
    );
    assert!(shim.contains(r#""1.2.3""#));
    assert!(shim.contains("var HEADLESS = true"));
}

/// A host→page payload must never be interpolated as source.
#[test]
fn dispatch_script_escapes_its_payload() {
    let hostile = r#"{"t":"result","id":1,"value":"</script><img onerror=alert(1)>\" + evil()"}"#;
    let script = htmlapp_bridge::dispatch_script(hostile);
    assert!(!script.contains("</script>"), "payload broke out: {script}");
    assert!(!script.contains('<'), "raw < survived escaping: {script}");
    assert!(script.starts_with("window.__htmlapp_dispatch"));
    assert!(
        script.contains(r"\u003c"),
        "< must be unicode-escaped: {script}"
    );

    // The escaping must be lossless: the page still parses the exact payload it was sent.
    let literal = script
        .trim_start_matches("window.__htmlapp_dispatch && window.__htmlapp_dispatch(")
        .trim_end_matches(");");
    let decoded: String = serde_json::from_str(literal).expect("still a valid JSON string literal");
    assert_eq!(decoded, hostile, "escaping changed the payload");
}

/// JS line terminators are legal inside JSON strings but historically hostile inside JS ones.
#[test]
fn dispatch_script_escapes_js_line_terminators() {
    let payload = "line\u{2028}separator\u{2029}here";
    let script = htmlapp_bridge::dispatch_script(payload);
    assert!(!script.contains('\u{2028}'), "U+2028 survived: {script}");
    assert!(!script.contains('\u{2029}'), "U+2029 survived: {script}");
}

// --- the type-safety promise TypeScript emit ---

#[test]
fn typescript_covers_the_whole_catalog() {
    let dts = emit_typescript();
    for module in htmlapp_bridge::catalog::MODULES {
        assert!(
            dts.contains(&format!("readonly {}?:", module.name)),
            "module `{}` missing from the global",
            module.name
        );
        for method in module.methods {
            assert!(
                dts.contains(&format!("  {}(", method.name)),
                "method `{}.{}` missing",
                module.name,
                method.name
            );
        }
    }
    // Every module is optional, because a grant decides whether it exists at runtime.
    assert!(dts.contains("readonly fs?: FsApi"));
    assert!(
        dts.contains("HtmlAppStream<WatchEvent>"),
        "streams are typed as streams"
    );
    assert!(
        dts.contains("Promise<FileStat>"),
        "invokes are typed as promises"
    );
}
