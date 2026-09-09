//! The host half of the Tier 3 modules (PRD §9.3, §10).
//!
//! `window`, `menu`, `palette`, and the native views can only be done by the thread that owns the
//! window. Bridge calls arrive on Tokio workers, so they are turned into commands here and drained
//! by the frame loop. Calls that need an answer the UI has not produced yet say so rather than
//! blocking a worker on a round trip through the render loop.

use std::collections::BTreeMap;
use std::sync::Arc;

use htmlapp_api::HostBridge;
use htmlapp_bridge::RpcError;
use parking_lot::Mutex;
use serde_json::{Value, json};

/// A request for the UI thread.
#[derive(Debug, Clone)]
pub enum HostCommand {
    SetTitle(String),
    Resize { width: f32, height: f32 },
    Move { x: f32, y: f32 },
    Fullscreen(bool),
    Minimize,
    Maximize(bool),
    AlwaysOnTop(bool),
    Close,
    /// Open the command palette overlay.
    OpenPalette { query: Option<String> },
    ClosePalette,
    /// Show a message, confirm, or prompt sheet.
    Dialog { kind: String, params: Value },
}

/// The concrete view behind an `<htmlapp-view>` element.
///
/// An enum rather than `Box<dyn NativeView>` because the shared state has to be `Send`, and the
/// trait is not declared that way — the set of view kinds is closed and known, so naming them
/// costs nothing and keeps the bound honest.
pub enum ViewBackend {
    Terminal(htmlapp_views::TerminalView),
    Table(htmlapp_views::TableView),
    /// A kind this build does not implement yet (§14, M8).
    Unimplemented(String),
}

impl ViewBackend {
    fn create(kind: &str) -> Self {
        match htmlapp_views::ViewKind::parse(kind) {
            Some(htmlapp_views::ViewKind::Terminal) => {
                ViewBackend::Terminal(htmlapp_views::TerminalView::default())
            }
            Some(htmlapp_views::ViewKind::Table) => {
                ViewBackend::Table(htmlapp_views::TableView::new())
            }
            _ => ViewBackend::Unimplemented(kind.to_string()),
        }
    }

    fn as_native(&mut self) -> Option<&mut dyn htmlapp_views::NativeView> {
        match self {
            ViewBackend::Terminal(view) => Some(view),
            ViewBackend::Table(view) => Some(view),
            ViewBackend::Unimplemented(_) => None,
        }
    }
}

/// One native view placed by the page (§10).
pub struct ViewState {
    pub kind: String,
    pub options: Value,
    /// Viewport rect reported by the page's `ResizeObserver`, in CSS pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub backend: ViewBackend,
}

/// Shared state the UI thread reads and the bridge writes.
#[derive(Default)]
pub struct HostState {
    pub commands: Mutex<Vec<HostCommand>>,
    pub views: Mutex<BTreeMap<String, ViewState>>,
    pub menu: Mutex<Option<Value>>,
    pub palette_commands: Mutex<Vec<Value>>,
    pub title: Mutex<Option<String>>,
}

impl HostState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Take everything queued for the UI thread.
    pub fn drain_commands(&self) -> Vec<HostCommand> {
        std::mem::take(&mut *self.commands.lock())
    }

    fn push(&self, command: HostCommand) {
        self.commands.lock().push(command);
    }
}

/// Implements the shell-owned modules against [`HostState`].
pub struct RuntimeHost {
    state: Arc<HostState>,
    /// `false` in headless mode, where none of this has anywhere to go.
    has_window: bool,
}

impl RuntimeHost {
    pub fn new(state: Arc<HostState>, has_window: bool) -> Self {
        Self { state, has_window }
    }
}

fn number(params: &Value, key: &str) -> Option<f32> {
    params.get(key).and_then(|v| v.as_f64()).map(|v| v as f32)
}

