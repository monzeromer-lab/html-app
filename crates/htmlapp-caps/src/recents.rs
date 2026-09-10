//! The launcher's recent-documents list (docs/building.md).
//!
//! The open questions, open question 8 flags that the paths themselves may be sensitive. The resolution taken here
//! is store-with-purge: recording is on by default, `--private` suppresses it for a session, and
//! `clear()` is reachable from both the launcher and the CLI.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{CapsError, Result};

/// The launcher: "The last ten documents".
pub const MAX_RECENTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    pub path: PathBuf,
    /// The name from the manifest at the time it was last opened.
    pub name: Option<String>,
    pub hash: String,
    pub opened_at: u64,
    /// A compact summary of what it was granted, e.g. `["fs", "process"]`.
    #[serde(default)]
    pub granted: Vec<String>,
}

impl RecentEntry {
    /// "fs, process" — or "no permissions" when the document is powerless.
    pub fn permission_summary(&self) -> String {
        if self.granted.is_empty() {
            "no permissions".to_string()
        } else {
            self.granted.join(", ")
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recents {
    #[serde(default)]
    pub entries: Vec<RecentEntry>,
}

impl Recents {
    /// `$XDG_DATA_HOME/htmlapp/recents.json`.
    pub fn default_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("htmlapp")
            .join("recents.json")
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(CapsError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn load_default() -> Result<Self> {
        Self::load(Self::default_path())
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| CapsError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).map_err(|source| CapsError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save_default(&self) -> Result<()> {
        self.save(Self::default_path())
    }

    /// Move a document to the front of the list, de-duplicating by path and capping at
    /// [`MAX_RECENTS`].
    pub fn push(&mut self, path: &Path, name: Option<&str>, hash: &str, granted: Vec<String>) {
        self.entries.retain(|e| e.path != path);
        self.entries.insert(
            0,
            RecentEntry {
                path: path.to_path_buf(),
                name: name.map(str::to_string),
                hash: hash.to_string(),
                opened_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                granted,
            },
        );
        self.entries.truncate(MAX_RECENTS);
    }

    /// Remove one entry — the launcher's `Delete` key and *Remove from recents* menu item.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.path != path);
        before != self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Entries whose file still exists. The launcher shows only these, so a list full of deleted
    /// paths never accumulates.
    pub fn live(&self) -> impl Iterator<Item = &RecentEntry> {
        self.entries.iter().filter(|e| e.path.exists())
    }
}
