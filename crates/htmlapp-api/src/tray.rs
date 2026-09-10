//! `tray` — StatusNotifierItem (docs/api-reference.md, Tier 2).
//!
//! `ksni` speaks StatusNotifierItem, which is what KDE, most wlroots bars, and GNOME with the
//! AppIndicator extension actually watch. The older XEmbed tray is not implemented: it needs an X11
//! window, and a tray icon that only works on X11 would be a trap on a Wayland-first runtime.

use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use htmlapp_bridge::{Events, RpcError};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct TraySpec {
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    tooltip: Option<String>,
    #[serde(default)]
    menu: Vec<MenuSpec>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct MenuSpec {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    separator: bool,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    checked: Option<bool>,
    #[serde(default)]
    submenu: Vec<MenuSpec>,
}

fn yes() -> bool {
    true
}

/// The item `ksni` drives. Events are pushed straight to the page.
#[cfg(feature = "tier2")]
struct HtmlAppTray {
    app_id: String,
    spec: TraySpec,
    events: Events,
}

#[cfg(feature = "tier2")]
impl ksni::Tray for HtmlAppTray {
    fn id(&self) -> String {
        self.app_id.clone()
    }

    fn title(&self) -> String {
        self.spec.title.clone().unwrap_or_else(|| self.app_id.clone())
    }

    fn icon_name(&self) -> String {
        // A themed icon name. A `data:` icon would need decoding into ARGB32 pixmaps, which is
        // more machinery than a tray icon warrants; naming a theme icon is what most tools do.
        self.spec
            .icon
            .clone()
            .unwrap_or_else(|| "application-x-executable".into())
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.spec.title.clone().unwrap_or_default(),
            description: self.spec.tooltip.clone().unwrap_or_default(),
            icon_name: self.icon_name(),
            icon_pixmap: Vec::new(),
        }
    }

    fn activate(&mut self, x: i32, y: i32) {
        self.events.emit("tray:activate", json!({ "x": x, "y": y }));
    }

    fn secondary_activate(&mut self, x: i32, y: i32) {
        self.events
            .emit("tray:activate", json!({ "x": x, "y": y, "secondary": true }));
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        build_menu(&self.spec.menu)
    }
}

/// Translate the manifest-shaped menu JSON into `ksni` items.
#[cfg(feature = "tier2")]
fn build_menu(items: &[MenuSpec]) -> Vec<ksni::MenuItem<HtmlAppTray>> {
    items
        .iter()
        .map(|item| {
            if item.separator {
                return ksni::MenuItem::Separator;
            }

            let id = item.id.clone().unwrap_or_default();
            let label = item.label.clone().unwrap_or_default();

            if !item.submenu.is_empty() {
                return ksni::menu::SubMenu {
                    label,
                    enabled: item.enabled,
                    submenu: build_menu(&item.submenu),
                    ..Default::default()
                }
                .into();
            }

            match item.checked {
                Some(checked) => ksni::menu::CheckmarkItem {
                    label,
                    enabled: item.enabled,
                    checked,
                    activate: Box::new(move |tray: &mut HtmlAppTray| {
                        tray.events.emit("tray:menu", json!({ "id": id }));
                    }),
                    ..Default::default()
                }
                .into(),
                None => ksni::menu::StandardItem {
                    label,
                    enabled: item.enabled,
                    activate: Box::new(move |tray: &mut HtmlAppTray| {
                        tray.events.emit("tray:menu", json!({ "id": id }));
                    }),
                    ..Default::default()
                }
                .into(),
            }
        })
        .collect()
}

pub struct TrayModule {
    #[allow(dead_code)]
    ctx: Ctx,
    app_id: String,
    events: Events,
    #[cfg(feature = "tier2")]
    handle: Mutex<Option<ksni::blocking::Handle<HtmlAppTray>>>,
    #[cfg(not(feature = "tier2"))]
    handle: Mutex<Option<()>>,
}

impl TrayModule {
    pub fn new(ctx: Ctx, app_id: impl Into<String>, events: Events) -> Self {
        Self {
            ctx,
            app_id: app_id.into(),
            events,
            handle: Mutex::new(None),
        }
    }
}

impl ApiHandler for TrayModule {
    fn name(&self) -> &'static str {
        "tray"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier2")]
                "set" => {
                    use ksni::blocking::TrayMethods as _;
                    let spec: TraySpec = decode("tray.set", params)?;

                    let mut handle = self.handle.lock();
                    // Updating in place keeps the item's position in the tray; recreating it would
                    // make it jump to the end every time the title changed.
                    if let Some(existing) = handle.as_ref() {
                        let updated = spec.clone();
                        existing.update(move |tray| tray.spec = updated);
                        return Ok(Value::Null);
                    }

                    let tray = HtmlAppTray {
                        app_id: self.app_id.clone(),
                        spec,
                        events: self.events.clone(),
                    };
                    let spawned = tray.spawn().map_err(|e| {
                        RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed,
                            format!("could not register a tray item: {e}"),
                        )
                    })?;
                    *handle = Some(spawned);
                    Ok(Value::Null)
                }
                #[cfg(feature = "tier2")]
                "remove" => {
                    // Dropping the handle withdraws the item from the tray.
                    *self.handle.lock() = None;
                    Ok(Value::Null)
                }
                #[cfg(not(feature = "tier2"))]
                "set" | "remove" => {
                    let _ = &params;
                    Err(RpcError::unsupported("this build has no tray support"))
                }
                other => Err(RpcError::not_found(&format!("tray.{other}"))),
            }
        })
    }
}

/// Withdraw the tray item when the document closes, so a closed app leaves no icon behind.
impl Drop for TrayModule {
    fn drop(&mut self) {
        *self.handle.lock() = None;
    }
}
