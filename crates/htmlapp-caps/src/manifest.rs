//! The document format (docs/document-format.md).
//!
//! A manifest lives inside the document itself, in a `<script type="application/htmlapp+json">`
//! block that the host parses *before* the file ever reaches the engine. The block is inert to
//! browsers, so renaming a `.hta` to `.html` still opens in any browser — without native APIs.

use serde::{Deserialize, Serialize};

use crate::error::{CapsError, Result};
use crate::permissions::Permissions;

/// The `<script>` type that carries a manifest. Anything else in the document is page content.
pub const MANIFEST_MIME: &str = "application/htmlapp+json";

/// A parsed `.hta` manifest.
///
/// Every field except `name` is optional: the security model requires that a document with no manifest — or a
/// manifest with no `permissions` block — is a valid, silent, powerless page rather than an error.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Human-readable name, shown in the titlebar, launcher recents, and consent sheet.
    #[serde(default)]
    pub name: Option<String>,

    /// Reverse-DNS application id. Scopes `store`, the consent record, and the built `.desktop`.
    #[serde(default)]
    pub id: Option<String>,

    #[serde(default)]
    pub version: Option<String>,

    /// A `data:` URI or a path relative to the document. Extracted by `htmlapp build`.
    #[serde(default)]
    pub icon: Option<String>,

    #[serde(default)]
    pub window: WindowSpec,

    /// The security model, rule 1: absent means no native APIs at all.
    #[serde(default)]
    pub permissions: Option<Permissions>,

    /// The import map: remote ES modules, pinned by integrity hash and cached forever after first fetch.
    #[serde(default)]
    pub imports: std::collections::BTreeMap<String, ImportSpec>,

    /// The origin and loading rules: `"sibling"` opts into resolving assets next to the source file. Default is
    /// strictly single-file.
    #[serde(default)]
    pub assets: AssetPolicy,

    /// The security model, rule 8: overrides the restrictive default CSP. Declaring this is a deliberate act.
    #[serde(default)]
    pub csp: Option<String>,
}

impl Manifest {
    /// Parse a manifest from the JSON found inside the manifest script block.
    pub fn from_json(src: &str) -> Result<Self> {
        let manifest: Manifest = serde_json::from_str(src)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Reject manifests that parse but cannot be safely enforced.
    pub fn validate(&self) -> Result<()> {
        if let Some(id) = &self.id {
            validate_id(id)?;
        }
        for (name, import) in &self.imports {
            if import.integrity.trim().is_empty() {
                return Err(CapsError::MissingIntegrity { name: name.clone() });
            }
        }
        if let Some(permissions) = &self.permissions {
            permissions.validate()?;
        }
        Ok(())
    }

    /// The name to show in UI, falling back to the id and then to a generic label.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.id.as_deref())
            .unwrap_or("Untitled HTML App")
    }

    /// The security model, rule 1: a document with no `permissions` block gets no native APIs, silently.
    pub fn is_powerless(&self) -> bool {
        self.permissions.as_ref().is_none_or(Permissions::is_empty)
    }
}

/// App ids are used as filesystem path components for the `store` and consent record, so they are
/// validated rather than trusted.
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(CapsError::InvalidId {
            value: id.to_string(),
            reason: "must not be empty",
        });
    }
    if id.starts_with('.') || id.ends_with('.') || id.contains("..") {
        return Err(CapsError::InvalidId {
            value: id.to_string(),
            reason: "must not begin, end, or contain consecutive dots",
        });
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(CapsError::InvalidId {
            value: id.to_string(),
            reason: "may only contain ASCII alphanumerics, '.', '-', and '_'",
        });
    }
    Ok(())
}

/// The origin and loading rules: whether sibling files next to the document are reachable over `htmlapp://app/`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetPolicy {
    /// The document is the whole app. Nothing else on disk is served.
    #[default]
    None,
    /// Files in the document's own directory resolve relative to it.
    Sibling,
}

/// The import map: one entry of the document's import map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImportSpec {
    pub url: String,
    /// Required. The threat model lists the unpinned supply chain as a named threat.
    pub integrity: String,
}

/// The manifest `window` and the window modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WindowSpec {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default)]
    pub min_width: Option<u32>,
    #[serde(default)]
    pub min_height: Option<u32>,
    #[serde(default)]
    pub titlebar: Titlebar,
    #[serde(default)]
    pub background: Background,
    #[serde(default = "default_true")]
    pub resizable: bool,
    #[serde(default)]
    pub mode: WindowMode,

    // --- `mode: "layer"` only (docs/document-format.md) ---
    #[serde(default)]
    pub layer: Layer,
    #[serde(default)]
    pub anchor: Vec<Anchor>,
    #[serde(default)]
    pub exclusive_zone: Option<i32>,
    #[serde(default)]
    pub keyboard_interactivity: KeyboardInteractivity,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub margin: Option<Margin>,
}

impl Default for WindowSpec {
    fn default() -> Self {
        Self {
            title: None,
            width: default_width(),
            height: default_height(),
            min_width: None,
            min_height: None,
            titlebar: Titlebar::default(),
            background: Background::default(),
            resizable: true,
            mode: WindowMode::default(),
            layer: Layer::default(),
            anchor: Vec::new(),
            exclusive_zone: None,
            keyboard_interactivity: KeyboardInteractivity::default(),
            output: None,
            margin: None,
        }
    }
}

fn default_width() -> u32 {
    1024
}
fn default_height() -> u32 {
    768
}
fn default_true() -> bool {
    true
}

/// The window modes: what kind of surface the document becomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    /// `xdg_toplevel`. An ordinary application window.
    #[default]
    Window,
    /// `wlr-layer-shell`. Bars, docks, widgets, overlays, launchers.
    Layer,
    /// `ext-session-lock`. Lock screens.
    Lock,
    /// No surface until clicked. Applets and menu-bar tools.
    Tray,
    /// No surface at all. CLI filters, stdin → stdout (docs/bridge.md).
    Headless,
}

impl WindowMode {
    /// Whether this mode ever puts pixels on screen.
    pub fn has_surface(self) -> bool {
        !matches!(self, WindowMode::Headless)
    }

    /// Whether this mode needs a Wayland protocol beyond `xdg_toplevel`.
    pub fn needs_wayland_shell(self) -> bool {
        matches!(self, WindowMode::Layer | WindowMode::Lock)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WindowMode::Window => "window",
            WindowMode::Layer => "layer",
            WindowMode::Lock => "lock",
            WindowMode::Tray => "tray",
            WindowMode::Headless => "headless",
        }
    }
}

impl std::str::FromStr for WindowMode {
    type Err = CapsError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "window" => Ok(WindowMode::Window),
            "layer" => Ok(WindowMode::Layer),
            "lock" => Ok(WindowMode::Lock),
            "tray" => Ok(WindowMode::Tray),
            "headless" => Ok(WindowMode::Headless),
            other => Err(CapsError::UnknownWindowMode(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Titlebar {
    /// GPUI paints the titlebar, matching the app chrome.
    #[default]
    Native,
    /// No titlebar; the page draws its own and declares drag regions.
    None,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    #[default]
    Opaque,
    /// Requires an alpha-capable surface; used by bars and overlays.
    Transparent,
}

/// wlr-layer-shell layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Background,
    Bottom,
    #[default]
    Top,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyboardInteractivity {
    #[default]
    None,
    OnDemand,
    Exclusive,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Margin {
    #[serde(default)]
    pub top: i32,
    #[serde(default)]
    pub right: i32,
    #[serde(default)]
    pub bottom: i32,
    #[serde(default)]
    pub left: i32,
}
