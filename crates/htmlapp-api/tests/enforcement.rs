//! Enforcement at the call site (docs/api-reference.md and docs/security.md).
//!
//! `htmlapp-caps` is tested for whether the *policy* is right. These tests are about whether the
//! modules actually apply it.

use std::sync::Arc;

use htmlapp_api::context::ApiContext;
use htmlapp_api::fs::FsModule;
use htmlapp_api::os::OsModule;
use htmlapp_api::process::ProcessModule;
use htmlapp_api::shell::ShellModule;
use htmlapp_api::store::StoreModule;
use htmlapp_bridge::dispatch::ApiHandler;
use htmlapp_bridge::{ErrorCode, RpcError};
use htmlapp_caps::Permissions;
use serde_json::{Value, json};

fn ctx(permissions_json: &str) -> Arc<ApiContext> {
    let permissions: Permissions = serde_json::from_str(permissions_json).expect("permissions");
    Arc::new(ApiContext::new("dev.test.app", permissions).expect("context"))
}

async fn call(handler: &dyn ApiHandler, method: &str, params: Value) -> Result<Value, RpcError> {
    handler.invoke(method, params).await
}

// --- fs ---

#[tokio::test]
async fn fs_reads_inside_the_scope_and_refuses_outside_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let allowed = root.join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(allowed.join("ok.txt"), b"visible").unwrap();
    std::fs::write(root.join("secret.txt"), b"hidden").unwrap();

    let fs = FsModule::new(ctx(&format!(
        r#"{{"fs":{{"read":["{}/**"]}}}}"#,
        allowed.display()
    )), htmlapp_bridge::BlobStore::new());

    let inside = call(&fs, "read", json!({ "path": allowed.join("ok.txt") }))
        .await
        .expect("a granted path must be readable");
    assert_eq!(inside, json!("visible"));

    let outside = call(&fs, "read", json!({ "path": root.join("secret.txt") }))
        .await
        .expect_err("an ungranted path must be refused");
    assert_eq!(outside.code, ErrorCode::PermissionDenied);
}

/// The security model, rule 5, at the point the file is actually opened.
#[tokio::test]
async fn fs_refuses_to_read_through_a_symlink_out_of_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let allowed = root.join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(root.join("secret.txt"), b"hidden").unwrap();
    std::os::unix::fs::symlink(root.join("secret.txt"), allowed.join("link.txt")).unwrap();

    let fs = FsModule::new(ctx(&format!(
        r#"{{"fs":{{"read":["{}/**"]}}}}"#,
        allowed.display()
    )), htmlapp_bridge::BlobStore::new());

    let error = call(&fs, "read", json!({ "path": allowed.join("link.txt") }))
        .await
        .expect_err("reading through an escaping symlink must be refused");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
}

/// Read access must not imply write access.
#[tokio::test]
async fn fs_read_grant_does_not_confer_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    std::fs::write(root.join("a.txt"), b"original").unwrap();

    let fs = FsModule::new(ctx(&format!(
        r#"{{"fs":{{"read":["{}/**"]}}}}"#,
        root.display()
    )), htmlapp_bridge::BlobStore::new());

    let error = call(
        &fs,
        "write",
        json!({ "path": root.join("a.txt"), "contents": "changed" }),
    )
    .await
    .expect_err("a read grant must not allow writing");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "original");
}

/// A rename removes the source, so it needs write on both ends.
#[tokio::test]
async fn fs_rename_requires_write_on_both_ends() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let writable = root.join("writable");
    std::fs::create_dir_all(&writable).unwrap();
    std::fs::write(root.join("readonly.txt"), b"x").unwrap();

    let fs = FsModule::new(ctx(&format!(
        r#"{{"fs":{{"read":["{root}/**"],"write":["{writable}/**"]}}}}"#,
        root = root.display(),
        writable = writable.display()
    )), htmlapp_bridge::BlobStore::new());

    // Source is readable but not writable: refused.
    let error = call(
        &fs,
        "rename",
        json!({ "from": root.join("readonly.txt"), "to": writable.join("moved.txt") }),
    )
    .await
    .expect_err("renaming out of a read-only area must be refused");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    assert!(root.join("readonly.txt").exists());
}

