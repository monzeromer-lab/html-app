//! The wry backend: WebKitGTK embedded as a child surface of the host window.
//!
//! # How it attaches
//!
//! `wry` takes the host window's `RawWindowHandle::Xlib` id, creates an X11 child window inside it
//! with `XCreateSimpleWindow`, wraps that in a foreign `GdkWindow`, and builds a WebKitGTK view in
//! The resulting GTK container. Three consequences follow from that, and all three are load-bearing
//! for the rest of the runtime:
//!
//! 1. **The host window must be X11.** A `RawWindowHandle::Wayland` is rejected outright. On a
//!    Wayland session this means going through XWayland — see [`force_x11_session`].
//! 2. **GTK must be initialised** before the view is built, because `gdk::Display::default()` has
//!    to return a display.
//! 3. **The GTK main loop must be pumped.** GPUI runs its own xcb event loop and knows nothing
//!    about GLib, so nothing would otherwise drive WebKit's timers, painting, or input. See
//!    [`pump_events`].
//!
//! # What this backend cannot do
//!
//! It is a native child surface, so it paints *above* everything GPUI draws into the same window
//! and is not clipped by the parent's rounding or overflow. That is exactly the limitation the rendering design
//! sets out to remove, and removing it is what the WPE offscreen path in the render pipeline is for. The
//! [`WebEngine`](crate::WebEngine) trait exists so that swap is an addition rather than a rewrite.

use std::borrow::Cow;
use std::sync::Arc;

use raw_window_handle::HasWindowHandle;
use wry::http::{Request, Response as HttpResponse, StatusCode, header};

use crate::origin::{self, OriginResolver, Response as OriginResponse};
use crate::{
    EngineCallbacks, EngineConfig, EngineError, EngineEvent, RenderPath, Result, ViewRect,
    WebEngine,
};

/// Make GPUI choose its X11 backend.
///
/// GPUI picks Wayland whenever `WAYLAND_DISPLAY` is set and non-empty, and falls back to X11 when
/// only `DISPLAY` is. Since this backend cannot attach to a Wayland surface at all, the choice has
/// to be made *before* the first window is opened — otherwise the runtime opens a window it then
/// cannot put a webview into.
///
/// Returns `false` if the session offers no X11 display to fall back to, in which case the caller
/// should report that rather than open a broken window.
pub fn force_x11_session() -> bool {
    let has_x11 = std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty());
    if !has_x11 {
        return false;
    }

    let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty());
    if on_wayland {
        tracing::info!(
            "wry backend cannot attach to a Wayland surface; using XWayland via DISPLAY instead"
        );
    }

    // SAFETY: `set_var`/`remove_var` are unsafe because they race with concurrent readers of the
    // environment. This runs during startup, before the runtime spawns any thread and before GPUI
    // or GTK have read a display variable, so no other thread can observe the change.
    #[allow(unsafe_code)]
    unsafe {
        // Clearing this is what makes GPUI pick its X11 backend rather than its Wayland one.
        std::env::remove_var("WAYLAND_DISPLAY");

        // Clearing it is *not* enough for GTK. `wl_display_connect(NULL)` falls back to
        // `$XDG_RUNTIME_DIR/wayland-0` when the variable is unset, so GDK would still open a
        // Wayland display — and `gdk::Display::default().downcast_ref::<X11Display>()` inside wry
        // would then return `None` and panic. Naming the backend explicitly is the only way to be
        // sure both halves of the process agree on X11.
        std::env::set_var("GDK_BACKEND", "x11");
    }

    true
}

/// Initialise GTK. Safe to call more than once.
pub fn init_toolkit() -> Result<()> {
    if gtk::init().is_err() {
        return Err(EngineError::Startup(
            "could not initialise GTK; the wry backend needs an X11 display".into(),
        ));
    }
    Ok(())
}

/// Drain pending GTK events.
///
/// GPUI owns the process's event loop, so this has to be called from it — once per frame — or the
/// webview never paints, never sees input, and never runs a timer. Draining is bounded so that a
/// page generating events faster than they can be processed cannot starve the host's own frame.
pub fn pump_events() {
    const MAX_ITERATIONS: usize = 64;
    for _ in 0..MAX_ITERATIONS {
        if !gtk::events_pending() {
            break;
        }
        gtk::main_iteration_do(false);
    }
}

