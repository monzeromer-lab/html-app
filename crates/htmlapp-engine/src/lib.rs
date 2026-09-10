//! The web engine abstraction and its backends (PRD §6.2).
//!
//! # Which backend, and why it matters
//!
//! The PRD's target backend is WPE WebKit rendered **offscreen** and composited *into* GPUI's
//! scene as a texture (§6.1). That is the architectural commitment of the project, and it is what
//! makes modals paint over the page, `rounded()` clip it, and opacity and transforms apply to it.
//!
//! The backend that ships today is [`wry`], embedded as a **native child surface** inside the GPUI
//! window. This is a different thing, and the difference is worth stating plainly rather than
//! burying:
//!
//! | | Offscreen (WPE, §6.3) | Native child (wry, today) |
//! |---|---|---|
//! | Native UI over the page | Yes | **No** — the page paints last |
//! | Parent `rounded()` / `overflow_hidden()` clips it | Yes | **No** |
//! | Wayland | Yes | **X11/XWayland only** |
//!
//! The Wayland restriction is not this crate's choice. `wry::WebViewBuilder::build_as_child`
//! documents it directly: "Linux: Only X11 is supported". Embedding into a foreign window needs
//! reparenting, and Wayland has no cross-toplevel reparenting to offer. [`prefers_x11`] exists so
//! the runtime can select a backend that works before it opens a window it cannot fill.
//!
//! Everything above this trait — the bridge, the capability model, the API modules, the shell — is
//! written against the trait rather than any backend, so the WPE path in §6.3 is an addition here
//! rather than a rewrite everywhere.

// `deny` rather than `forbid`: this is the crate that touches the engine's C libraries, so a
// small number of audited `unsafe` blocks live in the backend. Every other crate in the workspace
// forbids it outright.
#![deny(unsafe_code)]

pub mod imports;
pub mod origin;

#[cfg(feature = "wry-backend")]
pub mod occlusion;
#[cfg(feature = "wry-backend")]
pub mod wry_backend;
#[cfg(feature = "wry-backend")]
pub mod x11_window;

use std::path::PathBuf;
use std::sync::Arc;

use htmlapp_caps::Manifest;

pub use imports::{ImportError, ModuleCache, ModuleFetcher};
pub use origin::{INDEX_URL, ORIGIN, OriginResolver, Response as OriginResponse, SCHEME};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine failed to start: {0}")]
    Startup(String),

    #[error("script evaluation failed: {0}")]
    Script(String),

    #[error("no engine backend is available: {0}")]
    Unavailable(String),

    #[error(transparent)]
    Import(#[from] ImportError),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// How a backend gets pixels from the engine onto the screen (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPath {
    /// §6.3 stage 1. Engine renders offscreen to a CPU buffer, uploaded into a GPUI image.
    /// Composites correctly; upload cost is proportional to the damaged region.
    Shm,
    /// §6.3 stage 2. Zero-copy dmabuf imported as a Vulkan image and drawn by GPUI.
    DmaBuf,
    /// A native child surface positioned over the host window. Cheap, but it paints above
    /// everything the host draws and cannot be clipped by it.
    NativeChild,
    /// No surface at all (§9.4).
    Headless,
}

impl RenderPath {
    /// Whether the host can paint UI *over* the page — the §4.2 G2 promise.
    pub fn composites(self) -> bool {
        matches!(self, RenderPath::Shm | RenderPath::DmaBuf)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RenderPath::Shm => "shm",
            RenderPath::DmaBuf => "dmabuf",
            RenderPath::NativeChild => "native-child",
            RenderPath::Headless => "headless",
        }
    }
}

/// A rectangle in logical pixels, relative to the host window's client area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl ViewRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }
}

/// Something that happened in the engine and that the runtime may need to react to.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// The page posted a message over the bridge.
    Ipc(String),
    /// The document finished loading.
    Loaded,
    /// The page tried to navigate. Already checked against [`origin::allows_navigation`].
    NavigationBlocked(String),
    /// `document.title` changed.
    TitleChanged(String),
    /// A drag entered, moved over, dropped on, or left the page (§9.3 `dnd`).
    DragDrop(DragDropEvent),
}

