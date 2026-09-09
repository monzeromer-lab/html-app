//! Preparing one document to run (PRD §8, §11.2).
//!
//! Everything here happens before a window exists, and none of it needs one: load the file, read
//! its manifest, resolve consent, resolve pinned imports, inject the CSP, and build the shim. That
//! separation is what lets the whole startup path be tested without starting an engine.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use htmlapp_bridge::{Dispatcher, ShimConfig};
use htmlapp_caps::{
    ConsentDecision, ConsentStore, Document, PermissionDiff, Permissions, WindowMode,
};
use htmlapp_engine::{EngineConfig, ModuleCache, ModuleFetcher, origin};

use crate::fetcher::HttpFetcher;

/// The version reported to the page as `htmlapp.version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Caps(#[from] htmlapp_caps::CapsError),

    #[error(transparent)]
    Engine(#[from] htmlapp_engine::EngineError),

    #[error(transparent)]
    Import(#[from] htmlapp_engine::ImportError),

    #[error("{0}")]
    Refused(String),
}

pub type Result<T> = std::result::Result<T, SessionError>;

/// How a document's permissions were settled before it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentOutcome {
    /// Nothing was requested (§11.2 rule 1).
    NotRequired,
    /// A stored, hash-pinned grant applied.
    Remembered,
    /// The user must be asked. Carries what the sheet needs to show.
    Prompt {
        diff: PermissionDiff,
        previously_seen: bool,
    },
    /// A stored refusal applied; the document runs as a plain page.
    PreviouslyRefused,
}

/// Options that change how a document is brought up.
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// Run with no surface (§9.4).
    pub headless: bool,
    /// Make the engine inspector reachable.
    pub devtools: bool,
    /// §17 open question 8: suppress recents for this run.
    pub private: bool,
    /// Skip the consent sheet and run powerless. Used by `--no-permissions`.
    pub force_powerless: bool,
}

/// A document that has been read and had its permissions settled.
pub struct Session {
    pub document: Document,
    pub options: SessionOptions,
    /// What the document will actually be allowed to do — `None` means powerless.
    pub granted: Option<Permissions>,
    pub consent: ConsentOutcome,
    /// Resolved import-map entries (§8.5).
    pub module_urls: Vec<(String, String)>,
    pub module_root: Option<PathBuf>,
    /// Shared between the `fs` module, which mints blob tokens, and the origin resolver, which
    /// serves them (§9.1).
    pub blobs: htmlapp_bridge::BlobStore,
}

impl Session {
    /// Load a document from disk and settle its consent against the stored decisions.
    pub fn load(path: impl AsRef<Path>, options: SessionOptions) -> Result<Self> {
        let document = Document::load(path)?;
        Self::prepare(document, options, &ConsentStore::load_default()?)
    }

    /// Same, for a document already in memory — a stapled binary (§13) or stdin.
    pub fn from_document(document: Document, options: SessionOptions) -> Result<Self> {
        Self::prepare(document, options, &ConsentStore::load_default()?)
    }

    /// Settle consent and resolve imports. Split out so tests can supply their own store.
    pub fn prepare(
        document: Document,
        options: SessionOptions,
        store: &ConsentStore,
    ) -> Result<Self> {
        let requested = document.manifest.permissions.as_ref();

        let (granted, consent) = if options.force_powerless {
            (None, ConsentOutcome::NotRequired)
        } else {
            match store.decide(&document.hash, document.source.as_deref(), requested) {
                ConsentDecision::NotRequired => (None, ConsentOutcome::NotRequired),
                ConsentDecision::AlreadyGranted(record) => {
                    (Some(record.permissions.clone()), ConsentOutcome::Remembered)
                }
                ConsentDecision::AlreadyDenied(_) => (None, ConsentOutcome::PreviouslyRefused),
                ConsentDecision::NeedsPrompt { diff, previous } => (
                    None,
                    ConsentOutcome::Prompt {
                        diff,
                        previously_seen: previous.is_some(),
                    },
                ),
            }
        };

        // Imports are resolved regardless of consent: they are page content, not a capability, and
        // the integrity pin is what governs them (§8.5).
        let (module_urls, module_root) = resolve_imports(&document)?;

        Ok(Self {
            document,
            options,
            granted,
            consent,
            module_urls,
            module_root,
            blobs: htmlapp_bridge::BlobStore::new(),
        })
    }