/// A WebKitGTK view embedded in the host window.
pub struct WryEngine {
    webview: wry::WebView,
    /// Shapes the page's child window so the host can paint over it (docs/architecture.md, G2).
    ///
    /// `RefCell` rather than a lock: the engine has main-thread affinity and is only ever reached
    /// from the frame loop, so there is no contention to guard against.
    occluder: std::cell::RefCell<Option<crate::occlusion::Occluder>>,
    /// The host window, kept so the occluder can be built lazily once the child window exists.
    parent: std::cell::Cell<u32>,
    /// The page's current extent, needed to reset the shape before subtracting holes.
    extent: std::cell::Cell<(u16, u16)>,
}

impl WryEngine {
    /// Build the view as a child of `parent`.
    ///
    /// `parent` must be an X11 window; [`force_x11_session`] and [`init_toolkit`] must both have
    /// run first.
    pub fn new<W: HasWindowHandle>(
        parent: &W,
        config: EngineConfig,
        callbacks: EngineCallbacks,
        bounds: ViewRect,
    ) -> Result<Self> {
        let resolver = Arc::new(OriginResolver::with_blobs(
            config.index_html.clone(),
            config.asset_root.clone(),
            config.module_root.clone(),
            config.blobs.clone(),
        ));

        let on_event = Arc::clone(&callbacks.on_event);
        let ipc_event = Arc::clone(&callbacks.on_event);
        let navigation_manifest = config.manifest.clone();

        let builder = wry::WebViewBuilder::new()
            .with_url(origin::INDEX_URL)
            .with_transparent(config.transparent)
            .with_devtools(config.devtools)
            .with_bounds(to_wry_rect(bounds))
            // The bridge transport: injected at document-start, before any page script runs.
            .with_initialization_script(&config.init_script)
            .with_custom_protocol(origin::SCHEME.to_string(), move |_id, request| {
                serve(&resolver, &request)
            })
            .with_ipc_handler(move |request: Request<String>| {
                ipc_event(EngineEvent::Ipc(request.into_body()));
            })
            // The goals and non-goals, N2 and the security model, rule 8: this is not a browser.
            .with_navigation_handler(move |url: String| {
                let allowed = origin::allows_navigation(&url, &navigation_manifest);
                if !allowed {
                    tracing::warn!(%url, "blocked off-origin navigation");
                    on_event(EngineEvent::NavigationBlocked(url));
                }
                allowed
            })
            // The API catalog `dnd`: dropping files onto a tool is the common case, and the drop lands on
            // The page's own child window rather than on the host's.
            .with_drag_drop_handler({
                let on_event = Arc::clone(&callbacks.on_event);
                move |event: wry::DragDropEvent| {
                    let translated = match event {
                        wry::DragDropEvent::Enter { paths, position } => {
                            crate::DragDropEvent::Enter {
                                paths,
                                x: position.0 as f32,
                                y: position.1 as f32,
                            }
                        }
                        wry::DragDropEvent::Over { position } => crate::DragDropEvent::Over {
                            x: position.0 as f32,
                            y: position.1 as f32,
                        },
                        wry::DragDropEvent::Drop { paths, position } => {
                            crate::DragDropEvent::Drop {
                                paths,
                                x: position.0 as f32,
                                y: position.1 as f32,
                            }
                        }
                        _ => crate::DragDropEvent::Leave,
                    };
                    on_event(EngineEvent::DragDrop(translated));
                    // `false` lets the page see the drop too, so a document can use ordinary HTML
                    // drag events if it prefers them to the bridge.
                    false
                }
            })
            // A page that calls window.open() must not get an unmanaged, ungoverned second window.
            .with_new_window_req_handler(|url: String, _features| {
                tracing::info!(%url, "refused window.open; use htmlapp.window.open instead");
                wry::NewWindowResponse::Deny
            });

        let webview = builder
            .build_as_child(parent)
            .map_err(|e| EngineError::Startup(describe_build_failure(&e)))?;

        Ok(Self {
            webview,
            occluder: std::cell::RefCell::new(None),
            parent: std::cell::Cell::new(0),
            extent: std::cell::Cell::new((
                bounds.width.max(1.0) as u16,
                bounds.height.max(1.0) as u16,
            )),
        })
    }
}

