//! Manifest-to-protocol translation for layer-shell (docs/document-format.md).

use htmlapp_caps::Manifest;
use htmlapp_wayland::{
    ANCHOR_BOTTOM, ANCHOR_LEFT, ANCHOR_RIGHT, ANCHOR_TOP, Capabilities, LayerConfig, outputs,
};

fn config(json: &str) -> LayerConfig {
    let manifest = Manifest::from_json(json).expect("manifest must parse");
    LayerConfig::from_window(&manifest.window)
}

/// The window-mode example must translate to the values wlr-layer-shell expects.
#[test]
fn prd_example_translates_correctly() {
    let layer = config(
        r#"{ "window": { "mode": "layer",
                         "layer": "top",
                         "anchor": ["top", "left", "right"],
                         "exclusive_zone": 34,
                         "keyboard_interactivity": "on-demand",
                         "output": "primary" } }"#,
    );

    assert_eq!(layer.layer, 2, "`top` is layer 2");
    assert_eq!(layer.anchor, ANCHOR_TOP | ANCHOR_LEFT | ANCHOR_RIGHT);
    assert_eq!(layer.exclusive_zone, 34);
    assert_eq!(layer.keyboard_interactivity, 2, "`on-demand` is 2, not 1");
    assert_eq!(layer.output.as_deref(), Some("primary"));

    assert!(layer.spans_horizontally(), "a top bar spans left to right");
    assert!(!layer.spans_vertically());
    assert!(layer.warnings().is_empty(), "{:?}", layer.warnings());
}

#[test]
fn every_layer_maps_to_its_protocol_value() {
    for (name, value) in [("background", 0), ("bottom", 1), ("top", 2), ("overlay", 3)] {
        let layer = config(&format!(r#"{{"window":{{"layer":"{name}"}}}}"#));
        assert_eq!(layer.layer, value, "layer {name}");
    }
}

#[test]
fn keyboard_interactivity_uses_the_protocol_numbering() {
    // The protocol orders these none=0, exclusive=1, on-demand=2 — not alphabetically, and not in
    // The order they appear in the manifest documentation.
    for (name, value) in [("none", 0), ("exclusive", 1), ("on-demand", 2)] {
        let layer = config(&format!(
            r#"{{"window":{{"keyboard_interactivity":"{name}"}}}}"#
        ));
        assert_eq!(layer.keyboard_interactivity, value, "{name}");
    }
}

#[test]
fn anchors_combine_into_a_bitmask() {
    assert_eq!(config(r#"{"window":{"anchor":[]}}"#).anchor, 0);
    assert_eq!(config(r#"{"window":{"anchor":["bottom"]}}"#).anchor, ANCHOR_BOTTOM);

    let all = config(r#"{"window":{"anchor":["top","bottom","left","right"]}}"#);
    assert_eq!(all.anchor, ANCHOR_TOP | ANCHOR_BOTTOM | ANCHOR_LEFT | ANCHOR_RIGHT);
    assert!(all.spans_horizontally() && all.spans_vertically());
}

#[test]
fn margins_default_to_zero_and_are_carried_through() {
    assert_eq!(config(r#"{"window":{}}"#).margin, (0, 0, 0, 0));
    let margined = config(r#"{"window":{"margin":{"top":8,"left":4}}}"#);
    assert_eq!(margined.margin, (8, 0, 0, 4));
}

// --- configurations worth warning about before anything is drawn ---

#[test]
fn exclusive_zone_without_an_anchor_is_flagged() {
    let layer = config(r#"{"window":{"exclusive_zone":40}}"#);
    assert!(
        layer.warnings().iter().any(|w| w.contains("anchor")),
        "{:?}",
        layer.warnings()
    );
}

#[test]
fn fullscreen_surface_reserving_space_is_flagged() {
    let layer = config(
        r#"{"window":{"anchor":["top","bottom","left","right"],"exclusive_zone":40}}"#,
    );
    assert!(!layer.warnings().is_empty());
}

#[test]
fn exclusive_keyboard_on_a_low_layer_is_flagged() {
    let layer = config(
        r#"{"window":{"layer":"background","keyboard_interactivity":"exclusive"}}"#,
    );
    assert!(
        layer.warnings().iter().any(|w| w.contains("focus")),
        "{:?}",
        layer.warnings()
    );
}

// --- talking to the compositor ---

/// Must not panic or hang whether or not a compositor is there — this runs on every launch to fill
/// in the diagnostics strip.
#[test]
fn capability_detection_is_safe_without_a_compositor() {
    let capabilities = Capabilities::detect();
    if !capabilities.wayland {
        assert!(!capabilities.layer_shell && !capabilities.session_lock);
    }
}

#[test]
fn output_enumeration_is_safe_without_a_compositor() {
    let found = outputs();
    // Under a compositor there is at least one output, and the first is marked primary.
    if let Some(first) = found.first() {
        assert!(first.primary, "the first output should be primary");
        assert!(first.scale >= 1);
        assert!(found.iter().filter(|o| o.primary).count() == 1);
    }
}
