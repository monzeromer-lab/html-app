//! Import maps without a build step (docs/document-format.md).
//!
//! A document may name remote ES modules. They are fetched once, verified against the integrity
//! hash the manifest pins them to, cached under `~/.cache/htmlapp/modules/<sha256>`, and served
//! from that cache forever after — so a single file can use real libraries and still start offline.
//!
//! The open questions, open question 5 asks whether a cached module should ever be revalidated. It should not:
//! revalidation would make the same file behave differently on different days, and the integrity
//! hash already pins exactly one acceptable body. `htmlapp cache purge` is the explicit escape.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use htmlapp_caps::ImportSpec;
use sha2::{Digest, Sha256, Sha384, Sha512};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not fetch {url}: {reason}")]
    Fetch { url: String, reason: String },

    #[error(
        "integrity check failed for {name} ({url}): manifest pins {expected}, \
         but the server returned {actual}"
    )]
    IntegrityMismatch {
        name: String,
        url: String,
        expected: String,
        actual: String,
    },

    #[error("`{0}` is not a supported integrity hash (expected sha256-, sha384-, or sha512-)")]
    UnsupportedIntegrity(String),

    #[error("cache i/o error at {path}: {source}")]
    Cache {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// How the cache reaches the network. Separated from the cache logic so the pinning and
/// verification rules can be tested without one.
pub trait ModuleFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, ImportError>;
}

/// The on-disk module cache.
#[derive(Debug, Clone)]
pub struct ModuleCache {
    root: PathBuf,
}

impl ModuleCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `~/.cache/htmlapp/modules`.
    pub fn default_root() -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("htmlapp")
            .join("modules")
    }

    pub fn default_cache() -> Self {
        Self::new(Self::default_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a module with this integrity hash lives. Keyed on the *pinned hash*, not the URL, so
    /// two documents pinning the same bytes share one cache entry and a URL that starts serving
    /// different bytes cannot silently take over an existing one.
    pub fn path_for(&self, integrity: &str) -> PathBuf {
        self.root.join(format!("{}.js", cache_key(integrity)))
    }

    pub fn is_cached(&self, integrity: &str) -> bool {
        self.path_for(integrity).is_file()
    }

    /// Resolve every import in a manifest, fetching what is not already cached.
    ///
    /// Returns the import-map entries to inject: bare specifier → `htmlapp://app/__modules__/...`.
    /// The page therefore never reaches the network for a module itself, which is what lets
    /// `connect-src` stay pinned to the manifest's own allow-list.
    pub fn resolve_all(
        &self,
        imports: &BTreeMap<String, ImportSpec>,
        fetcher: &dyn ModuleFetcher,
    ) -> Result<Vec<(String, String)>, ImportError> {
        let mut resolved = Vec::new();
        for (name, spec) in imports {
            self.ensure(name, spec, fetcher)?;
            resolved.push((
                name.clone(),
                format!("/__modules__/{}.js", cache_key(&spec.integrity)),
            ));
        }
        Ok(resolved)
    }

    /// Ensure one module is present and verified.
    fn ensure(
        &self,
        name: &str,
        spec: &ImportSpec,
        fetcher: &dyn ModuleFetcher,
    ) -> Result<PathBuf, ImportError> {
        let path = self.path_for(&spec.integrity);
        if path.is_file() {
            tracing::debug!(module = %name, "serving pinned module from cache");
            return Ok(path);
        }

        tracing::info!(module = %name, url = %spec.url, "fetching pinned module");
        let body = fetcher.fetch(&spec.url)?;

        let actual = compute_integrity(&spec.integrity, &body)?;
        if !constant_time_eq(actual.as_bytes(), spec.integrity.trim().as_bytes()) {
            return Err(ImportError::IntegrityMismatch {
                name: name.to_string(),
                url: spec.url.clone(),
                expected: spec.integrity.clone(),
                actual,
            });
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ImportError::Cache {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        // Write-then-rename: a module is either absent or complete and verified, never truncated.
        let temp = path.with_extension("part");
        std::fs::write(&temp, &body).map_err(|source| ImportError::Cache {
            path: temp.clone(),
            source,
        })?;
        std::fs::rename(&temp, &path).map_err(|source| ImportError::Cache {
            path: path.clone(),
            source,
        })?;

        Ok(path)
    }

    /// `htmlapp cache purge`.
    pub fn purge(&self) -> Result<usize, ImportError> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Ok(0);
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Total bytes held in the cache, for `htmlapp cache info`.
    pub fn size(&self) -> u64 {
        std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    }
}

/// A filesystem-safe key for an integrity string.
fn cache_key(integrity: &str) -> String {
    hex::encode(Sha256::digest(integrity.trim().as_bytes()))
}

/// Compute the integrity string for `body` using the same algorithm `expected` names.
fn compute_integrity(expected: &str, body: &[u8]) -> Result<String, ImportError> {
    let expected = expected.trim();
    let (algorithm, _) = expected
        .split_once('-')
        .ok_or_else(|| ImportError::UnsupportedIntegrity(expected.to_string()))?;

    let digest = match algorithm {
        "sha256" => Sha256::digest(body).to_vec(),
        "sha384" => Sha384::digest(body).to_vec(),
        "sha512" => Sha512::digest(body).to_vec(),
        _ => return Err(ImportError::UnsupportedIntegrity(expected.to_string())),
    };

    Ok(format!(
        "{algorithm}-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    ))
}

/// Compare without leaking where the first difference is via timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