/// Presents a bare X11 window id to `wry` as a `RawWindowHandle`.
///
/// GPUI will not hand out its own handle (see [`crate::x11_window`]), so the id is recovered from
/// The X server and re-wrapped here. `wry` matches specifically on `RawWindowHandle::Xlib`, while
/// GPUI's internal handle is an XCB one — the distinction is only which client library is used to
/// talk to the server. The window id itself is the same XID either way.
struct ForeignX11Window(std::ffi::c_ulong);

impl HasWindowHandle for ForeignX11Window {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let handle = raw_window_handle::XlibWindowHandle::new(self.0);
        // SAFETY: the caller guarantees this id names a window that outlives the returned handle.
        // In practice the GPUI window is owned by the application and is destroyed only when the
        // process exits, well after the engine that borrows it.
        #[allow(unsafe_code)]
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl WryEngine {
    /// Build a view as a child of an X11 window identified by id.
    ///
    /// Used when the host toolkit owns a window it will not describe — see [`crate::x11_window`]
    /// for why that is the situation with GPUI 0.2.2.
    pub fn new_in_x11_window(
        window_id: u32,
        config: EngineConfig,
        callbacks: EngineCallbacks,
        bounds: ViewRect,
    ) -> Result<Self> {
        let engine = Self::new(
            &ForeignX11Window(window_id as std::ffi::c_ulong),
            config,
            callbacks,
            bounds,
        )?;
        // Remembered so the occluder can find the page's child window on a later frame — it does
        // not exist yet at the instant `build_as_child` returns.
        engine.parent.set(window_id);
        Ok(engine)
    }
}

impl WryEngine {
    /// Build a view with no visible surface, for headless mode (docs/bridge.md).
    ///
    /// This path does not embed into anything, so it does not need X11: the view goes into an
    /// ordinary GTK window that is never shown, which works just as well on Wayland. That matters
    /// because a document in a shell pipeline should not need a display server it never draws to.
    pub fn new_headless(config: EngineConfig, callbacks: EngineCallbacks) -> Result<Self> {
        use wry::WebViewBuilderExtUnix as _;

        let resolver = Arc::new(OriginResolver::with_blobs(
            config.index_html.clone(),
            config.asset_root.clone(),
            config.module_root.clone(),
            config.blobs.clone(),
        ));
        let ipc_event = Arc::clone(&callbacks.on_event);
        let navigation_manifest = config.manifest.clone();

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        gtk::prelude::ContainerExt::add(&window, &container);
        // Realised but never shown: WebKit needs a widget hierarchy to attach to, but nothing here
        // should ever appear on a user's screen.
        gtk::prelude::WidgetExt::realize(&window);

        let webview = wry::WebViewBuilder::new()
            .with_url(origin::INDEX_URL)
            .with_initialization_script(&config.init_script)
            .with_custom_protocol(origin::SCHEME.to_string(), move |_id, request| {
                serve(&resolver, &request)
            })
            .with_ipc_handler(move |request: Request<String>| {
                ipc_event(EngineEvent::Ipc(request.into_body()));
            })
            .with_navigation_handler(move |url: String| {
                origin::allows_navigation(&url, &navigation_manifest)
            })
            .build_gtk(&container)
            .map_err(|e| EngineError::Startup(describe_build_failure(&e)))?;

        // Kept alive for as long as the engine is: dropping the window destroys the webview.
        std::mem::forget(window);

        Ok(Self {
            webview,
            occluder: std::cell::RefCell::new(None),
            parent: std::cell::Cell::new(0),
            extent: std::cell::Cell::new((1, 1)),
        })
    }
}

/// Turn a wry build error into something a user can act on.
pub(crate) fn describe_build_failure(error: &wry::Error) -> String {
    let base = error.to_string();
    if matches!(error, wry::Error::UnsupportedWindowHandle) {
        format!(
            "{base} — the wry backend needs an X11 window, but the host window is not one. \
             On a Wayland session, XWayland must be available (DISPLAY set)."
        )
    } else {
        base
    }
}

/// Serve one `htmlapp://app/...` request.
pub(crate) fn serve(resolver: &OriginResolver, request: &Request<Vec<u8>>) -> HttpResponse<Cow<'static, [u8]>> {
    let path = request.uri().path().to_string();

