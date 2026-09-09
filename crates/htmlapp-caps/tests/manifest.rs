//! Manifest location and parsing (PRD §8).

use htmlapp_caps::{Document, Manifest, WindowMode, extract_manifest_json};

/// The exact manifest printed in PRD §8.2 must parse, field for field.
#[test]
fn prd_example_manifest_parses() {
    let json = r#"{
      "name": "Log Triage",
      "id": "dev.monzer.logtriage",
      "version": "1.2.0",
      "icon": "data:image/svg+xml;base64,AAAA",
      "window": {
        "title": "Log Triage",
        "width": 1200, "height": 800,
        "min_width": 640,
        "titlebar": "native",
        "background": "transparent",
        "resizable": true
      },
      "permissions": {
        "fs": {
          "read":  ["~/logs/**", "/var/log/nginx/*.log"],
          "write": ["~/.local/share/logtriage/**"]
        },
        "process": { "exec": ["rg", "journalctl"], "pty": false },
        "net": { "fetch": ["https://alerts.internal.corp/*"] },
        "sql": { "databases": ["~/.local/share/logtriage/index.db"] },
        "clipboard": ["read", "write"],
        "notifications": true
      }
    }"#;

    let manifest = Manifest::from_json(json).expect("PRD §8.2 example must parse");
    assert_eq!(manifest.name.as_deref(), Some("Log Triage"));
    assert_eq!(manifest.id.as_deref(), Some("dev.monzer.logtriage"));
    assert_eq!(manifest.window.width, 1200);
    assert_eq!(manifest.window.min_width, Some(640));
    assert_eq!(manifest.window.mode, WindowMode::Window);

    let permissions = manifest.permissions.expect("permissions present");
    let modules = permissions.granted_modules();
    assert!(modules.contains("fs"));
    assert!(modules.contains("process"));
    assert!(modules.contains("http"));
    assert!(modules.contains("sql"));
    assert!(modules.contains("clipboard"));
    assert!(modules.contains("notify"));
    // Never requested, so §9.3 says the module must not exist at all.
    assert!(!modules.contains("ffi"));
    assert!(!modules.contains("dbus"));
}

/// The §8.3 layer-shell block.
#[test]
fn layer_mode_parses() {
    let json = r#"{ "window": { "mode": "layer", "layer": "top",
                                "anchor": ["top", "left", "right"],
                                "exclusive_zone": 34,
                                "keyboard_interactivity": "on-demand",
                                "output": "primary" } }"#;
    let manifest = Manifest::from_json(json).unwrap();
    assert_eq!(manifest.window.mode, WindowMode::Layer);
    assert_eq!(manifest.window.exclusive_zone, Some(34));
    assert_eq!(manifest.window.anchor.len(), 3);
    assert!(manifest.window.mode.needs_wayland_shell());
}

#[test]
fn every_window_mode_round_trips() {
    for (text, mode) in [
        ("window", WindowMode::Window),
        ("layer", WindowMode::Layer),
        ("lock", WindowMode::Lock),
        ("tray", WindowMode::Tray),
        ("headless", WindowMode::Headless),
    ] {
        let manifest =
            Manifest::from_json(&format!(r#"{{"window":{{"mode":"{text}"}}}}"#)).unwrap();
        assert_eq!(manifest.window.mode, mode);
        assert_eq!(mode.as_str(), text);
    }
    assert!(!WindowMode::Headless.has_surface());
    assert!(WindowMode::Window.has_surface());
}

// --- §8.1: finding the block ---

#[test]
fn finds_manifest_regardless_of_quoting_and_attribute_order() {
    let cases = [
        r#"<script type="application/htmlapp+json">{"name":"A"}</script>"#,
        r#"<script type='application/htmlapp+json'>{"name":"A"}</script>"#,
        r#"<script type=application/htmlapp+json>{"name":"A"}</script>"#,
        r#"<script id="m" type="application/htmlapp+json">{"name":"A"}</script>"#,
        r#"<SCRIPT TYPE="APPLICATION/HTMLAPP+JSON">{"name":"A"}</SCRIPT>"#,
        "<script\n  type=\"application/htmlapp+json\"\n>{\"name\":\"A\"}</script>",
    ];
    for html in cases {
        let json = extract_manifest_json(html)
            .unwrap_or_else(|| panic!("should have found a manifest in: {html}"));
        assert_eq!(
            Manifest::from_json(&json).unwrap().name.as_deref(),
            Some("A"),
            "in: {html}"
        );
    }
}

#[test]
fn ignores_ordinary_scripts_and_lookalikes() {
    let cases = [
        r#"<script>var x = 1;</script>"#,
        r#"<script type="module">import "x";</script>"#,
        r#"<script type="application/json">{"name":"A"}</script>"#,
        r#"<scriptfoo type="application/htmlapp+json">{"name":"A"}</scriptfoo>"#,
        r#"<script type="application/htmlapp+json2">{"name":"A"}</script>"#,
    ];
    for html in cases {
        assert!(
            extract_manifest_json(html).is_none(),
            "should NOT have matched: {html}"
        );
    }
}

#[test]
fn skips_preceding_scripts_to_find_the_manifest() {
    let html = r#"
        <script>console.log("hi")</script>
        <script type="module">export {}</script>
        <script type="application/htmlapp+json">{"name":"Third"}</script>
    "#;
    let json = extract_manifest_json(html).unwrap();
    assert_eq!(
        Manifest::from_json(&json).unwrap().name.as_deref(),
        Some("Third")
    );
}

/// §8.1 and §11.2 rule 1: no manifest is a valid, silent, powerless document — not an error.
#[test]
fn document_without_manifest_is_powerless_not_an_error() {
    let html = b"<!DOCTYPE html><html><body><h1>Just a page</h1></body></html>";
    let document = Document::from_bytes(html).expect("must not error");
    assert!(!document.has_manifest);
    assert!(document.manifest.is_powerless());
    assert!(document.manifest.permissions.is_none());
}

/// §11.2 rule 3: the consent key is the hash of the file's content.
#[test]
fn hash_tracks_content_not_path() {
    let a = Document::from_bytes(b"<html>one</html>").unwrap();
    let b = Document::from_bytes(b"<html>one</html>").unwrap();
    let c = Document::from_bytes(b"<html>two</html>").unwrap();
    assert_eq!(a.hash, b.hash, "identical content must hash identically");
    assert_ne!(a.hash, c.hash, "edited content must hash differently");
    assert_eq!(a.hash.len(), 64, "sha256 hex is 64 chars");
}

/// App ids become path components for the store and consent record.
#[test]
fn traversal_in_app_id_is_rejected() {
    for bad in ["../../etc", "..", ".hidden", "trailing.", "has/slash", ""] {
        let json = format!(r#"{{"id":"{bad}"}}"#);
        assert!(
            Manifest::from_json(&json).is_err(),
            "id {bad:?} should have been rejected"
        );
    }
    assert!(Manifest::from_json(r#"{"id":"dev.monzer.log-triage_2"}"#).is_ok());
}

/// §8.5 / §11.3: an unpinned import is a supply-chain hole, so it fails to parse.
#[test]
fn imports_must_declare_integrity() {
    let missing = r#"{"imports":{"d3":{"url":"https://esm.sh/d3@7","integrity":""}}}"#;
    assert!(Manifest::from_json(missing).is_err());

    let pinned = r#"{"imports":{"d3":{"url":"https://esm.sh/d3@7","integrity":"sha384-abc"}}}"#;
    let manifest = Manifest::from_json(pinned).unwrap();
    assert_eq!(manifest.imports["d3"].integrity, "sha384-abc");
}
