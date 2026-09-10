//! `--sandbox` (docs/security.md, rule 7) and user configuration (the open questions Q9).

use htmlapp_caps::Permissions;
use htmlapp_runtime::Config;
use htmlapp_runtime::sandbox;

fn permissions(json: &str) -> Permissions {
    serde_json::from_str(json).expect("test permissions must parse")
}

/// The jail binds what the manifest granted, and only that.
#[test]
fn sandbox_binds_granted_paths_and_nothing_more() {
    let granted = permissions(
        r#"{"fs":{"read":["/var/log/nginx/*.log"],"write":["/tmp/htmlapp-test/**"]}}"#,
    );
    let args = sandbox::arguments(Some(&granted), None);
    let joined = args.join(" ");

    // The literal prefix of each glob, since a glob cannot be expressed as a mount. These are
    // exact: a grant of `/var/log/nginx/*.log` must not put the whole of `/var/log` in the
    // namespace just because `nginx/` happens not to exist on this machine.
    assert!(joined.contains("--ro-bind-try /var/log/nginx"), "{joined}");
    assert!(!joined.contains("--ro-bind-try /var/log /var/log "), "over-bound /var/log: {joined}");
    assert!(joined.contains("--bind-try /tmp/htmlapp-test"), "{joined}");
    assert!(!joined.contains("--bind-try /tmp /tmp"), "over-bound /tmp: {joined}");

    // The whole point: home is not in the namespace.
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(
        !home.is_empty() && !args.iter().any(|a| a == &home),
        "the sandbox must not bind $HOME"
    );
    assert!(joined.contains("--unshare-pid"));
    assert!(joined.contains("--die-with-parent"));
}

/// A document that was granted nothing gets a jail with no user paths in it at all.
#[test]
fn sandbox_for_a_powerless_document_binds_no_user_paths() {
    let args = sandbox::arguments(None, None);
    let joined = args.join(" ");

    assert!(!joined.contains("--ro-bind-try"), "{joined}");
    assert!(!joined.contains("--bind-try"), "{joined}");
    // The system directories the runtime itself needs are still there, read-only.
    assert!(joined.contains("--ro-bind /usr"));
}

/// `/` must never be bound: that would be the opposite of a sandbox.
#[test]
fn sandbox_refuses_to_bind_the_root() {
    let granted = permissions(r#"{"fs":{"read":["/**"]}}"#);
    let args = sandbox::arguments(Some(&granted), None);

    let mut bound = Vec::new();
    for (index, flag) in args.iter().enumerate() {
        if flag.ends_with("bind-try")
            && let Some(path) = args.get(index + 1)
        {
            bound.push(path.clone());
        }
    }
    assert!(!bound.iter().any(|p| p == "/"), "bound / : {bound:?}");
}

#[test]
fn sandbox_reports_whether_it_can_run() {
    // Both branches are valid; what matters is that asking does not panic.
    let _ = sandbox::is_available();
    assert!(!sandbox::is_sandboxed(), "the test process is not in a jail");
}

// --- the open questions Q9: configuration ---

/// The defaults have to match the documented behaviour: a bare `htmlapp` opens the launcher.
#[test]
fn config_defaults_match_the_prd() {
    let config = Config::default();
    assert!(config.launcher, "the launch modes: a bare invocation opens the launcher");
    assert!(config.recents, "the open questions Q8: store-with-purge, so recording is on");
    assert!(!config.sandbox, "the security model, rule 7 is opt-in");
    assert!(!config.devtools);
}

#[test]
fn config_round_trips_through_toml() {
    let config = Config {
        launcher: false,
        recents: false,
        sandbox: true,
        devtools: true,
        color_scheme: Some("dark".into()),
    };

    let text = toml::to_string_pretty(&config).unwrap();
    let parsed: Config = toml::from_str(&text).unwrap();

    assert!(!parsed.launcher);
    assert!(!parsed.recents);
    assert!(parsed.sandbox);
    assert_eq!(parsed.color_scheme.as_deref(), Some("dark"));
}

/// A partial file keeps the defaults for everything it does not mention.
#[test]
fn config_fills_in_unspecified_keys() {
    let parsed: Config = toml::from_str("launcher = false").unwrap();
    assert!(!parsed.launcher);
    assert!(parsed.recents, "unspecified keys keep their defaults");
}

/// A typo must not be silently accepted — `launchre = false` would otherwise leave the launcher on
/// while the user believes they turned it off.
#[test]
fn config_rejects_unknown_keys() {
    assert!(toml::from_str::<Config>("launchre = false").is_err());
}
