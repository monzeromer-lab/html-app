//! Punching holes in the page so the host can paint over it (PRD §4.2 G2).
//!
//! # The problem this solves
//!
//! §6.1 lists four consequences of layering a native child surface over the host window instead of
//! compositing offscreen. The first two are the ones authors actually feel:
//!
//! > The webview paints over everything GPUI draws, so any modal or popover forces the webview to
//! > be **removed from the element tree entirely** while the dialog is open.
//!
//! and the native views of §10, which are supposed to sit *above* the page, end up beneath it.
//!
//! # The mechanism
//!
//! `wry` puts the page in an X11 child window of the host window. X11's SHAPE extension can set a
//! window's *bounding* region — the part of it the server considers to exist — and its *input*
//! region. Subtracting a rectangle from both makes that area of the child window neither visible
//! nor clickable, so the parent shows through and receives the input.
//!
//! The parent is the GPUI window. So GPUI paints a modal, the runtime reports that modal's rect as
//! an occlusion, and the modal appears over the page with input routed to it — without hiding the
//! webview, and without a `mount_webview` flag.
//!
//! # What it does not do
//!
//! SHAPE regions are sets of rectangles. There is no per-pixel alpha and no anti-aliasing, so a
//! rounded corner is a staircase unless it is approximated with enough rectangles, and a drop
//! shadow that fades over the page cannot be expressed at all. Those need the offscreen path in
//! §6.3. What this does give is the load-bearing part: **native UI over the page, with input**.

use x11rb::connection::Connection;
use x11rb::protocol::shape::{self, ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{self, ConnectionExt as _, Rectangle};
use x11rb::rust_connection::RustConnection;

use crate::ViewRect;

/// Shapes the page's X11 child window so host-drawn regions show through.
pub struct Occluder {
    connection: RustConnection,
    /// The child windows `wry` created inside the host window. Normally exactly one.
    children: Vec<xproto::Window>,
    /// The last set applied, so an unchanged frame costs no X traffic.
    applied: Vec<Rectangle>,
    size: (u16, u16),
}

impl Occluder {
    /// Find the page's child window inside `parent` and prepare to shape it.
    ///
    /// Returns `None` when the server has no SHAPE extension, or when the child window is not
    /// there yet — the caller retries on a later frame rather than failing the document.
    pub fn new(parent: u32) -> Option<Self> {
        let (connection, _) = x11rb::connect(None).ok()?;

        // Without SHAPE there is nothing to do, and silently doing nothing would be worse than
        // saying so: the runtime falls back to hiding the page for modals.
        let present = connection
            .shape_query_version()
            .ok()?
            .reply()
            .ok()
            .is_some();
        if !present {
            tracing::warn!("the X server has no SHAPE extension; the page cannot be occluded");
            return None;
        }

        let children = connection.query_tree(parent).ok()?.reply().ok()?.children;
        if children.is_empty() {
            return None;
        }

        tracing::debug!(?children, "occluder attached to the page's child windows");
        Some(Self {
            connection,
            children,
            applied: Vec::new(),
            size: (0, 0),
        })
    }

    /// Set the regions of the page that should be punched through, in physical pixels relative to
    /// the child window's own origin.
    ///
    /// `size` is the child window's full extent; the bounding region is reset to it each time
    /// before the holes are subtracted, so clearing an occlusion restores the page.
    pub fn set_occlusions(&mut self, size: (u16, u16), holes: &[ViewRect]) {
        let rectangles: Vec<Rectangle> = holes
            .iter()
            .filter_map(|hole| {
                // A degenerate rectangle is not an error — a view can legitimately be laid out at
                // zero height — but it must not be sent to the server.
                let width = hole.width.round().max(0.0) as u16;
                let height = hole.height.round().max(0.0) as u16;
                (width > 0 && height > 0).then_some(Rectangle {
                    x: hole.x.round() as i16,
                    y: hole.y.round() as i16,
                    width,
                    height,
                })
            })
            .collect();

        // `xproto::Rectangle` is not `PartialEq`, so the comparison is done field-wise.
        let unchanged = size == self.size
            && rectangles.len() == self.applied.len()
            && rectangles.iter().zip(&self.applied).all(|(a, b)| {
                a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height
            });
        if unchanged {
            return;
        }
        self.applied = rectangles.clone();
        self.size = size;

        let full = [Rectangle {
            x: 0,
            y: 0,
            width: size.0.max(1),
            height: size.1.max(1),
        }];

        for &child in &self.children {
            // BOUNDING controls what is drawn; INPUT controls what is clickable. Both have to be
            // shaped, or the page would either still cover the modal or still swallow its clicks.
            for kind in [SK::BOUNDING, SK::INPUT] {
                if self
                    .connection
                    .shape_rectangles(
                        SO::SET,
                        kind,
                        xproto::ClipOrdering::UNSORTED,
                        child,
                        0,
                        0,
                        &full,
                    )
                    .is_err()
                {
                    continue;
                }
                if !rectangles.is_empty() {
                    let _ = self.connection.shape_rectangles(
                        SO::SUBTRACT,
                        kind,
                        xproto::ClipOrdering::UNSORTED,
                        child,
                        0,
                        0,
                        &rectangles,
                    );
                }
            }
        }

        let _ = self.connection.flush();
    }

    /// Restore the page to its full extent.
    pub fn clear(&mut self, size: (u16, u16)) {
        self.set_occlusions(size, &[]);
    }

    /// Whether anything is currently punched through.
    pub fn is_occluding(&self) -> bool {
        !self.applied.is_empty()
    }

    /// Re-read the child list, after the engine has been rebuilt.
    pub fn refresh(&mut self, parent: u32) {
        let Ok(cookie) = self.connection.query_tree(parent) else {
            return;
        };
        let Ok(reply) = cookie.reply() else {
            return;
        };
        self.children = reply.children;
        self.applied.clear();
    }
}

/// Whether the running X server supports the SHAPE extension.
pub fn is_supported() -> bool {
    let Ok((connection, _)) = x11rb::connect(None) else {
        return false;
    };
    connection
        .shape_query_version()
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some()
}

/// Merge overlapping rectangles so the server is not asked to subtract the same area twice.
///
/// Not required for correctness — SUBTRACT is idempotent — but a page with a dozen native views
/// would otherwise re-send a dozen near-identical regions on every frame.
pub fn merge(rects: &[ViewRect]) -> Vec<ViewRect> {
    let mut merged: Vec<ViewRect> = Vec::with_capacity(rects.len());
    for rect in rects {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            continue;
        }
        if let Some(existing) = merged.iter_mut().find(|existing| contains(existing, rect)) {
            let _ = existing;
            continue;
        }
        merged.retain(|existing| !contains(rect, existing));
        merged.push(*rect);
    }
    merged
}

fn contains(outer: &ViewRect, inner: &ViewRect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.x + outer.width >= inner.x + inner.width
        && outer.y + outer.height >= inner.y + inner.height
}

// Silences an unused-import warning when only part of the shape API is referenced.
#[allow(unused_imports)]
use shape as _shape;
