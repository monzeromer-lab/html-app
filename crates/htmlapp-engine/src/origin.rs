//! The `htmlapp://app/` origin (docs/document-format.md) and the injected CSP (docs/security.md, rule 8).
//!
//! Serving the document over a custom scheme rather than `file://` gives it a real, stable origin.
//! That is what makes `localStorage`, IndexedDB, service workers, ES module imports, and a
//! meaningful CSP work at all — `file://` origins are opaque, and most of that silently degrades.

use std::path::{Path, PathBuf};

use htmlapp_caps::{Document, Manifest};

/// The scheme the document is served over.
pub const SCHEME: &str = "htmlapp";

/// The full origin. Everything the page loads without an explicit origin resolves under this.
pub const ORIGIN: &str = "htmlapp://app";

/// The URL the document itself is loaded from.
pub const INDEX_URL: &str = "htmlapp://app/index.html";

/// What the custom-protocol handler decided to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Ok {
        body: Vec<u8>,
        content_type: String,
    },
    NotFound,
    /// The request resolved outside what the asset policy permits.
    Forbidden,
}

/// Resolves `htmlapp://app/...` requests against the document and its sibling assets.
#[derive(Debug, Clone)]
pub struct OriginResolver {
    /// The prepared HTML, served for `/` and `/index.html`.
    index: String,
    /// `Some` only when the manifest opted into `"assets": "sibling"` (docs/document-format.md).
    asset_root: Option<PathBuf>,
    /// Cached modules fetched through the import map (docs/document-format.md).
    module_root: Option<PathBuf>,
    /// Paths the page may fetch directly by token (docs/bridge.md).
    blobs: htmlapp_bridge::BlobStore,
}

impl OriginResolver {
    pub fn new(index: String, asset_root: Option<PathBuf>, module_root: Option<PathBuf>) -> Self {
        Self::with_blobs(
            index,
            asset_root,
            module_root,
            htmlapp_bridge::BlobStore::new(),
        )
    }

    pub fn with_blobs(
        index: String,
        asset_root: Option<PathBuf>,
        module_root: Option<PathBuf>,
        blobs: htmlapp_bridge::BlobStore,
    ) -> Self {
        Self {
            index,
            asset_root,
            module_root,
            blobs,
        }
    }

    /// Serve one request path, e.g. `/index.html` or `/style.css`.
    pub fn resolve(&self, request_path: &str) -> Response {
        let path = request_path.split(['?', '#']).next().unwrap_or("/");
        let path = path.trim_start_matches('/');

        if path.is_empty() || path == "index.html" {
            return Response::Ok {
                body: self.index.clone().into_bytes(),
                content_type: "text/html; charset=utf-8".into(),
            };
        }

        // A blob token already carries its own authorisation: it was only minted for a path that
        // passed a scope check, and it is unguessable. So no further check happens here — the
        // check happened when `fs.blob` handed the URL out.
        if let Some(token) = htmlapp_bridge::BlobStore::token_from_path(path) {
            return match self.blobs.get(token) {
                Some(blob) => match std::fs::read(&blob) {
                    Ok(body) => Response::Ok {
                        body,
                        content_type: guess_content_type(&blob),
                    },
                    Err(_) => Response::NotFound,
                },
                None => Response::NotFound,
            };
        }

        if let Some(module) = path.strip_prefix("__modules__/") {
            return match &self.module_root {
                Some(root) => serve_file(root, module, true),
                None => Response::NotFound,
            };
        }

        // The origin and loading rules: the default is strictly single-file. Without the opt-in there is nothing else here.
        match &self.asset_root {
            Some(root) => serve_file(root, path, false),
            None => Response::Forbidden,
        }
    }
}

/// Read a file from an allowed root, refusing anything that escapes it.
///
/// The containment check is done on the canonicalised path, so neither `..` nor a symlink planted
/// beside the document can reach outside the asset root.
fn serve_file(root: &Path, relative: &str, is_module: bool) -> Response {
    let decoded = percent_decode(relative);
    if decoded.contains('\0') {
        return Response::Forbidden;
    }

    let candidate = root.join(&decoded);
    let Ok(canonical) = candidate.canonicalize() else {
        return Response::NotFound;
    };
    let Ok(canonical_root) = root.canonicalize() else {
        return Response::NotFound;
    };
    if !canonical.starts_with(&canonical_root) {
        return Response::Forbidden;
    }

    match std::fs::read(&canonical) {
        Ok(body) => {
            let content_type = if is_module {
                "text/javascript; charset=utf-8".to_string()
            } else {
                guess_content_type(&canonical)
            };
            Response::Ok { body, content_type }
        }
        Err(_) => Response::NotFound,
    }
}

