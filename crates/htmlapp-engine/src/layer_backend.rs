//! Layer-shell surfaces (docs/document-format.md).
//!
//! # Why this does not go through GPUI
//!
//! A `zwlr_layer_surface_v1` is a surface the *compositor* positions and sizes. The client still has
//! to render into it, and `gpui` exposes no way to create a window from a foreign Wayland surface.
//! So a bar built on GPUI would have a shell and nothing to draw with.
//!
//! It does not need to. A bar has no titlebar, no consent sheet, and no native views — the page
//! *is* the whole UI. So `mode: "layer"` skips GPUI entirely: a GTK window, `gtk-layer-shell`
//! applied to it, and the WebKitGTK view filling it. GTK talks Wayland natively, so this path is
//! Wayland-native in a way the ordinary document window is not.
//!
//! # What it costs
//!
//! No `htmlapp-view` native views and no GPUI-painted overlays in layer mode, because there is no
//! GPUI scene to draw them into. `menu`, `palette`, and `dialog` therefore report unsupported. For
//! a bar or a widget that is the right trade; for anything else, use `mode: "window"`.

use std::sync::Arc;

use gtk::prelude::{ContainerExt as _, GtkWindowExt as _, WidgetExt as _};
use gtk_layer_shell::LayerShell as _;
use htmlapp_caps::{Anchor, KeyboardInteractivity, Layer, WindowMode, WindowSpec};
use wry::http::Request;

use crate::origin::{self, OriginResolver};
use crate::wry_backend::{describe_build_failure, serve};
use crate::{
    EngineCallbacks, EngineConfig, EngineError, EngineEvent, RenderPath, Result, ViewRect,
    WebEngine,
};

/// A page hosted directly in a layer-shell surface.
pub struct LayerEngine {
    webview: wry::WebView,
    /// Kept alive: dropping the window destroys the surface.
    _window: gtk::Window,
}

impl LayerEngine {
    /// Create the surface and put the page in it.
    ///
    /// `mode` selects between an ordinary layer surface and an overlay-layer one used for
    /// `mode: "lock"`. True `ext-session-lock` is a different protocol and is not implemented; a
    /// lock document therefore gets an exclusive-focus overlay, which is *not* a security boundary
    /// and is documented as such rather than quietly presented as one.
    pub fn new(
        config: EngineConfig,
        callbacks: EngineCallbacks,
        window_spec: &WindowSpec,
        mode: WindowMode,
    ) -> Result<Self> {
        if gtk::init().is_err() {
            return Err(EngineError::Startup(
                "could not initialise GTK for a layer-shell surface".into(),
            ));
        }

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        if !gtk_layer_shell::is_supported() {
            return Err(EngineError::Unavailable(
                "this compositor does not implement wlr-layer-shell".into(),
            ));
        }

        window.init_layer_shell();
        window.set_layer(match (mode, window_spec.layer) {
            // A lock surface has to be above everything, whatever the manifest asked for.
            (WindowMode::Lock, _) => gtk_layer_shell::Layer::Overlay,
            (_, Layer::Background) => gtk_layer_shell::Layer::Background,
            (_, Layer::Bottom) => gtk_layer_shell::Layer::Bottom,
            (_, Layer::Top) => gtk_layer_shell::Layer::Top,
            (_, Layer::Overlay) => gtk_layer_shell::Layer::Overlay,
        });
        window.set_namespace("htmlapp");

        for anchor in &window_spec.anchor {
            let edge = match anchor {
                Anchor::Top => gtk_layer_shell::Edge::Top,
                Anchor::Bottom => gtk_layer_shell::Edge::Bottom,
                Anchor::Left => gtk_layer_shell::Edge::Left,
                Anchor::Right => gtk_layer_shell::Edge::Right,
            };
            window.set_anchor(edge, true);
        }

        if let Some(margin) = window_spec.margin {
            window.set_layer_shell_margin(gtk_layer_shell::Edge::Top, margin.top);
            window.set_layer_shell_margin(gtk_layer_shell::Edge::Right, margin.right);
            window.set_layer_shell_margin(gtk_layer_shell::Edge::Bottom, margin.bottom);
            window.set_layer_shell_margin(gtk_layer_shell::Edge::Left, margin.left);
        }

        match window_spec.exclusive_zone {
            Some(zone) if zone > 0 => window.set_exclusive_zone(zone),
            // Anything else means "do not reserve space", which is the default for an overlay.
            _ => {}
        }

        window.set_keyboard_mode(match (mode, window_spec.keyboard_interactivity) {
            // A lock surface that cannot take the keyboard cannot take a password.
            (WindowMode::Lock, _) => gtk_layer_shell::KeyboardMode::Exclusive,
            (_, KeyboardInteractivity::None) => gtk_layer_shell::KeyboardMode::None,
            (_, KeyboardInteractivity::OnDemand) => gtk_layer_shell::KeyboardMode::OnDemand,
            (_, KeyboardInteractivity::Exclusive) => gtk_layer_shell::KeyboardMode::Exclusive,
        });

        // A surface anchored to opposite edges is sized by the compositor along that axis; the
        // manifest's dimension only applies to the axis it is free on.
        let spans_horizontally = window_spec.anchor.contains(&Anchor::Left)
            && window_spec.anchor.contains(&Anchor::Right);
        let spans_vertically = window_spec.anchor.contains(&Anchor::Top)
            && window_spec.anchor.contains(&Anchor::Bottom);
        window.set_default_size(
            if spans_horizontally {
                -1
            } else {
                window_spec.width as i32
            },
            if spans_vertically {
                -1
            } else {
                window_spec.height as i32
            },
        );

        if config.transparent {
            // An alpha-capable visual, or the surface composites onto black rather than onto the
            // desktop — which is very visible on a bar with a translucent background.
            use gtk::gdk::prelude::ScreenExt as _;
            window.set_app_paintable(true);
            if let Some(screen) = window.screen()
                && let Some(visual) = screen.rgba_visual()
            {
                window.set_visual(Some(&visual));
            }
        }

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&container);

        let resolver = Arc::new(OriginResolver::with_blobs(
            config.index_html.clone(),
            config.asset_root.clone(),
            config.module_root.clone(),
            config.blobs.clone(),
        ));
        let ipc_event = Arc::clone(&callbacks.on_event);
        let navigation_manifest = config.manifest.clone();

        use wry::WebViewBuilderExtUnix as _;
        let webview = wry::WebViewBuilder::new()
            .with_url(origin::INDEX_URL)
            .with_transparent(config.transparent)
            .with_devtools(config.devtools)
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

        window.show_all();

        Ok(Self {
            webview,
            _window: window,
        })
    }

    /// Whether this build and this compositor can host a layer surface.
    pub fn is_available() -> bool {
        gtk::init().is_ok() && gtk_layer_shell::is_supported()
    }
}

impl WebEngine for LayerEngine {
    fn render_path(&self) -> RenderPath {
        // The page owns the whole surface; nothing is composited over it.
        RenderPath::NativeChild
    }

    fn evaluate(&self, script: &str) -> Result<()> {
        self.webview
            .evaluate_script(script)
            .map_err(|e| EngineError::Script(e.to_string()))
    }

    fn set_bounds(&self, _bounds: ViewRect) -> Result<()> {
        // The compositor decides where a layer surface is and how big it is. That is the point of
        // layer-shell, and overriding it here would fight the anchors the manifest declared.
        Ok(())
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

    fn backend_name(&self) -> &'static str {
        "wry (WebKitGTK, wlr-layer-shell surface)"
    }
}