    match resolver.resolve(&path) {
        OriginResponse::Ok { body, content_type } => HttpResponse::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            // The document's own origin is the only thing that should ever read these.
            .header("Access-Control-Allow-Origin", origin::ORIGIN)
            .header("Cross-Origin-Resource-Policy", "same-origin")
            .body(Cow::Owned(body))
            .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR)),

        OriginResponse::NotFound => empty(StatusCode::NOT_FOUND),
        OriginResponse::Forbidden => empty(StatusCode::FORBIDDEN),
    }
}

fn empty(status: StatusCode) -> HttpResponse<Cow<'static, [u8]>> {
    HttpResponse::builder()
        .status(status)
        .body(Cow::Borrowed(&[][..]))
        .expect("static response is well-formed")
}

fn to_wry_rect(bounds: ViewRect) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(bounds.x, bounds.y).into(),
        size: wry::dpi::LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0)).into(),
    }
}

impl WebEngine for WryEngine {
    fn render_path(&self) -> RenderPath {
        RenderPath::NativeChild
    }

    fn evaluate(&self, script: &str) -> Result<()> {
        self.webview
            .evaluate_script(script)
            .map_err(|e| EngineError::Script(e.to_string()))
    }

    fn set_bounds(&self, bounds: ViewRect) -> Result<()> {
        self.extent.set((
            bounds.width.max(1.0) as u16,
            bounds.height.max(1.0) as u16,
        ));
        self.webview
            .set_bounds(to_wry_rect(bounds))
            .map_err(|e| EngineError::Script(e.to_string()))
    }

    /// The goals and non-goals, G2: let the host paint over the page.
    fn set_occlusions(&self, rects: &[ViewRect]) -> Result<bool> {
        let parent = self.parent.get();
        if parent == 0 {
            return Ok(false);
        }

        let mut slot = self.occluder.borrow_mut();
        if slot.is_none() {
            // Built lazily: the page's child window is created by `wry` inside `build_as_child`
            // and is not necessarily visible to a fresh X connection until a little later.
            *slot = crate::occlusion::Occluder::new(parent);
        }
        let Some(occluder) = slot.as_mut() else {
            return Ok(false);
        };

        occluder.set_occlusions(self.extent.get(), &crate::occlusion::merge(rects));
        Ok(true)
    }

    fn supports_occlusion(&self) -> bool {
        self.parent.get() != 0 && crate::occlusion::is_supported()
    }

    fn set_visible(&self, visible: bool) -> Result<()> {
        self.webview
            .set_visible(visible)
            .map_err(|e| EngineError::Script(e.to_string()))
    }

    fn focus(&self) -> Result<()> {
        self.webview
            .focus()
            .map_err(|e| EngineError::Script(e.to_string()))
    }

    fn reload(&self) -> Result<()> {
        self.webview
            .reload()
            .map_err(|e| EngineError::Script(e.to_string()))
    }

    fn open_devtools(&self) {
        #[cfg(feature = "devtools")]
        self.webview.open_devtools();
    }

    fn backend_name(&self) -> &'static str {
        "wry (WebKitGTK, native child surface)"
    }

    fn engine_version(&self) -> String {
        let (major, minor, micro) = webkit_version();
        format!("WebKitGTK {major}.{minor}.{micro}")
    }
}

/// The WebKitGTK version this process is linked against, for the launcher's diagnostics strip.
///
/// SAFETY: these three are pure getters in libwebkit2gtk. They take no arguments, read only static
/// version constants compiled into the library, and cannot fail or allocate.
#[allow(unsafe_code)]
fn webkit_version() -> (u32, u32, u32) {
    unsafe {
        (
            webkit2gtk_sys::webkit_get_major_version(),
            webkit2gtk_sys::webkit_get_minor_version(),
            webkit2gtk_sys::webkit_get_micro_version(),
        )
    }
}
