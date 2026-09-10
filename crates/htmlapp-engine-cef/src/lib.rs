//! The CEF backend (the engine choice, the crate layout, the risk register, R4).
//!
//! # Status: designed, not implemented
//!
//! The engine choice keeps CEF as "a **future optional backend** behind a feature flag for users who need
//! Chromium parity", and the risk register, R4 names the gap it closes: WebKit lags Chromium on WebGPU,
//! WebCodecs, and newer CSS. A document that needs those needs Chromium, and no amount of
//! WebKit-side work changes that.
//!
//! This crate exists so the seam is real rather than hypothetical. It is a workspace member, it
//! implements nothing, and turning it on is a deliberate act.
//!
//! # What implementing it involves
//!
//! Three things, in rough order of how much they will hurt:
//!
//! 1. **A multi-process bootstrap.** CEF requires the host binary to re-exec itself as renderer,
//!    GPU, and utility subprocesses, dispatching on `--type=` before anything else runs — before
//!    argument parsing, before `main` does any work. That interacts directly with the stapled
//!    binary detection in [`htmlapp_build::staple`], which also runs first.
//! 2. **Offscreen rendering.** `CefRenderHandler::OnPaint` hands back a dirty-rect buffer, which is
//!    exactly the render pipeline, Stage 1 shape: upload into a `gpui::RenderImage` and composite. This backend
//!    would therefore be the *first* one to satisfy the goals and non-goals, G2 properly, rather than through the X11
//!    SHAPE workaround the wry backend uses.
//! 3. **Size.** the engine choice puts it at 150–250 MB shipped, against a target of under 40 MB for the
//!    runtime. That is why it is opt-in and why WebKit remains the default.
//!
//! # What does not need to change
//!
//! Nothing above [`htmlapp_engine::WebEngine`]. The bridge, the capability model, the API modules,
//! and the shell are written against that trait, so this is an addition here rather than a rewrite
//! anywhere else — which is the whole reason the engine choice insists the trait exist "from day one".

#![forbid(unsafe_code)]

use htmlapp_engine::{EngineCallbacks, EngineConfig, EngineError, Result, WebEngine};

/// Whether this build contains a usable CEF backend.
pub fn is_available() -> bool {
    cfg!(feature = "cef")
}

/// Why it is not available, in a form worth showing a user.
pub fn unavailable_reason() -> &'static str {
    if cfg!(feature = "cef") {
        "the CEF backend is compiled in but not implemented yet"
    } else {
        "this build was compiled without the `cef` feature"
    }
}

/// A page hosted by Chromium Embedded Framework, rendered offscreen.
pub struct CefEngine {
    _private: (),
}

impl CefEngine {
    /// Start a CEF-hosted page.
    ///
    /// Returns [`EngineError::Unavailable`] in every build today. The signature matches the other
    /// backends so the runtime's selection logic can name this one without conditional plumbing.
    pub fn new(_config: EngineConfig, _callbacks: EngineCallbacks) -> Result<Self> {
        Err(EngineError::Unavailable(format!(
            "the CEF backend is not implemented: {}. \
             The default WebKit backend covers everything except Chromium-only features \
             (WebGPU, WebCodecs); see the risk register, R4.",
            unavailable_reason()
        )))
    }

    /// The subprocess dispatch CEF requires, called before anything else in `main`.
    ///
    /// Returns `true` when this process is a CEF subprocess and has finished its work, in which
    /// case the caller must exit immediately without touching argv, the manifest, or the stapled
    /// document trailer.
    pub fn run_subprocess_if_needed() -> bool {
        // Without CEF linked there are no subprocesses to be.
        false
    }
}

impl WebEngine for CefEngine {
    fn render_path(&self) -> htmlapp_engine::RenderPath {
        // The render pipeline, Stage 1: `OnPaint` gives a CPU buffer with damage rectangles.
        htmlapp_engine::RenderPath::Shm
    }

    fn evaluate(&self, _script: &str) -> Result<()> {
        Err(EngineError::Unavailable(unavailable_reason().into()))
    }

    fn set_bounds(&self, _bounds: htmlapp_engine::ViewRect) -> Result<()> {
        Err(EngineError::Unavailable(unavailable_reason().into()))
    }

    fn set_visible(&self, _visible: bool) -> Result<()> {
        Err(EngineError::Unavailable(unavailable_reason().into()))
    }

    fn focus(&self) -> Result<()> {
        Err(EngineError::Unavailable(unavailable_reason().into()))
    }

    fn reload(&self) -> Result<()> {
        Err(EngineError::Unavailable(unavailable_reason().into()))
    }

    fn backend_name(&self) -> &'static str {
        "cef (not implemented)"
    }

    /// An offscreen backend needs no occlusion: the host is already drawing over it in its own
    /// scene, which is the point of the rendering design.
    fn supports_occlusion(&self) -> bool {
        false
    }
}
