//! The webview as a GPUI element (docs/architecture.md).
//!
//! The WebView ergonomics says the *ergonomics* of `wf-studio`'s `WebView` carry over even though the backend does
//! not: an entity built with `cx.new(...)`, implementing `IntoElement` so it drops into a normal
//! tree via `.child(...)`, with `show()`/`hide()` and ordinary GPUI layout around it.
//!
//! What the WebView ergonomics says goes away is this:
//!
//! ```ignore
//! let mount_webview = has_page && app.modal.is_none();
//! ```
//!
//! That is only truly gone on an offscreen backend. With the native-child backend in use today the
//! page still paints above everything GPUI draws, so [`WebView::hide`] remains the honest way to
//! show a modal over it — which is exactly why it is still on this type. See
//! [`htmlapp_engine`](htmlapp_engine) for the compositing comparison.

use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId,
    IntoElement, LayoutId, Pixels, Size, Style, Window,
};
use htmlapp_engine::{ViewRect, WebEngine};

/// A GPUI entity wrapping a running engine.
pub struct WebView {
    engine: Rc<dyn WebEngine>,
    visible: bool,
    bounds: Bounds<Pixels>,
}

impl WebView {
    pub fn new(engine: Rc<dyn WebEngine>) -> Self {
        Self {
            engine,
            visible: true,
            bounds: Bounds::default(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        let _ = self.engine.set_visible(true);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        let _ = self.engine.set_visible(false);
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    pub fn engine(&self) -> &Rc<dyn WebEngine> {
        &self.engine
    }

    /// The element to place in a GPUI tree.
    pub fn element(&self) -> WebViewElement {
        WebViewElement {
            engine: Rc::clone(&self.engine),
            visible: self.visible,
        }
    }
}

/// The element half of [`WebView`].
pub struct WebViewElement {
    engine: Rc<dyn WebEngine>,
    visible: bool,
}

impl IntoElement for WebViewElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for WebViewElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: Size::full(),
            flex_grow: 1.0,
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Option<Hitbox> {
        if !self.visible {
            let _ = self.engine.set_visible(false);
            return None;
        }

        // The engine is told where it sits in the window on every frame. For a native child
        // surface this moves the X11 window; for an offscreen backend it sizes the buffer.
        let _ = self.engine.set_bounds(ViewRect::new(
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        ));
        let _ = self.engine.set_visible(true);

        Some(window.insert_hitbox(bounds, HitboxBehavior::Normal))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Option<Hitbox>,
        _: &mut Window,
        _: &mut App,
    ) {
        // Nothing to paint: the engine owns these pixels. On an offscreen backend this is where
        // The frame would be drawn into GPUI's scene instead (docs/architecture.md).
    }
}
