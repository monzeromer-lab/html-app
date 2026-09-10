//! Locating the manifest inside a `.hta` file (docs/document-format.md).
//!
//! The host reads the manifest *before* the document reaches the engine, so this is a small
//! purpose-built scanner rather than a full HTML parse: pulling in an HTML tree builder to find one
//! `<script>` block would mean the security decision depended on a parser the engine never sees.
//! The scanner is deliberately conservative — anything it cannot read confidently is treated as
//! "no manifest", which the security model makes the safe outcome.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{CapsError, Result};
use crate::manifest::{MANIFEST_MIME, Manifest};

/// A `.hta` file that has been read and had its manifest extracted.
#[derive(Debug, Clone)]
pub struct Document {
    /// Where the file came from. `None` for a stapled or stdin-fed document.
    pub source: Option<PathBuf>,
    /// The full HTML, exactly as it will be served over `htmlapp://app/`.
    pub html: String,
    /// The parsed manifest, or the default (powerless) manifest if the file had none.
    pub manifest: Manifest,
    /// Whether a manifest block was actually present, as opposed to defaulted.
    pub has_manifest: bool,
    /// `sha256(file bytes)` — the key that consent is pinned to (docs/security.md, rule 3).
    pub hash: String,
}

impl Document {
    /// Read and parse a `.hta` from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| CapsError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut document = Self::from_bytes(&bytes)?;
        document.source = Some(path.to_path_buf());
        Ok(document)
    }

    /// Parse a `.hta` already in memory — used for stapled binaries (docs/building.md) and stdin.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let hash = hex::encode(Sha256::digest(bytes));
        let html = String::from_utf8_lossy(bytes).into_owned();
        let (manifest, has_manifest) = match extract_manifest_json(&html) {
            Some(json) => (Manifest::from_json(&json)?, true),
            None => (Manifest::default(), false),
        };
        Ok(Self {
            source: None,
            html,
            manifest,
            has_manifest,
            hash,
        })
    }

    /// The directory sibling assets resolve against (docs/document-format.md), if the policy allows any.
    pub fn asset_root(&self) -> Option<PathBuf> {
        use crate::manifest::AssetPolicy;
        match self.manifest.assets {
            AssetPolicy::None => None,
            AssetPolicy::Sibling => self
                .source
                .as_ref()
                .and_then(|p| p.parent())
                .map(Path::to_path_buf),
        }
    }

    /// A short, stable identity for logs and the launcher's recents list.
    pub fn short_hash(&self) -> &str {
        &self.hash[..12]
    }
}

/// Find the contents of the first `<script type="application/htmlapp+json">` block.
///
/// Returns `None` when there is no such block, which is a valid, powerless document.
pub fn extract_manifest_json(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0usize;

    while let Some(offset) = lower[cursor..].find("<script") {
        let tag_start = cursor + offset;
        // Guard against matching `<scriptfoo`.
        let after_name = tag_start + "<script".len();
        let next = lower.as_bytes().get(after_name).copied();
        if !matches!(next, Some(b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/')) {
            cursor = after_name;
            continue;
        }

        let tag_end_rel = lower[after_name..].find('>')?;
        let tag_end = after_name + tag_end_rel;
        let attributes = &html[after_name..tag_end];

        // A self-closing `<script/>` carries no manifest.
        if attributes.trim_end().ends_with('/') {
            cursor = tag_end + 1;
            continue;
        }

        let body_start = tag_end + 1;
        let close_rel = lower[body_start..].find("</script")?;
        let body_end = body_start + close_rel;

        if attribute_equals(attributes, "type", MANIFEST_MIME) {
            return Some(html[body_start..body_end].to_string());
        }

        cursor = body_end;
    }

    None
}

/// Whether a tag's attribute string sets `name` to `expected`, case-insensitively on the name and
/// on the value, tolerating single quotes, double quotes, and unquoted values.
fn attribute_equals(attributes: &str, name: &str, expected: &str) -> bool {
    let bytes = attributes.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && !(bytes[i] as char).is_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        if key_start == i {
            i += 1;
            continue;
        }
        let key = &attributes[key_start..i];

        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            // Valueless attribute; keep scanning.
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return false;
        }

        let value = match bytes[i] {
            quote @ (b'"' | b'\'') => {
                i += 1;
                let value_start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                let value = &attributes[value_start..i.min(attributes.len())];
                i += 1;
                value
            }
            _ => {
                let value_start = i;
                while i < bytes.len() && !(bytes[i] as char).is_whitespace() {
                    i += 1;
                }
                &attributes[value_start..i]
            }
        };

        if key.eq_ignore_ascii_case(name) {
            return value.trim().eq_ignore_ascii_case(expected);
        }
    }

    false
}

/// `sha256` of arbitrary bytes, hex-encoded. The consent key (docs/security.md, rule 3).
pub fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
