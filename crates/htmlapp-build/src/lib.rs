//! `htmlapp build` (docs/building.md).
//!
//! ```sh
//! htmlapp build tool.hta
//! # → tool                    (standalone binary, HTML stapled into a copy of the runtime)
//! # → tool.desktop            (with icon extracted from the manifest)
//! # → tool-x86_64.AppImage
//! # → com.example.tool.json   (Flatpak manifest)
//! ```

#![forbid(unsafe_code)]

pub mod desktop;
pub mod package;
pub mod staple;

use std::path::{Path, PathBuf};

use htmlapp_caps::Document;

pub use desktop::{InstallError, InstallPaths, InstallReport, MIME_TYPE, install, uninstall};
pub use staple::{StapleError, extract, extract_from_current_exe, staple};

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Caps(#[from] htmlapp_caps::CapsError),

    #[error(transparent)]
    Staple(#[from] StapleError),

    #[error("could not write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not locate the running runtime binary: {0}")]
    NoRuntime(String),
}

/// What `htmlapp build` produced.
#[derive(Debug, Default)]
pub struct BuildOutput {
    pub binary: Option<PathBuf>,
    pub desktop_entry: Option<PathBuf>,
    pub icon: Option<PathBuf>,
    pub appdir: Option<PathBuf>,
    pub appimage: Option<PathBuf>,
    pub flatpak_manifest: Option<PathBuf>,
    /// Outputs that could not be produced, and why.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub output_dir: PathBuf,
    pub appimage: bool,
    pub flatpak: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("."),
            appimage: true,
            flatpak: true,
        }
    }
}

/// Build a standalone app from one `.hta`.
pub fn build(source: &Path, options: &BuildOptions) -> Result<BuildOutput, BuildError> {
    // Parsing first means a document with a broken manifest fails here rather than shipping and
    // failing on the user's machine.
    let document = Document::load(source)?;
    let bytes = std::fs::read(source).map_err(|source_error| BuildError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;

    let runtime = std::env::current_exe().map_err(|e| BuildError::NoRuntime(e.to_string()))?;

    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".into());

    std::fs::create_dir_all(&options.output_dir).map_err(|e| BuildError::Io {
        path: options.output_dir.clone(),
        source: e,
    })?;

    let mut output = BuildOutput::default();

    // 1. The standalone binary.
    let binary = options.output_dir.join(&stem);
    staple::staple(&runtime, &bytes, &binary)?;
    output.binary = Some(binary.clone());

    // 2. The icon, extracted from the manifest.
    let icon_base = options.output_dir.join(&stem);
    let icon = desktop::write_manifest_icon(&document.manifest, &icon_base);
    let icon_name = icon
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "htmlapp".into());
    if icon.is_none() {
        output
            .skipped
            .push("no icon: the manifest has no `icon` data: URI".into());
    }
    output.icon = icon;

    // 3. The .desktop entry.
    let desktop_path = options.output_dir.join(format!("{stem}.desktop"));
    let absolute = binary.canonicalize().unwrap_or(binary.clone());
    std::fs::write(
        &desktop_path,
        desktop::app_desktop_entry(&document.manifest, &absolute, &icon_name),
    )
    .map_err(|e| BuildError::Io {
        path: desktop_path.clone(),
        source: e,
    })?;
    output.desktop_entry = Some(desktop_path);

    // 4. The AppImage.
    if options.appimage {
        let appdir = options.output_dir.join(format!("{stem}.AppDir"));
        package::write_appdir(&document.manifest, &binary, &appdir).map_err(|e| {
            BuildError::Io {
                path: appdir.clone(),
                source: e,
            }
        })?;
        output.appdir = Some(appdir.clone());

        if package::has_appimagetool() {
            let appimage = options
                .output_dir
                .join(format!("{stem}-{}.AppImage", std::env::consts::ARCH));
            match package::build_appimage(&appdir, &appimage) {
                Ok(true) => output.appimage = Some(appimage),
                Ok(false) => output.skipped.push("appimagetool failed".into()),
                Err(e) => output.skipped.push(format!("appimagetool: {e}")),
            }
        } else {
            output.skipped.push(format!(
                "no .AppImage: appimagetool is not installed. \
                 The AppDir is ready — run: appimagetool {}",
                appdir.display()
            ));
        }
    }

    // 5. The Flatpak manifest.
    if options.flatpak {
        let id = document
            .manifest
            .id
            .clone()
            .unwrap_or_else(|| format!("com.example.{stem}"));
        let path = options.output_dir.join(format!("{id}.json"));
        std::fs::write(&path, package::flatpak_manifest(&document.manifest, &stem)).map_err(
            |e| BuildError::Io {
                path: path.clone(),
                source: e,
            },
        )?;
        output.flatpak_manifest = Some(path);
    }

    Ok(output)
}

/// A minimal starter document for the launcher's "New blank app" (docs/building.md).
///
/// The goals and non-goals, N1 rules out a scaffolding command, so this is deliberately one file with a commented
/// manifest — everything an author needs to see, and nothing to un-scaffold.
pub const BLANK_APP: &str = r##"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<!--
  This block is the manifest. The host reads it before the page loads.
  It is inert to browsers, so renaming this file to .html opens it in one —
  without any native APIs. That makes browser devtools a usable fallback.

  Everything under "permissions" is optional. With no permissions block at all,
  this file is just a page in a window, which is the safe default.
-->
<script type="application/htmlapp+json">
{
  "name": "My App",
  "id": "com.example.myapp",
  "version": "0.1.0",
  "window": { "title": "My App", "width": 900, "height": 640 }
}
</script>
<style>
  :root { color-scheme: light dark; }
  body {
    font: 15px/1.5 system-ui, sans-serif;
    margin: 0; padding: 2.5rem;
    display: flex; flex-direction: column; gap: 1rem;
  }
  code { background: color-mix(in srgb, currentColor 10%, transparent);
         padding: .15em .4em; border-radius: 4px; }
</style>
</head>
<body>
  <h1>Hello from HTML App</h1>
  <p id="status">Checking what this document was granted…</p>

  <p>
    To give this file filesystem access, add a <code>permissions</code> block to
    the manifest above and reopen it:
  </p>
  <pre><code>"permissions": { "fs": { "read": ["~/Documents/**"] } }</code></pre>

<script type="module">
  // A module only exists if the manifest was granted it, so feature detection
  // is the honest way to check.
  const granted = Object.keys(htmlapp.permissions ?? {});
  document.getElementById('status').textContent = granted.length
    ? `Granted: ${granted.join(', ')}`
    : 'No permissions requested — this is running as an ordinary page.';
</script>
</body>
</html>
"##;
