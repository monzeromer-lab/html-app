//! The enforcement half of the security model.

use std::fs;
use std::path::Path;

use htmlapp_caps::consent::ConsentDecision;
use htmlapp_caps::{ConsentStore, Manifest, PathScope, PermissionDiff, Permissions, Risk};

// --- the security model, rule 5: path scoping with symlink resolution ---

/// The escape the PRD names explicitly: "`~/logs/link-to-etc` cannot escape".
#[test]
fn symlink_cannot_escape_the_granted_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();

    let logs = root.join("logs");
    let secrets = root.join("secrets");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&secrets).unwrap();
    fs::write(logs.join("app.log"), b"ok").unwrap();
    fs::write(secrets.join("passwd"), b"secret").unwrap();

    // The document is granted only ~/logs/**.
    let scope = PathScope::new([format!("{}/**", logs.display())]).unwrap();

    // A real file inside the scope is fine.
    assert!(scope.allows(logs.join("app.log")));

    // Now plant the symlink the PRD calls out.
    let link = logs.join("link-to-secrets");
    std::os::unix::fs::symlink(&secrets, &link).unwrap();

    // Lexically this is "inside ~/logs/". It must still be refused, because it resolves outside.
    let escaped = link.join("passwd");
    assert!(
        !scope.allows(&escaped),
        "symlink traversal escaped the granted scope: {}",
        escaped.display()
    );

    let denied = scope.check(&escaped).expect_err("must be denied");
    assert_eq!(denied.resolved, secrets.join("passwd"));
}

/// `..` must not walk out of the scope either.
#[test]
fn dotdot_cannot_escape_the_granted_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let logs = root.join("logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(root.join("outside.txt"), b"nope").unwrap();

    let scope = PathScope::new([format!("{}/**", logs.display())]).unwrap();
    assert!(!scope.allows(logs.join("../outside.txt")));
}

/// A write target that does not exist yet must still be checked — and must still resolve symlinks
/// on the part of the path that does exist.
#[test]
fn nonexistent_write_target_is_checked_through_existing_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let data = root.join("data");
    let elsewhere = root.join("elsewhere");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();

    let scope = PathScope::new([format!("{}/**", data.display())]).unwrap();

    // Does not exist yet, but is genuinely inside the scope.
    assert!(scope.allows(data.join("new-file.txt")));

    // Does not exist yet, and is behind a symlink pointing out of the scope.
    std::os::unix::fs::symlink(&elsewhere, data.join("out")).unwrap();
    assert!(!scope.allows(data.join("out").join("new-file.txt")));
}

/// The security model, rule 1, at the enforcement layer: an empty scope grants nothing.
#[test]
fn empty_scope_denies_everything() {
    let scope = PathScope::empty();
    assert!(scope.is_empty());
    assert!(!scope.allows("/etc/passwd"));
    assert!(!scope.allows("/tmp"));
    assert!(!scope.allows("/"));
}

#[test]
fn scope_matches_the_prd_glob_shapes() {
    let scope = PathScope::new(["/var/log/nginx/*.log"]).unwrap();
    assert!(scope.allows("/var/log/nginx/access.log"));
    assert!(!scope.allows("/var/log/nginx/deep/access.log"));
    assert!(!scope.allows("/var/log/syslog"));
}

// --- the threat model: exfiltration via `http` ---

