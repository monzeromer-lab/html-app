//! Desktop integration (docs/building.md).
//!
//! Packaging and distribution is unusually firm about this: "An install that leaves double-click broken is a failed
//! install." So the `.desktop` entry, the MIME registration, and the icons are generated together
//! and installed together, and `update-desktop-database` and `update-mime-database` are always run.

use std::path::{Path, PathBuf};

use htmlapp_caps::Manifest;

/// The MIME type HTML App owns (docs/document-format.md).
pub const MIME_TYPE: &str = "application/hta";
/// The Linux registration alias.
pub const MIME_ALIAS: &str = "application/x-hta";

/// The runtime's own `.desktop` entry, exactly as desktop integration specifies.
///
/// `%f` is the whole mechanism: the application menu passes no argument and the launcher appears,
/// while a file manager passes a path and the document runs. One entry serves both paths.
pub fn runtime_desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=HTML App\n\
         GenericName=HTML Application Runtime\n\
         Comment=Run a single .hta file as a desktop application\n\
         Exec={exec} %f\n\
         Icon=htmlapp\n\
         Terminal=false\n\
         Categories=Development;Utility;\n\
         MimeType={MIME_TYPE};{MIME_ALIAS};\n\
         StartupWMClass=htmlapp\n\
         Actions=OpenFile;Permissions;\n\
         \n\
         [Desktop Action OpenFile]\n\
         Name=Open an .hta file…\n\
         Exec={exec} --open\n\
         \n\
         [Desktop Action Permissions]\n\
         Name=Manage permissions\n\
         Exec={exec} --permissions-ui\n"
    )
}

/// The shared-mime-info package that makes `.hta` a recognised type.
pub fn mime_package() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="{MIME_TYPE}">
    <comment>HTML Application</comment>
    <glob pattern="*.hta"/>
    <sub-class-of type="text/html"/>
    <icon name="text-html"/>
    <generic-icon name="text-html"/>
  </mime-type>
  <mime-type type="{MIME_ALIAS}">
    <comment>HTML Application</comment>
    <alias type="{MIME_TYPE}"/>
  </mime-type>
</mime-info>
"#
    )
}

/// A `.desktop` entry for an app built with `htmlapp build` (docs/building.md).
///
/// It names the built binary directly and carries no MIME association: a built app is its own
/// application, and the process and instance model says it "never routes through any of this".
pub fn app_desktop_entry(manifest: &Manifest, exec: &Path, icon: &str) -> String {
    let name = manifest.display_name();
    let comment = manifest
        .id
        .as_deref()
        .map(|id| format!("Comment={id}\n"))
        .unwrap_or_default();

    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         {comment}\
         Exec={exec} %f\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Utility;\n\
         StartupWMClass={wm_class}\n",
        exec = exec.display(),
        wm_class = manifest.id.as_deref().unwrap_or("htmlapp"),
    )
}

/// Where a user-local install puts things.
#[derive(Debug, Clone)]
pub struct InstallPaths {
    pub applications: PathBuf,
    pub mime_packages: PathBuf,
    pub icons: PathBuf,
}

impl InstallPaths {
    /// `~/.local/share/...`, which needs no root and is what the `curl | sh` installer uses.
    pub fn user() -> Option<Self> {
        let data = dirs::data_dir()?;
        Some(Self {
            applications: data.join("applications"),
            mime_packages: data.join("mime").join("packages"),
            icons: data
                .join("icons")
                .join("hicolor")
                .join("scalable")
                .join("apps"),
        })
    }

    /// The system paths, for a package install.
    pub fn system() -> Self {
        Self {
            applications: PathBuf::from("/usr/share/applications"),
            mime_packages: PathBuf::from("/usr/share/mime/packages"),
            icons: PathBuf::from("/usr/share/icons/hicolor/scalable/apps"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("could not write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not determine the user's data directory")]
    NoDataDir,
}

/// What an install actually did, so the CLI can report it honestly.
#[derive(Debug, Default)]
pub struct InstallReport {
    pub written: Vec<PathBuf>,
    /// Database refreshes that did not run, with the reason. packaging and distribution treats these as part of the
    /// install, so a failure here is reported rather than swallowed.
    pub warnings: Vec<String>,
}

/// Register the runtime with the desktop: `.desktop` entry, MIME type, and icon (docs/building.md).
pub fn install(paths: &InstallPaths, exec: &Path) -> Result<InstallReport, InstallError> {
    let mut report = InstallReport::default();

    let desktop = paths.applications.join("htmlapp.desktop");
    write_file(
        &desktop,
        runtime_desktop_entry(&exec.display().to_string()).as_bytes(),
    )?;
    report.written.push(desktop);

    let mime = paths.mime_packages.join("htmlapp.xml");
    write_file(&mime, mime_package().as_bytes())?;
    report.written.push(mime);

    let icon = paths.icons.join("htmlapp.svg");
    write_file(&icon, ICON_SVG.as_bytes())?;
    report.written.push(icon);

    // Packaging and distribution: "An install that leaves double-click broken is a failed install."
    for (program, argument) in [
        ("update-desktop-database", paths.applications.clone()),
        (
            "update-mime-database",
            paths
                .mime_packages
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        ),
    ] {
        match std::process::Command::new(program).arg(&argument).status() {
            Ok(status) if status.success() => {}
            Ok(status) => report.warnings.push(format!(
                "{program} exited with {status}; double-clicking a .hta may not work until it is \
                 run successfully"
            )),
            Err(error) => report.warnings.push(format!(
                "could not run {program} ({error}); double-clicking a .hta may not work until it \
                 is run manually"
            )),
        }
    }

    Ok(report)
}

/// Remove what [`install`] wrote.
pub fn uninstall(paths: &InstallPaths) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for path in [
        paths.applications.join("htmlapp.desktop"),
        paths.mime_packages.join("htmlapp.xml"),
        paths.icons.join("htmlapp.svg"),
    ] {
        if std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), InstallError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| InstallError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, contents).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The runtime's icon. Inline so a fresh checkout has no binary asset to keep in sync.
pub const ICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
  <rect width="64" height="64" rx="14" fill="#2563eb"/>
  <path d="M22 22 L13 32 L22 42" fill="none" stroke="#fff" stroke-width="4.5"
        stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M42 22 L51 32 L42 42" fill="none" stroke="#fff" stroke-width="4.5"
        stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M36 18 L28 46" fill="none" stroke="#fff" stroke-width="4.5" stroke-linecap="round"/>
</svg>
"##;

/// Extract a manifest icon into a file next to a built app.
///
/// Only `data:` URIs are honoured. A remote icon URL would mean fetching at build time from
/// somewhere the manifest names, which the goals and non-goals, N5 rules out.
pub fn write_manifest_icon(manifest: &Manifest, output: &Path) -> Option<PathBuf> {
    let icon = manifest.icon.as_deref()?;
    let (header, payload) = icon.strip_prefix("data:")?.split_once(',')?;

    let bytes = if header.ends_with(";base64") {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()?
    } else {
        payload.as_bytes().to_vec()
    };

    let extension = match header.split(';').next().unwrap_or("") {
        "image/svg+xml" => "svg",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => return None,
    };

    let path = output.with_extension(extension);
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}
