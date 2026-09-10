use std::path::PathBuf;

/// Errors produced while locating, parsing, or enforcing a document's manifest.
#[derive(Debug, thiserror::Error)]
pub enum CapsError {
    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("manifest is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("manifest is missing required field `{0}`")]
    MissingField(&'static str),

    #[error("`{value}` is not a valid app id: {reason}")]
    InvalidId { value: String, reason: &'static str },

    #[error("invalid glob `{glob}`: {source}")]
    InvalidGlob {
        glob: String,
        #[source]
        source: globset::Error,
    },

    /// The threat model: a wildcard origin would defeat the exfiltration mitigation entirely.
    #[error(
        "`{0}` is not an acceptable net origin: a bare wildcard grants unrestricted network access"
    )]
    WildcardOrigin(String),

    #[error(
        "`{0}` is not a valid window mode (expected one of: window, layer, lock, tray, headless)"
    )]
    UnknownWindowMode(String),

    #[error("import `{name}` must declare an integrity hash")]
    MissingIntegrity { name: String },

    #[error("consent store is corrupt at {path}: {reason}")]
    CorruptConsentStore { path: PathBuf, reason: String },
}

pub type Result<T> = std::result::Result<T, CapsError>;