fn string(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

impl HostBridge for RuntimeHost {
    fn call(&self, module: &str, method: &str, params: Value) -> Result<Value, RpcError> {
        if !self.has_window && module != "view" {
            return Err(RpcError::unsupported(format!(
                "`{module}.{method}` needs a window; this document is running headless"
            )));
        }

        match (module, method) {
            // --- window (§9.3 Tier 3) ---
            ("window", "setTitle") => {
                let title = string(&params, "title")
                    .ok_or_else(|| RpcError::invalid_params("title is required"))?;
                *self.state.title.lock() = Some(title.clone());
                self.state.push(HostCommand::SetTitle(title));
                Ok(Value::Null)
            }
            ("window", "resize") => {
                let (width, height) = (number(&params, "width"), number(&params, "height"));
                match (width, height) {
                    (Some(width), Some(height)) => {
                        self.state.push(HostCommand::Resize { width, height });
                        Ok(Value::Null)
                    }
                    _ => Err(RpcError::invalid_params("width and height are required")),
                }
            }
            ("window", "move") => {
                match (number(&params, "x"), number(&params, "y")) {
                    (Some(x), Some(y)) => {
                        self.state.push(HostCommand::Move { x, y });
                        Ok(Value::Null)
                    }
                    _ => Err(RpcError::invalid_params("x and y are required")),
                }
            }
            ("window", "fullscreen") => {
                let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                self.state.push(HostCommand::Fullscreen(enabled));
                Ok(Value::Null)
            }
            ("window", "minimize") => {
                self.state.push(HostCommand::Minimize);
                Ok(Value::Null)
            }
            ("window", "maximize") => {
                let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                self.state.push(HostCommand::Maximize(enabled));
                Ok(Value::Null)
            }
            ("window", "setAlwaysOnTop") => {
                let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                self.state.push(HostCommand::AlwaysOnTop(enabled));
                Ok(Value::Null)
            }
            ("window", "close") => {
                self.state.push(HostCommand::Close);
                Ok(Value::Null)
            }
            ("window", "setOpacity" | "setInputRegion" | "open") => Err(RpcError::unsupported(
                format!("`window.{method}` is not implemented yet"),
            )),

            // --- menu and palette ---
            ("menu", "setApplicationMenu") => {
                *self.state.menu.lock() = params.get("items").cloned();
                Ok(Value::Null)
            }
            ("palette", "register") => {
                let commands = params
                    .get("commands")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                *self.state.palette_commands.lock() = commands;
                Ok(Value::Null)
            }
            ("palette", "open") => {
                self.state.push(HostCommand::OpenPalette {
                    query: string(&params, "query"),
                });
                Ok(Value::Null)
            }
            ("palette", "close") => {
                self.state.push(HostCommand::ClosePalette);
                Ok(Value::Null)
            }

            // --- dialogs the shell paints (§9.3 Tier 1) ---
            ("dialog", kind @ ("message" | "confirm" | "prompt")) => {
                self.state.push(HostCommand::Dialog {
                    kind: kind.to_string(),
                    params,
                });
                // These resolve when the sheet closes. Until the sheet is wired to a reply
                // channel, saying so is more honest than resolving with a made-up answer.
                Err(RpcError::unsupported(format!(
                    "`dialog.{kind}` is queued but does not return a result yet"
                )))
            }

            // --- native views (§10) ---
            ("view", "create") => {
                let id = string(&params, "id")
                    .ok_or_else(|| RpcError::invalid_params("id is required"))?;
                let kind = string(&params, "kind").unwrap_or_else(|| "terminal".into());

                if htmlapp_views::ViewKind::parse(&kind).is_none_or(|k| !k.is_implemented()) {
                    return Err(RpcError::unsupported(format!(
                        "`{kind}` views are not implemented in this build"
                    )));
                }

                self.state.views.lock().insert(
                    id,
                    ViewState {
                        backend: ViewBackend::create(&kind),
                        kind,
                        options: params.get("options").cloned().unwrap_or(Value::Null),
                        x: 0.0,
                        y: 0.0,
                        width: 0.0,
                        height: 0.0,
                        scale: 1.0,
                    },
                );
                Ok(Value::Null)
            }
            ("view", "layout") => {
                let id = string(&params, "id")
                    .ok_or_else(|| RpcError::invalid_params("id is required"))?;
                let mut views = self.state.views.lock();
                let Some(view) = views.get_mut(&id) else {
                    return Err(RpcError::invalid_params(format!("no such view: {id}")));
                };
                view.x = number(&params, "x").unwrap_or(0.0);
                view.y = number(&params, "y").unwrap_or(0.0);
                view.width = number(&params, "width").unwrap_or(0.0);
                view.height = number(&params, "height").unwrap_or(0.0);
                view.scale = number(&params, "scale").unwrap_or(1.0);

                // The view converts pixels into whatever unit it works in — cells, for a terminal.
                if let Some(native) = view.backend.as_native() {
                    native.resize(view.width, view.height);
                }
                Ok(Value::Null)
            }
            ("view", "write") => {
                let id = string(&params, "id")
                    .ok_or_else(|| RpcError::invalid_params("id is required"))?;
                let data = string(&params, "data").unwrap_or_default();
                let mut views = self.state.views.lock();
                let Some(view) = views.get_mut(&id) else {
                    return Err(RpcError::invalid_params(format!("no such view: {id}")));
                };
                if let Some(native) = view.backend.as_native() {
                    native.write(&data);
                }
                Ok(Value::Null)
            }
            ("view", "destroy") => {
                let id = string(&params, "id")
                    .ok_or_else(|| RpcError::invalid_params("id is required"))?;
                self.state.views.lock().remove(&id);
                Ok(Value::Null)
            }
            ("view", "set") => {
                let id = string(&params, "id")
                    .ok_or_else(|| RpcError::invalid_params("id is required"))?;
                let mut views = self.state.views.lock();
                let Some(view) = views.get_mut(&id) else {
                    return Err(RpcError::invalid_params(format!("no such view: {id}")));
                };
                if let Some(native) = view.backend.as_native() {
                    native.set(params.get("props").unwrap_or(&Value::Null));
                }
                Ok(Value::Null)
            }
            ("view", "call") => {
                let id = string(&params, "id")
                    .ok_or_else(|| RpcError::invalid_params("id is required"))?;
                let method = string(&params, "method")
                    .ok_or_else(|| RpcError::invalid_params("method is required"))?;
                let mut views = self.state.views.lock();
                let Some(view) = views.get_mut(&id) else {
                    return Err(RpcError::invalid_params(format!("no such view: {id}")));
                };
                let Some(native) = view.backend.as_native() else {
                    return Err(RpcError::unsupported("this view kind is not implemented"));
                };
                native
                    .call(&method, params.get("params").cloned().unwrap_or(Value::Null))
                    .map_err(RpcError::invalid_params)
            }

            // --- layer-shell (§8.3) ---
            //
            // Enumerating outputs works whether or not layer-shell does: knowing which monitors
            // exist is useful to any document, not just a bar.
            ("layer", "listOutputs") => Ok(serde_json::to_value(htmlapp_wayland::outputs())
                .unwrap_or_else(|_| json!([]))),
            ("layer", _) => Err(RpcError::unsupported(
                "layer-shell control needs a Wayland session and the layer window mode",
            )),

            _ => Err(RpcError::not_found(&format!("{module}.{method}"))),
        }
    }
}

/// The host used in headless mode: everything that needs a window says so plainly.
pub struct HeadlessHost;

impl HostBridge for HeadlessHost {
    fn call(&self, module: &str, method: &str, _params: Value) -> Result<Value, RpcError> {
        Err(RpcError::unsupported(format!(
            "`{module}.{method}` needs a window; this document is running headless"
        )))
    }
}
