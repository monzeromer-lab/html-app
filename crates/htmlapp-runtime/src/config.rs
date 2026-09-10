//! User configuration (the open questions, open question 9).
//!
//! "Some users will only ever use the CLI and will not want a window on a bare invocation. A
//! `launcher = false` config key that makes `htmlapp` with no args print help instead. Low cost,
//! probably worth it."
//!
//! Kept deliberately small. The goals and non-goals, N1 rules out a config *directory*, and every setting here is one
//! The runtime cannot infer — nothing about a document's behaviour lives in this file, because that
//! belongs in the document (docs/document-format.md).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The open questions Q9: whether a bare `htmlapp` opens the launcher or prints help.
    pub launcher: bool,
    /// The open questions Q8: whether documents are recorded in the recents list at all.
    pub recents: bool,
    /// Re-exec every document under bubblewrap, as though `--sandbox` had been passed.
    pub sandbox: bool,
    /// Make the engine inspector reachable without passing `--devtools` each time.
    pub devtools: bool,
    /// Force a colour scheme instead of following the desktop.
    pub color_scheme: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            launcher: true,
            recents: true,
            sandbox: false,
            devtools: false,
            color_scheme: None,
        }
    }
}

impl Config {
    /// `$XDG_CONFIG_HOME/htmlapp/config.toml`.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("htmlapp")
            .join("config.toml")
    }

    /// Load the config, treating a missing file as the defaults.
    ///
    /// A *malformed* file is not treated as missing: silently falling back to defaults would mean a
    /// typo in `launcher = flase` quietly re-enables the window the user turned off.
    pub fn load() -> Self {
        let path = Self::default_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "htmlapp: {} is not valid: {error}\n\
                         Using defaults. Fix the file or delete it to silence this.",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("htmlapp: could not read {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Write the current settings, creating the directory if needed.
    pub fn save(&self) -> std::io::Result<PathBuf> {
        let path = Self::default_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(&path, text)?;
        Ok(path)
    }
}