    /// A copy of this session, for handing to the window after consent is settled.
    ///
    /// `Session` is deliberately not `Clone`: everything in it is cheap to copy, but a stray clone
    /// would make it easy to apply consent to one copy and run another. This is the one place that
    /// hand-off is correct, so it is the one place that can make a copy.
    pub fn clone_for_launch(&self) -> Self {
        Self {
            document: self.document.clone(),
            options: self.options.clone(),
            granted: self.granted.clone(),
            consent: self.consent.clone(),
            module_urls: self.module_urls.clone(),
            module_root: self.module_root.clone(),
            blobs: self.blobs.clone(),
        }
    }

    /// Apply the user's answer from the consent sheet.
    pub fn apply_consent(&mut self, allow: bool, store: &mut ConsentStore) -> Result<()> {
        let Some(requested) = self.document.manifest.permissions.clone() else {
            return Ok(());
        };

        store.record(
            &self.document.hash,
            self.document.source.as_deref(),
            self.document.manifest.name.as_deref(),
            self.document.manifest.id.as_deref(),
            &requested,
            allow,
        );
        store.save_default()?;

        self.granted = allow.then_some(requested);
        self.consent = if allow {
            ConsentOutcome::Remembered
        } else {
            ConsentOutcome::PreviouslyRefused
        };
        Ok(())
    }

    /// Whether the consent sheet still needs to be shown.
    pub fn needs_prompt(&self) -> bool {
        matches!(self.consent, ConsentOutcome::Prompt { .. })
    }

    /// The mode this document asked to run in, forced to headless when the CLI said so.
    pub fn window_mode(&self) -> WindowMode {
        if self.options.headless {
            WindowMode::Headless
        } else {
            self.document.manifest.window.mode
        }
    }

    /// A stable identity for the store and consent record, falling back to the content hash so an
    /// unnamed document still gets private storage rather than sharing a global namespace.
    pub fn app_id(&self) -> String {
        self.document
            .manifest
            .id
            .clone()
            .unwrap_or_else(|| format!("unnamed-{}", self.document.short_hash()))
    }

    /// Everything the engine needs to bring the document up.
    pub fn engine_config(&self) -> EngineConfig {
        let headless = self.window_mode() == WindowMode::Headless;
        let shim = htmlapp_bridge::render_shim(&ShimConfig::for_permissions(
            VERSION,
            self.granted.as_ref(),
            headless,
        ));

        EngineConfig {
            index_html: origin::prepare_document(&self.document, &self.module_urls),
            asset_root: self.document.asset_root(),
            module_root: self.module_root.clone(),
            init_script: shim,
            manifest: self.document.manifest.clone(),
            transparent: self.document.manifest.window.background
                == htmlapp_caps::Background::Transparent,
            devtools: self.options.devtools,
            blobs: self.blobs.clone(),
        }
    }

    /// Register the modules this document was granted.
    pub fn register_modules(
        &self,
        dispatcher: &mut Dispatcher,
        host: Arc<dyn htmlapp_api::HostBridge>,
    ) -> std::result::Result<htmlapp_api::Registered, htmlapp_bridge::RpcError> {
        htmlapp_api::register_all(
            dispatcher,
            htmlapp_api::Registration {
                app_id: self.app_id(),
                app_name: self.document.manifest.display_name().to_string(),
                permissions: self.granted.clone(),
                host,
                headless: self.window_mode() == WindowMode::Headless,
                blobs: self.blobs.clone(),
            },
        )
    }

    /// Record this run in the launcher's recents, unless `--private` (§17 open question 8).
    pub fn record_recent(&self) {
        if self.options.private {
            return;
        }
        let Some(path) = self.document.source.as_deref() else {
            return;
        };

        let granted: Vec<String> = self
            .granted
            .as_ref()
            .map(|p| p.granted_modules().into_iter().map(str::to_string).collect())
            .unwrap_or_default();

        let mut recents = htmlapp_caps::Recents::load_default().unwrap_or_default();
        recents.push(
            path,
            self.document.manifest.name.as_deref(),
            &self.document.hash,
            granted,
        );
        if let Err(error) = recents.save_default() {
            tracing::warn!(%error, "could not update the recents list");
        }
    }
}

/// Resolve and cache the document's pinned imports.
fn resolve_imports(document: &Document) -> Result<(Vec<(String, String)>, Option<PathBuf>)> {
    if document.manifest.imports.is_empty() {
        return Ok((Vec::new(), None));
    }

    let cache = ModuleCache::default_cache();
    let fetcher = HttpFetcher::new();
    let urls = cache.resolve_all(&document.manifest.imports, &fetcher as &dyn ModuleFetcher)?;
    Ok((urls, Some(cache.root().to_path_buf())))
}
