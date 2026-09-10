//! Punching holes in the page (docs/architecture.md, G2).

use htmlapp_engine::occlusion::merge;
use htmlapp_engine::{ViewRect, WebEngine};

#[test]
fn merge_drops_rectangles_already_covered() {
    let outer = ViewRect::new(0.0, 0.0, 100.0, 100.0);
    let inner = ViewRect::new(10.0, 10.0, 20.0, 20.0);

    // An overlay that covers the whole page subsumes every view inside it; sending both would ask
    // The server to subtract the same area twice on every frame.
    assert_eq!(merge(&[outer, inner]).len(), 1);
    assert_eq!(merge(&[inner, outer]).len(), 1);
}

#[test]
fn merge_keeps_disjoint_rectangles() {
    let left = ViewRect::new(0.0, 0.0, 40.0, 40.0);
    let right = ViewRect::new(60.0, 0.0, 40.0, 40.0);
    assert_eq!(merge(&[left, right]).len(), 2);
}

/// A view can legitimately be laid out at zero height — mid-animation, or inside a collapsed
/// element. That must not become a degenerate rectangle sent to the X server.
#[test]
fn merge_drops_degenerate_rectangles() {
    let rects = [
        ViewRect::new(0.0, 0.0, 0.0, 50.0),
        ViewRect::new(0.0, 0.0, 50.0, 0.0),
        ViewRect::new(0.0, 0.0, -5.0, -5.0),
        ViewRect::new(10.0, 10.0, 20.0, 20.0),
    ];
    assert_eq!(merge(&rects).len(), 1);
}

#[test]
fn merge_of_nothing_is_nothing() {
    assert!(merge(&[]).is_empty());
}

/// An offscreen backend has nothing to punch a hole in — the host already draws over it.
#[test]
fn occlusion_support_is_reported_honestly() {
    // The default trait implementation reports no support, which is correct for a compositing
    // backend and is what makes the runtime fall back to hiding the page.
    struct Composited;
    impl htmlapp_engine::WebEngine for Composited {
        fn render_path(&self) -> htmlapp_engine::RenderPath {
            htmlapp_engine::RenderPath::Shm
        }
        fn evaluate(&self, _: &str) -> htmlapp_engine::Result<()> {
            Ok(())
        }
        fn set_bounds(&self, _: ViewRect) -> htmlapp_engine::Result<()> {
            Ok(())
        }
        fn set_visible(&self, _: bool) -> htmlapp_engine::Result<()> {
            Ok(())
        }
        fn focus(&self) -> htmlapp_engine::Result<()> {
            Ok(())
        }
        fn reload(&self) -> htmlapp_engine::Result<()> {
            Ok(())
        }
        fn backend_name(&self) -> &'static str {
            "test"
        }
    }

    let engine = Composited;
    assert!(!engine.supports_occlusion());
    assert!(!engine.set_occlusions(&[]).unwrap());
    assert!(engine.render_path().composites());
}