/// Minimal percent-decoding for request paths.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn guess_content_type(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" | "hta" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" | "log" | "md" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Build the Content-Security-Policy the runtime injects (docs/security.md, rule 8).
///
/// `'unsafe-inline'` for scripts and styles is not an oversight: G1 says the input is one file with
/// no build step, so a single-file app's script *is* inline, and a nonce-based policy would mean
/// rewriting the author's document. The exfiltration control that actually matters here is
/// `connect-src`, which is pinned to the manifest's `net.fetch` allow-list and nothing else —
/// so a page can still run whatever it likes, but it cannot phone anywhere it was not granted.
pub fn default_csp(manifest: &Manifest) -> String {
    let mut connect = vec![ORIGIN.to_string(), "blob:".to_string(), "data:".to_string()];

    if let Some(net) = manifest.permissions.as_ref().and_then(|p| p.net.as_ref()) {
        for origin in &net.fetch {
            if let Some(source) = csp_source(origin) {
                connect.push(source);
            }
        }
    }

    // Remote modules are fetched by the host and re-served from this origin, so the page never
    // needs to reach the network for them itself.
    format!(
        "default-src {origin}; \
         script-src {origin} 'unsafe-inline' 'unsafe-eval' blob:; \
         style-src {origin} 'unsafe-inline'; \
         img-src {origin} data: blob:; \
         font-src {origin} data:; \
         media-src {origin} data: blob:; \
         connect-src {connect}; \
         worker-src {origin} blob:; \
         frame-src 'none'; \
         object-src 'none'; \
         base-uri 'none'; \
         form-action 'none'; \
         frame-ancestors 'none'",
        origin = ORIGIN,
        connect = connect.join(" "),
    )
}

/// Reduce a manifest `net.fetch` glob to a CSP source expression.
///
/// A CSP source is host-and-scheme granular; it cannot express a path glob. Narrowing to the
/// origin here is therefore a widening relative to the manifest, which is fine only because the
/// `http` module itself still enforces the full pattern. This governs `fetch()` calls the page
/// makes directly, which the host never sees.
fn csp_source(pattern: &str) -> Option<String> {
    let (scheme, rest) = pattern.split_once("://")?;
    let host = rest.split('/').next()?;
    if host.is_empty() || host == "*" {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

/// Prepare a document for serving: inject the CSP and rewrite its import map (docs/document-format.md).
///
/// The CSP goes in as the first `<meta>` in `<head>`, so it applies to every subsequent element.
/// A manifest may override it, which is a deliberate act recorded in the document itself.
pub fn prepare_document(document: &Document, module_urls: &[(String, String)]) -> String {
    let manifest = &document.manifest;
    let csp = manifest
        .csp
        .clone()
        .unwrap_or_else(|| default_csp(manifest));

    let mut injected = String::new();
    injected.push_str(&format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">",
        html_escape_attribute(&csp)
    ));

    if !module_urls.is_empty() {
        let imports: serde_json::Map<String, serde_json::Value> = module_urls
            .iter()
            .map(|(name, path)| (name.clone(), serde_json::Value::String(path.clone())))
            .collect();
        let map = serde_json::json!({ "imports": imports });
        injected.push_str(&format!(
            "<script type=\"importmap\">{}</script>",
            // The map is JSON inside a script element, so a `</` in a specifier would close it.
            map.to_string().replace("</", "<\\/")
        ));
    }

    insert_into_head(&document.html, &injected)
}

/// Insert markup immediately after `<head>`, or synthesise a head if the document has none.
fn insert_into_head(html: &str, injected: &str) -> String {
    let lower = html.to_ascii_lowercase();

    if let Some(head_start) = lower.find("<head")
        && let Some(offset) = lower[head_start..].find('>')
    {
        let insertion = head_start + offset + 1;
        let mut out = String::with_capacity(html.len() + injected.len());
        out.push_str(&html[..insertion]);
        out.push_str(injected);
        out.push_str(&html[insertion..]);
        return out;
    }

    // No <head>. Put one after <html>, or at the very top if there is no <html> either.
    let wrapped = format!("<head>{injected}</head>");
    if let Some(html_start) = lower.find("<html")
        && let Some(offset) = lower[html_start..].find('>')
    {
        let insertion = html_start + offset + 1;
        let mut out = String::with_capacity(html.len() + wrapped.len());
        out.push_str(&html[..insertion]);
        out.push_str(&wrapped);
        out.push_str(&html[insertion..]);
        return out;
    }

    format!("{wrapped}{html}")
}

fn html_escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Whether a navigation is allowed to proceed (docs/architecture.md, N2 and docs/security.md, rule 8).
///
/// The runtime is not a browser: it has no tabs, no address bar, and no arbitrary browsing. A page
/// that navigates itself to an attacker's origin would be running that origin's code inside a
/// window that already holds this document's grants, so anything off-origin is refused unless the
/// manifest declared it. External links are opened in the user's real browser instead.
pub fn allows_navigation(url: &str, manifest: &Manifest) -> bool {
    if url.starts_with(ORIGIN) || url.starts_with("about:blank") {
        return true;
    }

    manifest
        .permissions
        .as_ref()
        .and_then(|p| p.net.as_ref())
        .is_some_and(|net| net.fetch.iter().any(|pattern| origin_matches(pattern, url)))
}

/// Match a `net.fetch` pattern against a URL, at origin granularity.
fn origin_matches(pattern: &str, url: &str) -> bool {
    let Some((pattern_scheme, pattern_rest)) = pattern.split_once("://") else {
        return false;
    };
    let Some((url_scheme, url_rest)) = url.split_once("://") else {
        return false;
    };
    if pattern_scheme != url_scheme {
        return false;
    }

    let pattern_host = pattern_rest.split('/').next().unwrap_or("");
    let url_host = url_rest.split('/').next().unwrap_or("");

    if let Some(suffix) = pattern_host.strip_prefix("*.") {
        // `*.example.com` covers subdomains but not the bare apex, matching CSP semantics.
        //
        // The leading dot in the comparison is the whole point: matching on the bare suffix would
        // let `notexample.com` satisfy `*.example.com`, because it does end with those characters.
        // Requiring a label boundary is what makes this a domain match rather than a string match.
        url_host.ends_with(&format!(".{suffix}"))
    } else {
        pattern_host == url_host
    }
}
