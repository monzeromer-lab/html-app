//! Wiring the launcher to the things only the process can do (docs/building.md and docs/architecture.md).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use htmlapp_shell::launcher::LauncherDelegate;

/// Implements the launcher's callbacks.
///
/// The process and instance model is what shapes this: "Each document runs in its own process", and "the launcher is
/// single-instance. Opening a file from it spawns a detached child; the launcher stays up."
pub struct CliDelegate {
    /// A small runtime for the portal calls, which are async.
    runtime: tokio::runtime::Handle,
    _keepalive: Arc<tokio::runtime::Runtime>,
}

impl CliDelegate {
    pub fn new() -> Result<Arc<Self>> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("htmlapp-launcher")
                .build()?,
        );
        Ok(Arc::new(Self {
            runtime: runtime.handle().clone(),
            _keepalive: runtime,
        }))
    }

    /// Spawn a document in its own process, fully detached from this one.
    fn spawn_document(path: &Path) {
        let Ok(exe) = std::env::current_exe() else {
            tracing::error!("could not locate the running binary");
            return;
        };

        match std::process::Command::new(exe)
            .arg(path)
            // Detached: the process and instance model says closing the launcher must not kill running documents.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
        {
            Ok(child) => tracing::info!(pid = child.id(), path = %path.display(), "opened document"),
            Err(error) => tracing::error!(%error, path = %path.display(), "could not open document"),
        }
    }
}

impl LauncherDelegate for CliDelegate {
    fn open_document(&self, path: &Path) {
        Self::spawn_document(path);
    }

    fn choose_document(&self) {
        let handle = self.runtime.clone();
        handle.spawn(async move {
            match htmlapp_api::dialog::choose_hta().await {
                Ok(Some(path)) => CliDelegate::spawn_document(&path),
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "the file chooser failed"),
            }
        });
    }

    fn create_blank_app(&self) {
        let handle = self.runtime.clone();
        handle.spawn(async move {
            let chosen = match htmlapp_api::dialog::choose_save_location("my-app.hta").await {
                Ok(Some(path)) => path,
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!(%error, "the save dialog failed");
                    return;
                }
            };

            // The launcher: "removes the blank-page problem without introducing a scaffolding command".
            let path = if chosen.extension().is_none() {
                chosen.with_extension("hta")
            } else {
                chosen
            };

            if let Err(error) = std::fs::write(&path, htmlapp_build::BLANK_APP) {
                tracing::error!(%error, path = %path.display(), "could not write the new app");
                return;
            }

            // Hand it to the user's text handler so they land in an editor, not an empty window.
            if let Err(error) = open::that_detached(&path) {
                tracing::warn!(%error, "could not hand the new file to a text editor");
            }
        });
    }

    fn manage_permissions(&self) {
        let Ok(exe) = std::env::current_exe() else { return };
        // A separate process keeps the manager's window independent of the launcher's.
        let _ = std::process::Command::new(exe)
            .arg("--permissions-ui")
            .stdin(std::process::Stdio::null())
            .spawn();
    }

    fn copy_to_clipboard(&self, text: &str) {
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
            Ok(()) => tracing::info!("diagnostics copied"),
            Err(error) => tracing::warn!(%error, "could not reach the clipboard"),
        }
    }

    fn open_url(&self, url: &str) {
        if let Err(error) = open::that_detached(url) {
            tracing::warn!(%error, %url, "could not open the link");
        }
    }

    fn revoke_permissions(&self, path: &Path) {
        // Revoked by path *and* by current content hash, so both a moved file and an edited one
        // are covered — the same thing `htmlapp permissions revoke` does.
        let Ok(mut store) = htmlapp_caps::ConsentStore::load_default() else {
            return;
        };
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut removed = store.revoke_path(&canonical);
        if let Ok(document) = htmlapp_caps::Document::load(&canonical) {
            removed += store.revoke_hash(&document.hash);
        }
        if let Err(error) = store.save_default() {
            tracing::error!(%error, "could not save the consent store");
        } else {
            tracing::info!(removed, path = %canonical.display(), "revoked permissions");
        }
    }

    fn reveal(&self, path: &Path) {
        let target: PathBuf = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.to_path_buf())
        };
        if let Err(error) = open::that_detached(&target) {
            tracing::warn!(%error, "could not open the file manager");
        }
    }
}

/// Open the launcher window (docs/building.md).
pub fn run_launcher(open_immediately: bool, permissions_ui: bool) -> Result<()> {
    let delegate = CliDelegate::new()?;

    if permissions_ui {
        return htmlapp_runtime::app::run_permissions_manager().map_err(Into::into);
    }

    // The process and instance model: "The launcher is single-instance." Documents are the opposite — one process each — so
    // The guard applies only here. Held for the life of the process.
    let Some(_lock) = htmlapp_runtime::single_instance::acquire() else {
        eprintln!(
            "htmlapp: a launcher is already running.\n\
             Open a document with `htmlapp <file.hta>`, or use the running window."
        );
        return Ok(());
    };

    if open_immediately {
        // The desktop entry's "Open an .hta file…" action: go straight to the chooser, and only
        // fall back to the launcher window if nothing is chosen.
        delegate.choose_document();
    }

    htmlapp_runtime::app::run_launcher(delegate)?;
    Ok(())
}
