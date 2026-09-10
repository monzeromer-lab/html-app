//! Hash-pinned consent (the security model rules 3 and 6).
//!
//! A decision is stored against `sha256(file)`, not against the path. Editing the file invalidates
//! The grant and re-prompts, and the prompt shows a diff of what changed in the permission set —
//! which is the mitigation for the "trojan update to a trusted file" threat in the threat model.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{CapsError, Result};
use crate::permissions::Permissions;

/// Seconds since the Unix epoch. Stored rather than a formatted date so the file stays
/// locale-independent and diffable.
pub type Timestamp = u64;

fn now() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the runtime should do with a document whose manifest requests capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentDecision {
    /// Nothing was requested. The security model, rule 1 — run it, silently, with no native APIs.
    NotRequired,
    /// This exact file content was approved before. Run it.
    AlreadyGranted(Box<ConsentRecord>),
    /// This exact file content was refused before. Run it as a powerless page.
    AlreadyDenied(Box<ConsentRecord>),
    /// Never seen this content. Show the consent sheet.
    NeedsPrompt {
        /// `Some` when a *different version* of this path was decided on before, in which case the
        /// sheet shows what changed rather than presenting the request as brand new.
        previous: Option<Box<ConsentRecord>>,
        diff: PermissionDiff,
    },
}

/// One stored decision, keyed by content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRecord {
    /// `sha256` of the document bytes. The primary key.
    pub hash: String,
    /// Where the file was when the decision was made. Informational only — it is never trusted to
    /// identify the document, because the hash does that.
    pub path: Option<PathBuf>,
    pub name: Option<String>,
    pub id: Option<String>,
    pub granted: bool,
    /// Exactly what was approved. Re-checked against the manifest on every run, so a stored record
    /// can never grant more than the current file asks for.
    pub permissions: Permissions,
    pub decided_at: Timestamp,
    #[serde(default)]
    pub last_used: Option<Timestamp>,
}

/// What changed between two versions of a document's permission set (docs/security.md, rule 3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDiff {
    /// Modules present now that were not granted before. These are what the user must weigh.
    pub added: Vec<String>,
    /// Modules that were granted before and are no longer requested.
    pub removed: Vec<String>,
    /// Modules present in both, but with a widened or narrowed scope.
    pub changed: Vec<String>,
}

impl PermissionDiff {
    /// Diff `new` against `old`, at module granularity plus scope changes within a module.
    pub fn between(old: Option<&Permissions>, new: &Permissions) -> Self {
        let old_modules: BTreeSet<&str> = old
            .map(|p| p.granted_modules().into_iter().collect())
            .unwrap_or_default();
        let new_modules: BTreeSet<&str> = new.granted_modules().into_iter().collect();

        let added = new_modules
            .difference(&old_modules)
            .map(|s| s.to_string())
            .collect();
        let removed = old_modules
            .difference(&new_modules)
            .map(|s| s.to_string())
            .collect();

        let mut changed = Vec::new();
        if let Some(old) = old {
            for module in new_modules.intersection(&old_modules) {
                if scope_changed(module, old, new) {
                    changed.push((*module).to_string());
                }
            }
        }

        Self {
            added,
            removed,
            changed,
        }
    }

    /// Whether anything about this diff warrants the user's attention.
    pub fn is_escalation(&self) -> bool {
        !self.added.is_empty() || !self.changed.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Whether a module granted in both versions asks for a different scope now.
fn scope_changed(module: &str, old: &Permissions, new: &Permissions) -> bool {
    match module {
        "fs" => old.fs != new.fs,
        "process" => old.process != new.process,
        "http" => old.net != new.net,
        "sql" => old.sql != new.sql,
        "dbus" => old.dbus != new.dbus,
        "clipboard" => old.clipboard != new.clipboard,
        "shortcut" => old.shortcut != new.shortcut,
        "portal" => old.portal != new.portal,
        "net" => old.sockets != new.sockets,
        "serial" => old.serial != new.serial,
        "ffi" => old.ffi != new.ffi,
        "plugin" => old.plugin != new.plugin,
        _ => false,
    }
}

/// The on-disk consent database.
///
/// Kept as one JSON file rather than a database: it must stay hand-inspectable and hand-editable,
/// because "what has this granted?" is a question users need to be able to answer without tooling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsentStore {
    #[serde(default = "store_version")]
    pub version: u32,
    #[serde(default)]
    pub records: Vec<ConsentRecord>,
}

fn store_version() -> u32 {
    1
}

impl ConsentStore {
    /// `$XDG_DATA_HOME/htmlapp/consent.json`.
    pub fn default_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("htmlapp")
            .join("consent.json")
    }

