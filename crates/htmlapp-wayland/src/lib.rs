//! Layer-shell, session-lock, and output management (PRD §8.3, §9.3 Tier 3).
//!
//! # Status
//!
//! The manifest's window modes translate cleanly onto Wayland protocols, and that translation lives
//! here and is tested. Actually *creating* a layer surface does not, yet, and the reason is worth
//! being precise about rather than filing under "not done":
//!
//! A `zwlr_layer_surface_v1` is a surface the compositor positions and sizes; the client still has
//! to render into it. GPUI owns rendering, and `gpui` 0.2.2 exposes no way to create a window from
//! a foreign Wayland surface or to target one of these. Until it does, a bar has a shell but
//! nothing to draw with — so the runtime refuses `mode: "layer"` with an explanation instead of
//! opening something that cannot paint.
//!
//! What *does* work here, today, and is used by the runtime:
//!
//! - [`Capabilities::detect`] — whether this compositor offers layer-shell and session-lock at all,
//!   which is what the launcher's diagnostics strip and `htmlapp inspect` report.
//! - [`outputs`] — enumerating monitors, backing `layer.listOutputs()` in §9.3 Tier 3.
//! - [`LayerConfig`] — the manifest-to-protocol translation, including the anchor bitmask.

#![forbid(unsafe_code)]

use htmlapp_caps::{Anchor, KeyboardInteractivity, Layer, WindowSpec};
use serde::{Deserialize, Serialize};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::globals::GlobalListContents;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop, globals};

// --- anchor bitmask, as defined by zwlr_layer_surface_v1 ---

pub const ANCHOR_TOP: u32 = 1;
pub const ANCHOR_BOTTOM: u32 = 2;
pub const ANCHOR_LEFT: u32 = 4;
pub const ANCHOR_RIGHT: u32 = 8;

/// A layer-shell surface configuration, derived from the manifest (§8.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerConfig {
    /// `zwlr_layer_shell_v1` layer enum value.
    pub layer: u32,
    /// The anchor bitmask.
    pub anchor: u32,
    /// Space to reserve so other windows do not cover this surface.
    ///
    /// `-1` means "do not reserve, and do not be covered"; `0` means "do not reserve". The manifest
    /// leaving it unset means a bar that overlaps rather than one that pushes windows aside, which
    /// is the less surprising default for something that might be transparent.
    pub exclusive_zone: i32,
    pub keyboard_interactivity: u32,
    pub margin: (i32, i32, i32, i32),
    /// The output name, or `None` for the compositor's choice.
    pub output: Option<String>,
    pub width: u32,
    pub height: u32,
}

impl LayerConfig {
    /// Translate a manifest `window` block into protocol values.
    pub fn from_window(window: &WindowSpec) -> Self {
        let anchor = window.anchor.iter().fold(0, |bits, anchor| {
            bits | match anchor {
                Anchor::Top => ANCHOR_TOP,
                Anchor::Bottom => ANCHOR_BOTTOM,
                Anchor::Left => ANCHOR_LEFT,
                Anchor::Right => ANCHOR_RIGHT,
            }
        });

        let margin = window
            .margin
            .map(|m| (m.top, m.right, m.bottom, m.left))
            .unwrap_or((0, 0, 0, 0));

        Self {
            layer: match window.layer {
                Layer::Background => 0,
                Layer::Bottom => 1,
                Layer::Top => 2,
                Layer::Overlay => 3,
            },
            anchor,
            exclusive_zone: window.exclusive_zone.unwrap_or(0),
            keyboard_interactivity: match window.keyboard_interactivity {
                KeyboardInteractivity::None => 0,
                KeyboardInteractivity::Exclusive => 1,
                KeyboardInteractivity::OnDemand => 2,
            },
            margin,
            output: window.output.clone(),
            width: window.width,
            height: window.height,
        }
    }

    /// Whether the surface spans the full width of its output.
    pub fn spans_horizontally(&self) -> bool {
        self.anchor & ANCHOR_LEFT != 0 && self.anchor & ANCHOR_RIGHT != 0
    }

    /// Whether the surface spans the full height of its output.
    pub fn spans_vertically(&self) -> bool {
        self.anchor & ANCHOR_TOP != 0 && self.anchor & ANCHOR_BOTTOM != 0
    }

