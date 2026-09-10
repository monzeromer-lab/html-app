//! Stapling and desktop integration (docs/building.md).

use htmlapp_build::desktop;
use htmlapp_build::staple;

/// Packaging and distribution: the end user gets one file and never learns the word "HTML App".
#[test]
fn stapled_document_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = temp.path().join("runtime");
    // Stand-in for the runtime binary; stapling only ever appends to it.
    std::fs::write(&runtime, b"\x7fELF-not-really-but-close-enough").unwrap();

    let document = b"<html><head><script type=\"application/htmlapp+json\">{\"name\":\"T\"}</script></head></html>";
    let output = temp.path().join("tool");
    staple::staple(&runtime, document, &output).unwrap();

    let extracted = staple::extract(&output)
        .unwrap()
        .expect("a stapled document");
    assert_eq!(extracted, document);

    // The whole point is that it can be executed.
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(&output).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "the built binary must be executable");
}

/// An ordinary runtime binary must report "no document" rather than failing — this runs on every
/// single startup.
#[test]
fn unstapled_binary_reports_no_document() {
    let temp = tempfile::tempdir().unwrap();
    let plain = temp.path().join("runtime");
    std::fs::write(&plain, b"just a binary, no trailer here").unwrap();
    assert!(staple::extract(&plain).unwrap().is_none());

    // Shorter than the trailer itself.
    let tiny = temp.path().join("tiny");
    std::fs::write(&tiny, b"x").unwrap();
    assert!(staple::extract(&tiny).unwrap().is_none());
}

/// A binary that merely *contains* the magic bytes is not a stapled binary.
#[test]
fn magic_bytes_in_the_payload_do_not_confuse_extraction() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = temp.path().join("runtime");
    // The magic appears in the runtime's own body, as it genuinely does — the constant is
    // compiled into the real binary.
    let mut body = b"prefix".to_vec();
    body.extend_from_slice(staple::MAGIC);
    body.extend_from_slice(b"suffix");
    std::fs::write(&runtime, &body).unwrap();

    assert!(
        staple::extract(&runtime).unwrap().is_none(),
        "magic in the body must not be read as a trailer"
    );

    // And a genuinely stapled copy of that same runtime still works.
    let output = temp.path().join("tool");
    staple::staple(&runtime, b"<html>real</html>", &output).unwrap();
    assert_eq!(
        staple::extract(&output).unwrap().unwrap(),
        b"<html>real</html>"
    );
}

#[test]
fn empty_document_staples_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = temp.path().join("runtime");
    std::fs::write(&runtime, b"binary").unwrap();
    let output = temp.path().join("tool");

    staple::staple(&runtime, b"", &output).unwrap();
    assert_eq!(staple::extract(&output).unwrap(), Some(Vec::new()));
}

// --- desktop integration ---

/// Desktop integration: "`%f` is the whole mechanism" — one entry serves both the menu and the file manager.
#[test]
fn desktop_entry_matches_the_prd() {
    let entry = desktop::runtime_desktop_entry("htmlapp");

    assert!(entry.contains("Exec=htmlapp %f"), "the %f is load-bearing");
    assert!(entry.contains("MimeType=application/hta;application/x-hta;"));
    assert!(entry.contains("StartupWMClass=htmlapp"));
    assert!(entry.contains("Actions=OpenFile;Permissions;"));
    assert!(entry.contains("Exec=htmlapp --open"));
    assert!(entry.contains("Exec=htmlapp --permissions-ui"));
    assert!(entry.contains("Categories=Development;Utility;"));
}

/// Packaging and distribution: ".html is deliberately not hijacked."
#[test]
fn mime_registration_claims_hta_only() {
    let package = desktop::mime_package();
    assert!(package.contains(r#"<glob pattern="*.hta"/>"#));
    assert!(
        !package.contains(r#"pattern="*.html""#),
        "hijacking .html would break every browser on the system"
    );
    assert!(package.contains("application/hta"));
    assert!(package.contains("application/x-hta"));
}

/// The process and instance model: an app built with `htmlapp build` "never routes through any of this".
#[test]
fn built_app_entry_does_not_claim_the_hta_type() {
    let manifest =
        htmlapp_caps::Manifest::from_json(r#"{"name":"My Tool","id":"com.example.tool"}"#).unwrap();
    let entry = desktop::app_desktop_entry(&manifest, std::path::Path::new("/opt/tool"), "tool");

    assert!(entry.contains("Name=My Tool"));
    assert!(entry.contains("Exec=/opt/tool %f"));
    assert!(
        !entry.contains("MimeType="),
        "a built app must not take over the .hta association"
    );
}

#[test]
fn manifest_icon_is_extracted() {
    let temp = tempfile::tempdir().unwrap();
    // A one-pixel SVG, base64'd.
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#;
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(svg);
    let manifest = htmlapp_caps::Manifest::from_json(&format!(
        r#"{{"name":"T","icon":"data:image/svg+xml;base64,{encoded}"}}"#
    ))
    .unwrap();

    let written = desktop::write_manifest_icon(&manifest, &temp.path().join("tool"))
        .expect("the icon should be extracted");
    assert_eq!(written.extension().unwrap(), "svg");
    assert_eq!(std::fs::read_to_string(&written).unwrap(), svg);
}

/// A remote icon URL would mean fetching from wherever the manifest points, which the goals and non-goals, N5 rules out.
#[test]
fn remote_icon_url_is_not_fetched() {
    let temp = tempfile::tempdir().unwrap();
    let manifest =
        htmlapp_caps::Manifest::from_json(r#"{"name":"T","icon":"https://example.com/icon.png"}"#)
            .unwrap();
    assert!(desktop::write_manifest_icon(&manifest, &temp.path().join("tool")).is_none());
}