    /// Load the store, treating a missing file as an empty store.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| CapsError::CorruptConsentStore {
                path: path.to_path_buf(),
                reason: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                version: store_version(),
                records: Vec::new(),
            }),
            Err(source) => Err(CapsError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn load_default() -> Result<Self> {
        Self::load(Self::default_path())
    }

    /// Write the store back, creating the directory and replacing atomically so an interrupted
    /// write cannot leave a truncated file that would silently drop every grant.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| CapsError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, json.as_bytes()).map_err(|source| CapsError::Io {
            path: temp.clone(),
            source,
        })?;
        std::fs::rename(&temp, path).map_err(|source| CapsError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save_default(&self) -> Result<()> {
        self.save(Self::default_path())
    }

    pub fn find(&self, hash: &str) -> Option<&ConsentRecord> {
        self.records.iter().find(|r| r.hash == hash)
    }

    /// The most recent decision about a *path*, regardless of content hash. Used only to build the
    /// "what changed" diff when a known file has been edited.
    pub fn find_by_path(&self, path: &Path) -> Option<&ConsentRecord> {
        self.records
            .iter()
            .filter(|r| r.path.as_deref() == Some(path))
            .max_by_key(|r| r.decided_at)
    }

    /// Decide what to do with a document.
    ///
    /// `requested` is read from the file being run right now, never from the store, so a stored
    /// record can only ever confirm a grant — it can never widen one.
    pub fn decide(
        &self,
        hash: &str,
        path: Option<&Path>,
        requested: Option<&Permissions>,
    ) -> ConsentDecision {
        let requested = match requested {
            Some(p) if !p.is_empty() => p,
            _ => return ConsentDecision::NotRequired,
        };

        if let Some(record) = self.find(hash) {
            // Defence in depth: if the stored permissions no longer match what the file asks for,
            // The hash match was not meaningful and the user is asked again.
            if &record.permissions == requested {
                return if record.granted {
                    ConsentDecision::AlreadyGranted(Box::new(record.clone()))
                } else {
                    ConsentDecision::AlreadyDenied(Box::new(record.clone()))
                };
            }
        }

        let previous = path.and_then(|p| self.find_by_path(p));
        let diff = PermissionDiff::between(previous.map(|r| &r.permissions), requested);
        ConsentDecision::NeedsPrompt {
            previous: previous.cloned().map(Box::new),
            diff,
        }
    }

    /// Record a decision, replacing any prior record for the same content hash.
    pub fn record(
        &mut self,
        hash: &str,
        path: Option<&Path>,
        name: Option<&str>,
        id: Option<&str>,
        permissions: &Permissions,
        granted: bool,
    ) -> &ConsentRecord {
        self.records.retain(|r| r.hash != hash);
        self.records.push(ConsentRecord {
            hash: hash.to_string(),
            path: path.map(Path::to_path_buf),
            name: name.map(str::to_string),
            id: id.map(str::to_string),
            granted,
            permissions: permissions.clone(),
            decided_at: now(),
            last_used: Some(now()),
        });
        self.records.last().expect("just pushed")
    }

    /// Note that a granted document was run, for the launcher's recents ordering.
    pub fn touch(&mut self, hash: &str) {
        if let Some(record) = self.records.iter_mut().find(|r| r.hash == hash) {
            record.last_used = Some(now());
        }
    }

    /// Revoke by content hash. Returns how many records were removed.
    pub fn revoke_hash(&mut self, hash: &str) -> usize {
        let before = self.records.len();
        self.records.retain(|r| r.hash != hash);
        before - self.records.len()
    }

    /// Revoke every decision recorded for a path, across all versions of its content.
    pub fn revoke_path(&mut self, path: &Path) -> usize {
        let before = self.records.len();
        self.records.retain(|r| r.path.as_deref() != Some(path));
        before - self.records.len()
    }

    /// Drop every stored decision.
    pub fn revoke_all(&mut self) -> usize {
        let count = self.records.len();
        self.records.clear();
        count
    }
}
