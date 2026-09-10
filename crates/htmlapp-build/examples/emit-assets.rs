//! Writes the packaged desktop assets to `packaging/`.
//!
//! Generated from the same code `htmlapp install` uses, so a packaged install and a user-local one
//! cannot disagree about the `.desktop` entry or the MIME type (docs/building.md).
fn main() -> std::io::Result<()> {
    let out = std::path::Path::new("packaging");
    std::fs::create_dir_all(out)?;

    // `Exec=htmlapp %f` — a packaged install puts the binary on PATH.
    std::fs::write(
        out.join("htmlapp.desktop"),
        htmlapp_build::desktop::runtime_desktop_entry("htmlapp"),
    )?;
    std::fs::write(
        out.join("htmlapp.xml"),
        htmlapp_build::desktop::mime_package(),
    )?;
    std::fs::write(out.join("htmlapp.svg"), htmlapp_build::desktop::ICON_SVG)?;

    println!("wrote packaging/htmlapp.{{desktop,xml,svg}}");
    Ok(())
}
