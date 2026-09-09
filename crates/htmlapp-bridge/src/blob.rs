//! Bulk transfer that bypasses JSON (PRD §9.1).
//!
//! "For genuine bulk transfer the host mints a `blob://` URL the page fetches directly, so bytes
//! never pass through JSON."
//!
//! Lives in this crate because both halves need it: `htmlapp-api`'s `fs` module mints the tokens,
//! and `htmlapp-engine`'s origin resolver serves them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

/// Tokens that have been minted for this document, and the paths they stand for.
///
/// A blob URL is a capability in its own right: holding one grants read access to that exact file
/// for the life of the document, whether or not the manifest's globs still cover it. So tokens are
/// unguessable, and only ever created for a path that already passed a scope check.
#[derive(Clone, Default)]
pub struct BlobStore {
    entries: Arc<RwLock<HashMap<String, PathBuf>>>,
}

impl BlobStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a path under a token. The caller must have checked the path first.
    pub fn insert(&self, token: impl Into<String>, path: impl Into<PathBuf>) {
        self.entries.write().insert(token.into(), path.into());
    }

    /// Resolve a token back to its path.
    pub fn get(&self, token: &str) -> Option<PathBuf> {
        self.entries.read().get(token).cloned()
    }

    /// Drop a token, revoking the URL that carried it.
    pub fn remove(&self, token: &str) -> Option<PathBuf> {
        self.entries.write().remove(token)
    }

    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    /// The URL a page fetches to read the blob.
    pub fn url_for(token: &str) -> String {
        format!("htmlapp://app/__blob__/{token}")
    }

    /// Pull the token out of a request path like `/__blob__/abc123`.
    pub fn token_from_path(path: &str) -> Option<&str> {
        path.trim_start_matches('/')
            .strip_prefix("__blob__/")
            .filter(|token| {
                !token.is_empty()
                    && !token.contains('/')
                    && token.chars().all(|c| c.is_ascii_alphanumeric())
            })
    }

    /// Every path currently reachable by token, for diagnostics.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.entries.read().values().cloned().collect()
    }

    /// Whether a path is reachable by some token.
    pub fn contains_path(&self, path: &Path) -> bool {
        self.entries.read().values().any(|p| p == path)
    }
}

impl std::fmt::Debug for BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobStore")
            .field("tokens", &self.len())
            .finish()
    }
}
