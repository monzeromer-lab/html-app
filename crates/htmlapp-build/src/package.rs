//! AppImage and Flatpak outputs (docs/building.md).

use std::path::{Path, PathBuf};

use htmlapp_caps::Manifest;

/// Build an AppDir next to a stapled binary.
///
/// The AppDir is produced unconditionally; turning it into a single-file `.AppImage` needs
/// `appimagetool`, which is not something the runtime can supply. packaging and distribution promises the AppImage, so
/// when the tool is missing the AppDir plus the exact command to finish the job is reported rather
/// than silently skipping the output.
pub fn write_appdir(manifest: &Manifest, binary: &Path, appdir: &Path) -> std::io::Result<PathBuf> {
    let name = manifest.display_name();
    let id = manifest.id.as_deref().unwrap_or("htmlapp.app");

    std::fs::create_dir_all(appdir.join("usr").join("bin"))?;
    std::fs::create_dir_all(appdir.join("usr").join("share").join("applications"))?;

    let target = appdir.join("usr").join("bin").join("app");
    std::fs::copy(binary, &target)?;

    let desktop = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Exec=app %f\n\
         Icon={id}\n\
         Terminal=false\n\
         Categories=Utility;\n"
    );
    std::fs::write(appdir.join(format!("{id}.desktop")), &desktop)?;
    std::fs::write(
        appdir
            .join("usr")
            .join("share")
            .join("applications")
            .join(format!("{id}.desktop")),
        &desktop,
    )?;

    // AppRun is what the AppImage runtime executes; it has to resolve the bundled libraries.
    let apprun = "#!/bin/sh\n\
                  HERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\n\
                  export LD_LIBRARY_PATH=\"$HERE/usr/lib:$LD_LIBRARY_PATH\"\n\
                  exec \"$HERE/usr/bin/app\" \"$@\"\n";
    let apprun_path = appdir.join("AppRun");
    std::fs::write(&apprun_path, apprun)?;

    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&apprun_path, std::fs::Permissions::from_mode(0o755))?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;

    std::fs::write(appdir.join(format!("{id}.svg")), crate::desktop::ICON_SVG)?;

    Ok(appdir.to_path_buf())
}

/// Whether `appimagetool` is on PATH.
pub fn has_appimagetool() -> bool {
    std::process::Command::new("appimagetool")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `appimagetool` over an AppDir.
pub fn build_appimage(appdir: &Path, output: &Path) -> std::io::Result<bool> {
    let status = std::process::Command::new("appimagetool")
        .arg(appdir)
        .arg(output)
        .status()?;
    Ok(status.success())
}

/// A Flatpak manifest for a built app (docs/building.md).
///
/// The bundled runtime links WebKitGTK, GTK, and Vulkan, so the finish-args have to grant the
/// sockets those need. Filesystem access is deliberately *not* granted here: the document's own
/// manifest governs that, and a blanket `--filesystem=home` would silently overrule it.
pub fn flatpak_manifest(manifest: &Manifest, binary_name: &str) -> String {
    let id = manifest.id.as_deref().unwrap_or("com.example.htmlapp");
    let name = manifest.display_name();

    serde_json::to_string_pretty(&serde_json::json!({
        "app-id": id,
        "runtime": "org.freedesktop.Platform",
        "runtime-version": "24.08",
        "sdk": "org.freedesktop.Sdk",
        "command": binary_name,
        "finish-args": [
            "--share=ipc",
            "--socket=fallback-x11",
            "--socket=wayland",
            "--device=dri",
            "--talk-name=org.freedesktop.Notifications",
        ],
        "modules": [{
            "name": name,
            "buildsystem": "simple",
            "build-commands": [
                format!("install -Dm755 {binary_name} /app/bin/{binary_name}"),
                format!("install -Dm644 {id}.desktop /app/share/applications/{id}.desktop"),
            ],
            "sources": [
                { "type": "file", "path": binary_name },
                { "type": "file", "path": format!("{id}.desktop") },
            ]
        }]
    }))
    .unwrap_or_default()
}
