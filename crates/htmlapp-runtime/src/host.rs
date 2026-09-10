//! The host half of the Tier 3 modules (docs/api-reference.md and docs/bridge.md).
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

/// Something the shell is drawing over the page (docs/architecture.md, G2).
///
/// While one of these is up the page is occluded entirely, so the overlay is genuinely on top and
/// receives input — rather than the page being hidden, which is what the WebView ergonomics says goes away.
#[derive(Debug, Clone)]
pub enum Overlay {
    /// The native command palette (the API catalog `palette`).
    Palette { query: String, selected: usize },
    /// A context menu (the API catalog `menu.popup`).
    Menu {
        items: Vec<MenuEntry>,
        x: f32,
        y: f32,
        selected: usize,
    },
    /// `dialog.message` / `confirm` / `prompt`.
    Dialog {
        kind: DialogKind,
        title: String,
        body: String,
        input: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    Message,
    Confirm,
    Prompt,
}

/// One flattened menu entry, ready to draw.
#[derive(Debug, Clone)]
pub struct MenuEntry {
    pub id: String,
    pub label: String,
    pub separator: bool,
    pub enabled: bool,
    pub checked: Option<bool>,
    /// Nesting depth, so a submenu renders indented rather than needing a second popup.
    pub depth: usize,
}

/// Flatten the manifest-shaped menu JSON into drawable entries.
pub fn flatten_menu(items: &[Value], depth: usize, out: &mut Vec<MenuEntry>) {
    for item in items {
        let separator = item.get("separator").and_then(|v| v.as_bool()).unwrap_or(false);
        if separator {
            out.push(MenuEntry {
                id: String::new(),
                label: String::new(),
                separator: true,
                enabled: false,
                checked: None,
                depth,
            });
            continue;
        }

        out.push(MenuEntry {
            id: item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            label: item.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            separator: false,
            enabled: item.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
            checked: item.get("checked").and_then(|v| v.as_bool()),
            depth,
        });

        if let Some(submenu) = item.get("submenu").and_then(|v| v.as_array()) {
            flatten_menu(submenu, depth + 1, out);
        }
    }
}

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
    SetOpacity(f32),
    SetInputRegion(Option<Vec<htmlapp_engine::ViewRect>>),
    OpenWindow { path: Option<String> },
    /// Begin dragging files out of this window (the API catalog `dnd.startDrag`).
    StartDrag { paths: Vec<std::path::PathBuf> },
}

/// The concrete view behind an `<htmlapp-view>` element.
///
/// An enum rather than `Box<dyn NativeView>` because the shared state has to be `Send`, and the
/// trait is not declared that way — the set of view kinds is closed and known, so naming them
/// costs nothing and keeps the bound honest.
pub enum ViewBackend {
    Terminal(htmlapp_views::TerminalView),
    Table(htmlapp_views::TableView),
    /// A kind this build does not implement yet (the roadmap, M8).
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