/// `fs.list` must not leak entries the document may not read.
#[tokio::test]
async fn fs_list_omits_entries_outside_the_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    std::fs::write(root.join("visible.log"), b"x").unwrap();
    std::fs::write(root.join("secret.key"), b"x").unwrap();

    let fs = FsModule::new(ctx(&format!(
        r#"{{"fs":{{"read":["{}/*.log","{}"]}}}}"#,
        root.display(),
        root.display()
    )), htmlapp_bridge::BlobStore::new());

    let listed = call(&fs, "list", json!({ "path": root.clone() })).await.unwrap();
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(names.contains(&"visible.log"));
    assert!(!names.contains(&"secret.key"), "listed an ungranted file: {names:?}");
}

/// A document with no `fs` grant reaches nothing, even for a world-readable path.
#[tokio::test]
async fn fs_without_a_grant_reaches_nothing() {
    let fs = FsModule::new(ctx("{}"), htmlapp_bridge::BlobStore::new());
    let error = call(&fs, "read", json!({ "path": "/etc/hostname" }))
        .await
        .expect_err("an ungranted document must not read anything");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
}

// --- process ---

/// A grant of `rg` must not be satisfiable by a path that merely ends in `rg`.
#[tokio::test]
async fn process_allow_list_distinguishes_a_name_from_a_path() {
    let process = ProcessModule::new(ctx(r#"{"process":{"exec":["echo"]}}"#));

    let allowed = call(
        &process,
        "exec",
        json!({ "program": "echo", "args": ["hello"] }),
    )
    .await
    .expect("an allow-listed program must run");
    assert_eq!(allowed["stdout"], json!("hello\n"));
    assert_eq!(allowed["code"], json!(0));

    for impostor in ["/tmp/echo", "./echo", "../bin/echo", "sh"] {
        let error = call(&process, "exec", json!({ "program": impostor }))
            .await
            .expect_err("{impostor} must not satisfy a grant of `echo`");
        assert_eq!(
            error.code,
            ErrorCode::PermissionDenied,
            "{impostor} was allowed"
        );
    }
}

#[tokio::test]
async fn process_pty_needs_its_own_grant() {
    let process = ProcessModule::new(ctx(r#"{"process":{"exec":["echo"],"pty":false}}"#));
    // `ValueStream` is not Debug, so this cannot use `expect_err`.
    match process.open_stream("pty", json!({ "program": "echo" })).await {
        Err(error) => assert_eq!(error.code, ErrorCode::PermissionDenied),
        Ok(_) => panic!("pty must need `process.pty`"),
    }
}

/// A page must not be able to kill a process it did not start.
#[tokio::test]
async fn process_cannot_signal_a_process_it_did_not_start() {
    let process = ProcessModule::new(ctx(r#"{"process":{"exec":["echo"]}}"#));
    let error = call(&process, "kill", json!({ "pid": 1 }))
        .await
        .expect_err("killing pid 1 must be refused");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
}

#[tokio::test]
async fn process_streams_output_then_exits() {
    use futures::StreamExt as _;

    let process = ProcessModule::new(ctx(r#"{"process":{"exec":["printf"]}}"#));
    let stream = process
        .open_stream(
            "spawn",
            json!({ "program": "printf", "args": ["one\ntwo\n"] }),
        )
        .await
        .expect("spawn");

    let events: Vec<Value> = stream.filter_map(|e| async { e.ok() }).collect().await;
    let kinds: Vec<&str> = events.iter().filter_map(|e| e["kind"].as_str()).collect();

    assert_eq!(kinds.first(), Some(&"started"));
    assert_eq!(kinds.last(), Some(&"exit"), "stream must report the exit");
    let output: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "stdout")
        .filter_map(|e| e["data"].as_str())
        .collect();
    assert_eq!(output, ["one", "two"], "output must not be lost to the exit race");
}

// --- os ---

/// Environment variables routinely hold credentials.
#[tokio::test]
async fn os_env_withholds_likely_credentials() {
    let os = OsModule::new(ctx(r#"{"os":true}"#));

    for name in ["GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "MY_API_KEY", "DB_PASSWORD"] {
        let error = call(&os, "env", json!({ "name": name }))
            .await
            .expect_err("{name} should be withheld");
        assert_eq!(error.code, ErrorCode::PermissionDenied, "{name} was returned");
    }

    // Ordinary variables still work.
    call(&os, "env", json!({ "name": "HOME" })).await.unwrap();
}

// --- shell ---

/// `shell.open` hands a target to a desktop handler, so the schemes it accepts are limited.
#[tokio::test]
async fn shell_open_refuses_unexpected_schemes() {
    let shell = ShellModule::new(ctx(r#"{"shell":true}"#));

    for hostile in ["file:///etc/passwd", "javascript:alert(1)", "ssh://host/x"] {
        let error = call(&shell, "open", json!({ "target": hostile }))
            .await
            .expect_err("{hostile} must be refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied, "{hostile} allowed");
    }
}

/// A bare path passed to `shell.open` is a filesystem reference and is scoped like one.
#[tokio::test]
async fn shell_open_scopes_bare_paths() {
    let shell = ShellModule::new(ctx(r#"{"shell":true}"#));
    let error = call(&shell, "open", json!({ "target": "/etc/passwd" }))
        .await
        .expect_err("an unscoped path must be refused");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
}

// --- store ---

#[tokio::test]
async fn store_round_trips_and_is_scoped_to_the_app() {
    let store = StoreModule::new(ctx(r#"{"store":true}"#));

    call(&store, "set", json!({ "key": "theme", "value": "dark" }))
        .await
        .unwrap();
    assert_eq!(
        call(&store, "get", json!({ "key": "theme" })).await.unwrap(),
        json!("dark")
    );
    assert_eq!(
        call(&store, "keys", Value::Null).await.unwrap(),
        json!(["theme"])
    );

    call(&store, "delete", json!({ "key": "theme" })).await.unwrap();
    assert_eq!(
        call(&store, "get", json!({ "key": "theme" })).await.unwrap(),
        Value::Null
    );
}

// --- origin matching (docs/security.md) ---

#[test]
fn url_matching_enforces_scheme_host_and_path() {
    use htmlapp_api::context::url_matches;

    assert!(url_matches("https://api.example.com/*", "https://api.example.com/v1/x"));
    assert!(!url_matches("https://api.example.com/*", "http://api.example.com/v1/x"));
    assert!(!url_matches("https://api.example.com/*", "https://evil.example/x"));

    // Suffix confusion, again at this layer.
    assert!(url_matches("https://*.example.com/**", "https://api.example.com/x"));
    assert!(!url_matches("https://*.example.com/**", "https://notexample.com/x"));

    // A path glob really does narrow.
    assert!(url_matches("https://h.example/v1/*", "https://h.example/v1/users"));
    assert!(!url_matches("https://h.example/v1/*", "https://h.example/v2/users"));
}

// --- blob tokens (docs/bridge.md) ---

/// `/dev/urandom` is an endless stream. Anything that reads it to EOF never returns, so this is a
/// regression test for a hang, not just for entropy.
#[test]
fn random_token_returns_promptly_and_is_unpredictable() {
    let started = std::time::Instant::now();
    let first = htmlapp_api::random_token();
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "minting a token took {elapsed:?}; it must not read /dev/urandom to EOF"
    );
    assert_eq!(first.len(), 32, "16 bytes, hex-encoded");
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));

    // A blob URL is a capability; a predictable token would be a way to forge one.
    let tokens: std::collections::HashSet<String> =
        (0..64).map(|_| htmlapp_api::random_token()).collect();
    assert_eq!(tokens.len(), 64, "tokens must not repeat");
}

#[tokio::test]
async fn fs_blob_only_mints_tokens_for_granted_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let allowed = root.join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::write(allowed.join("ok.bin"), b"bytes").unwrap();
    std::fs::write(root.join("secret.bin"), b"nope").unwrap();

    let blobs = htmlapp_bridge::BlobStore::new();
    let fs = htmlapp_api::fs::FsModule::new(
        ctx(&format!(r#"{{"fs":{{"read":["{}/**"]}}}}"#, allowed.display())),
        blobs.clone(),
    );

    let url = call(&fs, "blob", json!({ "path": allowed.join("ok.bin") }))
        .await
        .expect("a granted path should mint a token");
    assert!(url.as_str().unwrap().starts_with("htmlapp://app/__blob__/"));
    assert_eq!(blobs.len(), 1);

    // An ungranted path must not become fetchable by minting a URL for it.
    let denied = call(&fs, "blob", json!({ "path": root.join("secret.bin") }))
        .await
        .expect_err("an ungranted path must not mint a token");
    assert_eq!(denied.code, ErrorCode::PermissionDenied);
    assert_eq!(blobs.len(), 1, "no token should have been added");
}
