//! Native views placed inline in HTML layout (docs/bridge.md).
//!
//! # What this is for
//!
//! Because the page is a texture in GPUI's scene, GPUI can draw on top of it — which enables
//! something no other web-based desktop runtime offers: real native widgets participating in
//! ordinary HTML layout.
//!
//! ```html
//! <htmlapp-view kind="terminal" id="term" style="flex:1"></htmlapp-view>
//! <htmlapp-view kind="table"    id="rows" style="height:400px"></htmlapp-view>
//! ```
//!
//! The injected shim registers the custom element, a `ResizeObserver` reports its viewport rect
//! over the bridge, and the host renders a GPUI element at that rect, above the page.
//!
//! # The catch, stated plainly
//!
//! "Above the page" is exactly the part the current engine backend cannot do. wry gives the page a
//! *native child surface*, which paints above everything GPUI draws into the same window — so a
//! view rendered here would be underneath the page rather than on top of it. That is the
//! limitation the rendering design exists to remove, and it is removed by the offscreen backend in the render pipeline, not here.
//!
//! So the views in this crate are complete, tested GPUI elements that the runtime positions and
//! keeps in sync; what they are waiting on is a backend that lets them be seen. Nothing about them
//! changes when that lands.

#![forbid(unsafe_code)]

pub mod table;
pub mod terminal;

use serde_json::Value;

pub use table::TableView;
pub use terminal::TerminalView;

/// The `kind` attribute of an `<htmlapp-view>` element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// `alacritty_terminal` — a real PTY, real escape handling, 60fps scrollback.
    Terminal,
    /// A GPUI virtualized table — millions of rows without a DOM.
    Table,
    /// GPUI editor plus tree-sitter. Not implemented yet (the roadmap, M8).
    Editor,
    /// GStreamer / dmabuf. Not implemented yet (the roadmap, M8).
    Video,
    /// A direct Vulkan surface. Not implemented yet (the roadmap, M8).
    Canvas3d,
}

impl ViewKind {
    pub fn parse(kind: &str) -> Option<Self> {
        Some(match kind {
            "terminal" => ViewKind::Terminal,
            "table" => ViewKind::Table,
            "editor" => ViewKind::Editor,
            "video" => ViewKind::Video,
            "canvas3d" => ViewKind::Canvas3d,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ViewKind::Terminal => "terminal",
            ViewKind::Table => "table",
            ViewKind::Editor => "editor",
            ViewKind::Video => "video",
            ViewKind::Canvas3d => "canvas3d",
        }
    }

    /// Whether this build can actually render this kind. The roadmap ships `terminal` and `table` in M5;
    /// The rest are M8.
    pub fn is_implemented(self) -> bool {
        matches!(self, ViewKind::Terminal | ViewKind::Table)
    }
}

/// A view the host renders at a rect the page reported.
pub trait NativeView {
    fn kind(&self) -> ViewKind;

    /// Bytes the page wrote with `htmlapp.view(id).write(...)`.
    fn write(&mut self, data: &str);

    /// A structured call from `htmlapp.view(id).call(method, params)`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;

    /// Properties from `htmlapp.view(id).set(props)`.
    fn set(&mut self, props: &Value);

    /// Resize in character cells or rows, as the view sees fit.
    fn resize(&mut self, width: f32, height: f32);
}