    /// Configuration mistakes worth telling the author about before anything is drawn.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        if self.anchor == 0 && self.exclusive_zone > 0 {
            warnings.push(
                "exclusive_zone has no effect without an anchor: the compositor cannot know which \
                 edge to reserve space along"
                    .into(),
            );
        }
        if self.spans_horizontally() && self.spans_vertically() && self.exclusive_zone > 0 {
            warnings.push(
                "a surface anchored to all four edges covers the whole output; reserving an \
                 exclusive zone as well leaves no room for anything else"
                    .into(),
            );
        }
        if self.keyboard_interactivity == 1 && self.layer < 2 {
            warnings.push(
                "exclusive keyboard interactivity on a background or bottom layer will take focus \
                 from windows drawn above it"
                    .into(),
            );
        }
        warnings
    }
}

/// One monitor, as reported to `layer.listOutputs()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub description: Option<String>,
    pub width: i32,
    pub height: i32,
    pub scale: i32,
    pub primary: bool,
}

/// What the running compositor actually supports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub wayland: bool,
    pub layer_shell: bool,
    pub session_lock: bool,
}

impl Capabilities {
    /// Ask the compositor what it offers, by listing its globals.
    ///
    /// Reported on the launcher's diagnostics strip, and used to explain *why* a `mode: "layer"`
    /// document will not run — "your compositor has no layer-shell" and "this build cannot render
    /// into one" are different problems with different fixes.
    pub fn detect() -> Self {
        let Ok(connection) = Connection::connect_to_env() else {
            return Self::default();
        };
        let Ok((globals, _queue)) = globals::registry_queue_init::<NoopState>(&connection) else {
            return Self {
                wayland: true,
                ..Default::default()
            };
        };

        let mut capabilities = Self {
            wayland: true,
            ..Default::default()
        };
        for global in globals.contents().clone_list() {
            match global.interface.as_str() {
                "zwlr_layer_shell_v1" => capabilities.layer_shell = true,
                "ext_session_lock_manager_v1" => capabilities.session_lock = true,
                _ => {}
            }
        }
        capabilities
    }
}

/// Enumerate the compositor's outputs.
///
/// Backs `layer.listOutputs()` (§9.3 Tier 3) and works whether or not layer-shell is available —
/// knowing which monitors exist is useful to any document, not just a bar.
pub fn outputs() -> Vec<OutputInfo> {
    let Ok(connection) = Connection::connect_to_env() else {
        return Vec::new();
    };
    let Ok((globals, mut queue)) = globals::registry_queue_init::<OutputState>(&connection) else {
        return Vec::new();
    };
    let handle = queue.handle();

    let mut state = OutputState::default();
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            // Version 2 is enough for geometry, mode, and scale, and is supported everywhere.
            let version = global.version.min(2);
            let _ = globals
                .registry()
                .bind::<wl_output::WlOutput, _, _>(global.name, version, &handle, global.name);
        }
    }

    // Two round trips: one to bind, one to receive the events the binding produced.
    let _ = queue.roundtrip(&mut state);
    let _ = queue.roundtrip(&mut state);

    let mut outputs = state.outputs;
    // The compositor announces outputs in its own order; the first is treated as primary, which is
    // also what `"output": "primary"` in a manifest resolves to.
    if let Some(first) = outputs.first_mut() {
        first.primary = true;
    }
    outputs
}

#[derive(Default)]
struct OutputState {
    outputs: Vec<OutputInfo>,
}

#[derive(Default)]
struct NoopState;

impl Dispatch<wl_output::WlOutput, u32> for OutputState {
    fn event(
        state: &mut Self,
        _output: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &u32,
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
        let entry = match state.outputs.iter_mut().find(|o| o.name == id.to_string()) {
            Some(entry) => entry,
            None => {
                state.outputs.push(OutputInfo {
                    // Replaced by the compositor's own name as soon as it sends one.
                    name: id.to_string(),
                    description: None,
                    width: 0,
                    height: 0,
                    scale: 1,
                    primary: false,
                });
                state.outputs.last_mut().expect("just pushed")
            }
        };

        match event {
            wl_output::Event::Geometry { make, model, .. } => {
                entry.description = Some(format!("{make} {model}").trim().to_string());
            }
            wl_output::Event::Mode { width, height, .. } => {
                entry.width = width;
                entry.height = height;
            }
            wl_output::Event::Scale { factor } => {
                entry.scale = factor;
            }
            wl_output::Event::Name { name } => {
                entry.name = name;
            }
            wl_output::Event::Description { description } => {
                entry.description = Some(description);
            }
            _ => {}
        }
    }
}

delegate_noop!(NoopState: ignore wl_output::WlOutput);

// `registry_queue_init` collects the global list itself, so neither state needs to react to
// registry events — but the queue still requires a handler to be registered for them.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for OutputState {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _globals: &GlobalListContents,
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for NoopState {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _globals: &GlobalListContents,
        _connection: &Connection,
        _handle: &QueueHandle<Self>,
    ) {
    }
}