/// "Origin allow-list; no wildcard `*` accepted."
#[test]
fn wildcard_origins_are_refused() {
    for bad in ["*", "*://*", "https://*", "http://*", "https://*/*", ""] {
        let json = format!(r#"{{"permissions":{{"net":{{"fetch":["{bad}"]}}}}}}"#);
        assert!(
            Manifest::from_json(&json).is_err(),
            "wildcard origin {bad:?} must be refused"
        );
    }
}

#[test]
fn specific_origins_are_accepted() {
    let json = r#"{"permissions":{"net":{"fetch":["https://alerts.internal.corp/*"]}}}"#;
    assert!(Manifest::from_json(json).is_ok());
}

// --- the API catalog: ungranted modules are absent, not merely disabled ---

#[test]
fn ungranted_modules_are_absent() {
    let permissions = Permissions::default();
    assert!(permissions.is_empty());
    assert!(
        permissions.granted_modules().is_empty(),
        "a default permission set must inject nothing at all"
    );
}

// --- the security model, rule 3: hash-pinned consent ---

#[test]
fn consent_is_pinned_to_content_and_reprompts_on_edit() {
    let mut store = ConsentStore::default();
    let permissions: Permissions =
        serde_json::from_str(r#"{"fs":{"read":["~/logs/**"]}}"#).unwrap();
    let path = Path::new("/home/u/tool.hta");

    // First run of unknown content: prompt.
    assert!(matches!(
        store.decide("hash-v1", Some(path), Some(&permissions)),
        ConsentDecision::NeedsPrompt { previous: None, .. }
    ));

    store.record(
        "hash-v1",
        Some(path),
        Some("Tool"),
        None,
        &permissions,
        true,
    );

    // Same content again: no prompt.
    assert!(matches!(
        store.decide("hash-v1", Some(path), Some(&permissions)),
        ConsentDecision::AlreadyGranted(_)
    ));

    // The file is edited to ask for more. The threat model "trojan update to a trusted file".
    let escalated: Permissions =
        serde_json::from_str(r#"{"fs":{"read":["~/logs/**"]},"process":{"exec":["sh"]}}"#).unwrap();
    match store.decide("hash-v2", Some(path), Some(&escalated)) {
        ConsentDecision::NeedsPrompt { previous, diff } => {
            assert!(
                previous.is_some(),
                "the sheet must show what was there before"
            );
            assert!(diff.is_escalation());
            assert_eq!(diff.added, vec!["process".to_string()]);
        }
        other => panic!("edited file must re-prompt, got {other:?}"),
    }
}

/// A stored record must never be able to grant more than the file currently asks for.
#[test]
fn stored_record_cannot_widen_a_grant() {
    let mut store = ConsentStore::default();
    let narrow: Permissions = serde_json::from_str(r#"{"fs":{"read":["~/logs/**"]}}"#).unwrap();
    store.record("h", None, None, None, &narrow, true);

    // Same hash, but the manifest now asks for something else entirely. Tampering with the store,
    // or a hash collision, must not shortcut the prompt.
    let wide: Permissions = serde_json::from_str(r#"{"fs":{"read":["/**"]}}"#).unwrap();
    assert!(matches!(
        store.decide("h", None, Some(&wide)),
        ConsentDecision::NeedsPrompt { .. }
    ));
}

/// The security model, rule 1: nothing requested means nothing to consent to.
#[test]
fn powerless_document_needs_no_consent() {
    let store = ConsentStore::default();
    assert_eq!(store.decide("h", None, None), ConsentDecision::NotRequired);
    assert_eq!(
        store.decide("h", None, Some(&Permissions::default())),
        ConsentDecision::NotRequired
    );
}

/// The security model, rule 6: revocable.
#[test]
fn consent_is_revocable() {
    let mut store = ConsentStore::default();
    let permissions: Permissions = serde_json::from_str(r#"{"store":true}"#).unwrap();
    let path = Path::new("/home/u/a.hta");

    store.record("h1", Some(path), None, None, &permissions, true);
    store.record("h2", Some(path), None, None, &permissions, true);
    assert_eq!(store.records.len(), 2);

    // Revoking by path clears every version of that document.
    assert_eq!(store.revoke_path(path), 2);
    assert!(store.records.is_empty());
    assert!(matches!(
        store.decide("h1", Some(path), Some(&permissions)),
        ConsentDecision::NeedsPrompt { .. }
    ));
}

#[test]
fn consent_store_round_trips_through_disk() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested").join("consent.json");

    let mut store = ConsentStore::default();
    let permissions: Permissions =
        serde_json::from_str(r#"{"fs":{"read":["~/x/**"]},"notifications":true}"#).unwrap();
    store.record(
        "abc",
        Some(Path::new("/t.hta")),
        Some("T"),
        None,
        &permissions,
        true,
    );
    store.save(&path).unwrap();

    let reloaded = ConsentStore::load(&path).unwrap();
    assert_eq!(reloaded.records.len(), 1);
    assert_eq!(reloaded.find("abc").unwrap().permissions, permissions);

    // A missing store is an empty store, not an error.
    assert!(
        ConsentStore::load(temp.path().join("absent.json"))
            .unwrap()
            .records
            .is_empty()
    );
}

// --- the consent sheet's plain-language copy (docs/security.md, rule 3) ---

#[test]
fn unrestricted_exec_and_ffi_are_flagged_as_extreme() {
    let permissions: Permissions =
        serde_json::from_str(r#"{"process":{"exec":[]},"ffi":["libc.so.6"]}"#).unwrap();
    let described = permissions.describe();

    // The API catalog: ffi is "the loudest permission in the system" — it must lead.
    assert_eq!(described[0].module, "ffi");
    assert_eq!(described[0].risk, Risk::Extreme);
    // An empty exec allow-list means *any* program, which is not a mild grant.
    let process = described.iter().find(|d| d.module == "process").unwrap();
    assert_eq!(process.risk, Risk::Extreme);
}

#[test]
fn descriptions_are_ordered_most_alarming_first() {
    let permissions: Permissions = serde_json::from_str(
        r#"{"notifications":true,"fs":{"read":["~/a/**"],"write":["~/b/**"]},
            "process":{"exec":["rg"]}}"#,
    )
    .unwrap();
    let risks: Vec<_> = permissions.describe().iter().map(|d| d.risk).collect();
    assert!(
        risks.windows(2).all(|w| w[0] >= w[1]),
        "risk order regressed: {risks:?}"
    );
}

#[test]
fn diff_reports_narrowing_as_well_as_widening() {
    let old: Permissions =
        serde_json::from_str(r#"{"fs":{"read":["~/a/**"]},"process":{"exec":["rg"]}}"#).unwrap();
    let new: Permissions = serde_json::from_str(r#"{"fs":{"read":["~/a/**","~/b/**"]}}"#).unwrap();

    let diff = PermissionDiff::between(Some(&old), &new);
    assert_eq!(diff.removed, vec!["process".to_string()]);
    assert_eq!(diff.changed, vec!["fs".to_string()], "fs scope widened");
    assert!(
        diff.is_escalation(),
        "a widened fs scope is still an escalation"
    );
}