    /// The GPUI element for this view, if the kind is one this build renders.
    pub fn element(&self, height: f32) -> Option<gpui::AnyElement> {
        match self {
            ViewBackend::Terminal(view) => Some(view.element()),
            ViewBackend::Table(view) => Some(view.element(height)),
            ViewBackend::Unimplemented(_) => None,
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

/// One native view placed by the page (docs/bridge.md).
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
    /// A full-page region the host is painting over — a menu, palette, or dialog sheet. When set,
    /// The page is occluded entirely rather than per-view (docs/architecture.md, G2).
    pub overlay: Mutex<Option<htmlapp_engine::ViewRect>>,
    /// What that overlay is showing.
    pub overlay_state: Mutex<Option<Overlay>>,
    /// Where to send the overlay's outcome, for the calls that have one.
    pub overlay_reply: Mutex<Option<tokio::sync::oneshot::Sender<Value>>>,
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

    /// Put an overlay up and, if it has an outcome, hand back the receiver for it.
    fn show_overlay(&self, overlay: Overlay) -> tokio::sync::oneshot::Receiver<Value> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        // Replacing an overlay drops the previous reply channel, which resolves the earlier call
        // with a cancellation rather than leaving it hanging forever.
        *self.overlay_state.lock() = Some(overlay);
        *self.overlay_reply.lock() = Some(tx);
        rx
    }

    /// Dismiss the overlay, answering whatever was waiting on it.
    pub fn close_overlay(&self, outcome: Value) {
        *self.overlay_state.lock() = None;
        *self.overlay.lock() = None;
        if let Some(reply) = self.overlay_reply.lock().take() {
            let _ = reply.send(outcome);
        }
    }

    /// Whether an overlay is currently up.
    pub fn has_overlay(&self) -> bool {
        self.overlay_state.lock().is_some()
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
    /// Set once the API modules are registered, so `dnd.startDrag` can scope the files it is asked
    /// to drag out. `None` until then, which is before any page script has run.
    ctx: parking_lot::RwLock<Option<htmlapp_api::Ctx>>,
}

impl RuntimeHost {
    pub fn new(state: Arc<HostState>, has_window: bool) -> Self {
        Self {
            state,
            has_window,
            ctx: parking_lot::RwLock::new(None),
        }
    }

    /// Hand over the enforcement context once the modules exist.
    pub fn set_context(&self, ctx: htmlapp_api::Ctx) {
        *self.ctx.write() = Some(ctx);
    }

    fn check_read(&self, path: &str) -> Result<std::path::PathBuf, RpcError> {
        let ctx = self.ctx.read();
        let ctx = ctx
            .as_ref()
            .ok_or_else(|| RpcError::internal("the host has no enforcement context yet"))?;
        ctx.check_read(path)
    }
}

fn number(params: &Value, key: &str) -> Option<f32> {
    params.get(key).and_then(|v| v.as_f64()).map(|v| v as f32)
}

fn string(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

impl HostBridge for RuntimeHost {
    fn call<'a>(
        &'a self,
        module: &'a str,
        method: &'a str,
        params: Value,
    ) -> htmlapp_bridge::BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move { self.dispatch(module, method, params).await })
    }
}

