//! Stapling a document onto a copy of the runtime (PRD §13).
//!
//! "Stapling appends the document to a copy of the runtime with a trailer containing offset and
//! length, which the runtime detects at startup. The end user gets one file and never learns the
//! word HTML App."
//!
//! Appending rather than embedding is what keeps this a *copy* operation instead of a build: the
//! runtime binary is already compiled, so producing a standalone app is a file write, not a
//! toolchain invocation. That is what makes §4.2 G6 compatible with §4.2 G1.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Identifies a stapled binary. Read from the last bytes of the file at startup.
pub const MAGIC: &[u8; 12] = b"HTMLAPP\0STAP";

/// `MAGIC` + document length (u64 LE) + trailer format version (u32 LE).
pub const TRAILER_LEN: usize = 12 + 8 + 4;

const TRAILER_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StapleError {
    #[error("could not read {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("this binary has a stapled trailer from a newer format (version {0})")]
    UnsupportedVersion(u32),

    #[error("the stapled trailer claims {claimed} bytes, but the file only has {available}")]
    Truncated { claimed: u64, available: u64 },
}

type Result<T> = std::result::Result<T, StapleError>;

/// Append `document` to a copy of `runtime`, writing the result to `output`.
pub fn staple(runtime: &Path, document: &[u8], output: &Path) -> Result<()> {
    let mut bytes = std::fs::read(runtime).map_err(|source| StapleError::Io {
        path: runtime.to_path_buf(),
        source,
    })?;

    bytes.extend_from_slice(document);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(document.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&TRAILER_VERSION.to_le_bytes());

    std::fs::write(output, &bytes).map_err(|source| StapleError::Io {
        path: output.to_path_buf(),
        source,
    })?;

    // Without this the "standalone binary" cannot be run, which rather defeats the purpose.
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = std::fs::metadata(output)
        .map_err(|source| StapleError::Io {
            path: output.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(output, permissions).map_err(|source| StapleError::Io {
        path: output.to_path_buf(),
        source,
    })?;

    Ok(())
}

/// Read the document stapled to `binary`, if there is one.
///
/// Returns `Ok(None)` for an ordinary runtime binary, which is the common case on every startup —
/// so this has to be cheap and must never fail merely because there is nothing there.
pub fn extract(binary: &Path) -> Result<Option<Vec<u8>>> {
    let mut file = std::fs::File::open(binary).map_err(|source| StapleError::Io {
        path: binary.to_path_buf(),
        source,
    })?;

    let length = file
        .metadata()
        .map_err(|source| StapleError::Io {
            path: binary.to_path_buf(),
            source,
        })?
        .len();

    if length < TRAILER_LEN as u64 {
        return Ok(None);
    }

    file.seek(SeekFrom::End(-(TRAILER_LEN as i64)))
        .map_err(|source| StapleError::Io {
            path: binary.to_path_buf(),
            source,
        })?;

    let mut trailer = [0u8; TRAILER_LEN];
    file.read_exact(&mut trailer).map_err(|source| StapleError::Io {
        path: binary.to_path_buf(),
        source,
    })?;

    if &trailer[..12] != MAGIC {
        return Ok(None);
    }

    let document_len = u64::from_le_bytes(trailer[12..20].try_into().expect("8 bytes"));
    let version = u32::from_le_bytes(trailer[20..24].try_into().expect("4 bytes"));
    if version > TRAILER_VERSION {
        return Err(StapleError::UnsupportedVersion(version));
    }

    let available = length - TRAILER_LEN as u64;
    if document_len > available {
        return Err(StapleError::Truncated {
            claimed: document_len,
            available,
        });
    }

    file.seek(SeekFrom::End(
        -((TRAILER_LEN as i64) + document_len as i64),
    ))
    .map_err(|source| StapleError::Io {
        path: binary.to_path_buf(),
        source,
    })?;

    let mut document = vec![0u8; document_len as usize];
    file.read_exact(&mut document).map_err(|source| StapleError::Io {
        path: binary.to_path_buf(),
        source,
    })?;

    Ok(Some(document))
}

/// Read the document stapled to the currently running executable, if any.
///
/// §7.1: "A stapled binary (`./tool`) runs its embedded document. The launcher never appears."
pub fn extract_from_current_exe() -> Option<Vec<u8>> {
    let exe = std::env::current_exe().ok()?;
    match extract(&exe) {
        Ok(document) => document,
        Err(error) => {
            tracing::warn!(%error, "could not read the stapled document");
            None
        }
    }
}