/// A file drag over the page.
#[derive(Debug, Clone)]
pub enum DragDropEvent {
    Enter { paths: Vec<PathBuf>, x: f32, y: f32 },
    Over { x: f32, y: f32 },
    Drop { paths: Vec<PathBuf>, x: f32, y: f32 },
    Leave,
}

/// Callbacks the runtime installs before the engine starts.
pub struct EngineCallbacks {
    /// Invoked for every message the page posts. Called on the engine's own thread, so it should
    /// hand work off rather than block.
    pub on_event: Arc<dyn Fn(EngineEvent) + Send + Sync>,
}

impl std::fmt::Debug for EngineCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineCallbacks").finish_non_exhaustive()
    }
}

/// Everything a backend needs to bring up a document.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// The prepared HTML — CSP injected, import map rewritten — served at `htmlapp://app/`.
    pub index_html: String,
    /// Sibling asset directory, if the manifest opted in (§8.4).
    pub asset_root: Option<PathBuf>,
    /// Cached remote modules (§8.5).
    pub module_root: Option<PathBuf>,
    /// The bridge shim, injected at document-start before any page script runs (§9.1).
    pub init_script: String,
    /// Used for the navigation policy and the window's background treatment.
    pub manifest: Manifest,
    /// Whether the surface should be alpha-capable.
    pub transparent: bool,
    /// Whether to make devtools reachable. Off unless asked for.
    pub devtools: bool,
    /// Paths the page may fetch directly by token (§9.1). Shared with the `fs` module, which is
    /// what mints the tokens.
    pub blobs: htmlapp_bridge::BlobStore,
}

/// A running web engine hosting one document.
///
/// Implementations are used from the thread that created them; the runtime never moves one across
/// threads, because both backends have main-thread affinity.
pub trait WebEngine {
    /// How this engine is getting pixels to the screen.
    fn render_path(&self) -> RenderPath;

    /// Run script inside the page. This is the host→page half of the bridge.
    fn evaluate(&self, script: &str) -> Result<()>;

    /// Position the view within the host window. Meaningful for [`RenderPath::NativeChild`];
    /// offscreen backends use it to size their buffer.
    fn set_bounds(&self, bounds: ViewRect) -> Result<()>;

    fn set_visible(&self, visible: bool) -> Result<()>;

    /// Give the page keyboard focus.
    fn focus(&self) -> Result<()>;

    fn reload(&self) -> Result<()>;

    /// Punch holes in the page so the host can paint over it (§4.2 G2).
    ///
    /// Rects are in logical pixels relative to the page's own origin. An empty slice restores the
    /// page to its full extent.
    ///
    /// The default does nothing and reports `false`, which is the honest answer for a backend that
    /// composites properly: an offscreen engine has no surface to punch a hole in, because the host
    /// is already drawing over it in its own scene.
    fn set_occlusions(&self, _rects: &[ViewRect]) -> Result<bool> {
        Ok(false)
    }

    /// Whether this engine can be occluded, so the runtime knows whether it must fall back to
    /// hiding the page while a modal is up.
    fn supports_occlusion(&self) -> bool {
        false
    }

    /// Open the engine's inspector, if this build has one.
    fn open_devtools(&self) {}

    /// A human-readable backend name for the launcher's diagnostics strip (§7.2).
    fn backend_name(&self) -> &'static str;

    /// The engine version, for the same diagnostics strip.
    fn engine_version(&self) -> String {
        "unknown".into()
    }
}

/// Whether the active backend needs an X11 surface.
///
/// The runtime calls this *before* opening a window: on a Wayland session the wry backend can only
/// work through XWayland, so GPUI has to be told to take the X11 path or the webview will have
/// nothing it can attach to. Returning `false` on an offscreen backend is what lets the WPE path
/// stay Wayland-native.
pub fn prefers_x11() -> bool {
    cfg!(all(feature = "wry-backend", not(feature = "wpe-backend")))
}

/// Which backends this build actually contains, for diagnostics.
pub fn available_backends() -> Vec<&'static str> {
    let mut backends = Vec::new();
    if cfg!(feature = "wry-backend") {
        backends.push("wry");
    }
    if cfg!(feature = "wpe-backend") {
        backends.push("wpe");
    }
    backends
}