impl RuntimeHost {
    async fn dispatch(
        &self,
        module: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if !self.has_window && module != "view" {
            return Err(RpcError::unsupported(format!(
                "`{module}.{method}` needs a window; this document is running headless"
            )));
        }

        match (module, method) {
            // --- window (docs/api-reference.md, Tier 3) ---
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
            ("window", "setOpacity") => {
                let opacity = number(&params, "opacity").unwrap_or(1.0).clamp(0.0, 1.0);
                self.state.push(HostCommand::SetOpacity(opacity));
                Ok(Value::Null)
            }
            ("window", "setInputRegion") => {
                // `null` restores the whole window; a list of rects makes everything else
                // click-through, which is what an overlay bar wants.
                let rects = params.get("rects").and_then(|v| v.as_array()).map(|rects| {
                    rects
                        .iter()
                        .filter_map(|rect| {
                            Some(htmlapp_engine::ViewRect::new(
                                number(rect, "x")?,
                                number(rect, "y")?,
                                number(rect, "width")?,
                                number(rect, "height")?,
                            ))
                        })
                        .collect::<Vec<_>>()
                });
                self.state.push(HostCommand::SetInputRegion(rects));
                Ok(Value::Null)
            }
            ("window", "open") => {
                // The process and instance model: every document is its own process, so a second window from the same file
                // is a second process rather than a second surface in this one.
                let path = string(&params, "path");
                self.state.push(HostCommand::OpenWindow { path: path.clone() });
                Ok(Value::Null)
            }

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
                self.state.show_overlay(Overlay::Palette {
                    query: string(&params, "query").unwrap_or_default(),
                    selected: 0,
                });
                Ok(Value::Null)
            }
            ("palette", "close") => {
                self.state.close_overlay(Value::Null);
                Ok(Value::Null)
            }

            ("menu", "popup") => {
                let mut entries = Vec::new();
                flatten_menu(
                    params.get("items").and_then(|v| v.as_array()).unwrap_or(&Vec::new()),
                    0,
                    &mut entries,
                );
                if entries.is_empty() {
                    return Ok(Value::Null);
                }

                let reply = self.state.show_overlay(Overlay::Menu {
                    items: entries,
                    x: number(&params, "x").unwrap_or(0.0),
                    y: number(&params, "y").unwrap_or(0.0),
                    selected: 0,
                });
                // Resolves with the chosen id, or null if dismissed.
                Ok(reply.await.unwrap_or(Value::Null))
            }

            // --- dialogs the shell paints (docs/api-reference.md, Tier 1) ---
            //
            // Painted by GPUI rather than routed through a portal, because these are modal to *this
            // document's window*: the portal has no notion of "ask inside that window".
            ("dialog", kind @ ("message" | "confirm" | "prompt")) => {
                let dialog_kind = match kind {
                    "confirm" => DialogKind::Confirm,
                    "prompt" => DialogKind::Prompt,
                    _ => DialogKind::Message,
                };

                let reply = self.state.show_overlay(Overlay::Dialog {
                    kind: dialog_kind,
                    title: string(&params, "title").unwrap_or_default(),
                    body: string(&params, "body").unwrap_or_default(),
                    input: string(&params, "default").unwrap_or_default(),
                });

                let outcome = reply.await.unwrap_or(Value::Null);
                Ok(match dialog_kind {
                    DialogKind::Message => Value::Null,
                    DialogKind::Confirm => Value::Bool(outcome.as_bool().unwrap_or(false)),
                    DialogKind::Prompt => outcome,
                })
            }

            // --- drag out (the API catalog `dnd`) ---
            //
            // Files dragged *in* arrive as `dnd:enter`/`dnd:drop` events from the engine. Dragging
            // *out* is this call. The file is materialised here — writing `data` to a temp file if
            // that is what was given — and handed to the host, which owns the X11 selection the
            // drag needs.
            ("dnd", "startDrag") => {
                let mut paths: Vec<std::path::PathBuf> = Vec::new();

                if let Some(listed) = params.get("paths").and_then(|v| v.as_array()) {
                    for path in listed.iter().filter_map(|p| p.as_str()) {
                        // Only files the document may read can be dragged out of it.
                        paths.push(self.check_read(path)?);
                    }
                }

                if let Some(data) = params.get("data") {
                    let name = data
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("untitled");
                    // A name, not a path: this must not be a way to write outside the temp dir.
                    let name = std::path::Path::new(name)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "untitled".into());
                    let contents = data.get("contents").and_then(|v| v.as_str()).unwrap_or("");

                    let directory = std::env::temp_dir().join("htmlapp-drag");
                    std::fs::create_dir_all(&directory)
                        .map_err(|e| RpcError::internal(e.to_string()))?;
                    let path = directory.join(name);
                    std::fs::write(&path, contents)
                        .map_err(|e| RpcError::internal(e.to_string()))?;
                    paths.push(path);
                }

                if paths.is_empty() {
                    return Err(RpcError::invalid_params(
                        "startDrag needs either `paths` or `data`",
                    ));
                }

                self.state.push(HostCommand::StartDrag { paths: paths.clone() });
                Ok(json!(
                    paths
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                ))
            }

            // --- native views (docs/bridge.md) ---
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

            // --- layer-shell (docs/document-format.md) ---
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
    fn call<'a>(
        &'a self,
        module: &'a str,
        method: &'a str,
        _params: Value,
    ) -> htmlapp_bridge::BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            Err(RpcError::unsupported(format!(
                "`{module}.{method}` needs a window; this document is running headless"
            )))
        })
    }
}
