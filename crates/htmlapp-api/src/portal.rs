//! `portal` — the `xdg-desktop-portal` surface (PRD §9.3 Tier 2).
//!
//! §11.2 rule 4 is the reason this module is preferred wherever it covers a capability: "the user
//! sees their desktop's own dialog, and HTML App never holds a broad grant it doesn't need." A
//! screenshot taken through the portal is one the user approved in their compositor's own UI, and
//! nothing here can take a second one without asking again.
//!
//! A side benefit §9.3 calls out: because these are portal calls, Flatpak confinement comes free.

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use htmlapp_caps::PortalCapability;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct PortalModule {
    ctx: Ctx,
    /// Live inhibitor cookies, so `uninhibit` has something to release.
    #[cfg(feature = "tier2")]
    inhibitors: parking_lot::Mutex<
        std::collections::HashMap<u64, ashpd::desktop::Session<'static, ashpd::desktop::inhibit::InhibitProxy<'static>>>,
    >,
    next: std::sync::atomic::AtomicU64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ScreenshotParams {
    #[serde(default)]
    interactive: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenUriParams {
    uri: String,
    #[serde(default)]
    writable: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InhibitParams {
    reason: String,
    #[serde(default)]
    flags: Vec<String>,
}

#[derive(Deserialize)]
struct UninhibitParams {
    handle: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundParams {
    reason: String,
    #[serde(default)]
    autostart: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WallpaperParams {
    uri: String,
    #[serde(default)]
    target: Option<String>,
}

impl PortalModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            #[cfg(feature = "tier2")]
            inhibitors: parking_lot::Mutex::new(std::collections::HashMap::new()),
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Each portal interface is a separate grant: a document that asked for `Screenshot` has not
    /// thereby asked for `Location`.
    fn check(&self, capability: PortalCapability) -> Result<(), RpcError> {
        if self.ctx.permissions().portal.contains(&capability) {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "this document was not granted the {capability:?} portal"
            )))
        }
    }
}

#[cfg(feature = "tier2")]
fn portal_error(error: ashpd::Error) -> RpcError {
    match error {
        // The user declining is an ordinary outcome, not a failure.
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            RpcError::new(htmlapp_bridge::ErrorCode::Cancelled, "the user cancelled")
        }
        other => RpcError::new(
            htmlapp_bridge::ErrorCode::OperationFailed,
            format!("the desktop portal refused: {other}"),
        ),
    }
}

#[cfg(feature = "tier2")]
impl PortalModule {
    async fn screenshot(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::screenshot::Screenshot;

        self.check(PortalCapability::Screenshot)?;
        let params: ScreenshotParams = decode("portal.screenshot", params)?;

        let response = Screenshot::request()
            .interactive(params.interactive)
            .modal(true)
            .send()
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;

        // A file URI, not the bytes: the page can hand it straight to an <img>, and the runtime
        // does not have to hold a screenshot in memory to pass it along.
        let uri = response.uri().to_string();
        // The user picked this file by taking the screenshot, so reading it is authorised the same
        // way a file-chooser result is (§11.2 rule 4).
        if let Some(path) = uri.strip_prefix("file://") {
            self.ctx.grant_from_portal(path);
        }
        Ok(json!(uri))
    }

    async fn pick_color(&self) -> Result<Value, RpcError> {
        use ashpd::desktop::Color;

        self.check(PortalCapability::Screenshot)?;
        let color = Color::pick().send().await
            .and_then(|request| request.response())
            .map_err(portal_error)?;

        Ok(json!({
            "red": color.red(),
            "green": color.green(),
            "blue": color.blue(),
        }))
    }

    async fn open_uri(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::open_uri::OpenFileRequest;

        self.check(PortalCapability::OpenUri)?;
        let params: OpenUriParams = decode("portal.openUri", params)?;

        let uri = url::Url::parse(&params.uri)
            .map_err(|e| RpcError::invalid_params(format!("not a URI: {e}")))?;

        OpenFileRequest::default()
            .writeable(params.writable)
            .ask(true)
            .send_uri(&uri)
            .await
            .map_err(portal_error)?;
        Ok(Value::Null)
    }

    async fn inhibit(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::inhibit::{InhibitFlags, InhibitProxy};

        self.check(PortalCapability::Inhibit)?;
        let params: InhibitParams = decode("portal.inhibit", params)?;

        let flags: ashpd::enumflags2::BitFlags<InhibitFlags> = params
            .flags
            .iter()
            .filter_map(|flag| match flag.as_str() {
                "logout" => Some(InhibitFlags::Logout),
                "switch" => Some(InhibitFlags::UserSwitch),
                "suspend" => Some(InhibitFlags::Suspend),
                "idle" => Some(InhibitFlags::Idle),
                _ => None,
            })
            .fold(Default::default(), |acc, flag| acc | flag);

        let proxy = InhibitProxy::new().await.map_err(portal_error)?;
        let session = proxy
            .create_monitor(None)
            .await
            .map_err(portal_error)?;
        proxy
            .inhibit(None, flags, &params.reason)
            .await
            .map_err(portal_error)?;

        let handle = self
            .next
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inhibitors.lock().insert(handle, session);
        Ok(json!(handle))
    }

    async fn uninhibit(&self, params: Value) -> Result<Value, RpcError> {
        let params: UninhibitParams = decode("portal.uninhibit", params)?;
        // Dropping the session closes it, which is what releases the inhibitor.
        self.inhibitors.lock().remove(&params.handle);
        Ok(Value::Null)
    }

    async fn request_background(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::background::Background;

        self.check(PortalCapability::Background)?;
        let params: BackgroundParams = decode("portal.requestBackground", params)?;

        let response = Background::request()
            .reason(params.reason.as_str())
            .auto_start(params.autostart)
            .send()
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;

        Ok(json!(response.run_in_background()))
    }

    async fn set_wallpaper(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::wallpaper::{SetOn, WallpaperRequest};

        self.check(PortalCapability::Wallpaper)?;
        let params: WallpaperParams = decode("portal.setWallpaper", params)?;

        let uri = url::Url::parse(&params.uri)
            .map_err(|e| RpcError::invalid_params(format!("not a URI: {e}")))?;
        let set_on = match params.target.as_deref() {
            Some("lockscreen") => SetOn::Lockscreen,
            Some("both") => SetOn::Both,
            _ => SetOn::Background,
        };

        WallpaperRequest::default()
            .set_on(set_on)
            .show_preview(true)
            .build_uri(&uri)
            .await
            .map_err(portal_error)?;
        Ok(Value::Null)
    }

    async fn location(&self) -> Result<Value, RpcError> {
        use ashpd::desktop::location::LocationProxy;
        use futures::StreamExt as _;

        self.check(PortalCapability::Location)?;

        let proxy = LocationProxy::new().await.map_err(portal_error)?;
        let session = proxy
            .create_session(None, None, None)
            .await
            .map_err(portal_error)?;

        let mut updates = proxy.receive_location_updated().await.map_err(portal_error)?;
        proxy.start(&session, None).await.map_err(portal_error)?;

        // One fix, then done. A page that wants continuous tracking should say so explicitly
        // rather than have a single call quietly keep the GPS awake.
        let fix = tokio::time::timeout(std::time::Duration::from_secs(30), updates.next())
            .await
            .map_err(|_| {
                RpcError::new(
                    htmlapp_bridge::ErrorCode::OperationFailed,
                    "timed out waiting for a location fix",
                )
            })?
            .ok_or_else(|| {
                RpcError::new(
                    htmlapp_bridge::ErrorCode::OperationFailed,
                    "the location portal closed without a fix",
                )
            })?;

        Ok(json!({
            "latitude": fix.latitude(),
            "longitude": fix.longitude(),
            "accuracy": fix.accuracy(),
        }))
    }
}

impl ApiHandler for PortalModule {
    fn name(&self) -> &'static str {
        "portal"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier2")]
                "screenshot" => self.screenshot(params).await,
                #[cfg(feature = "tier2")]
                "pickColor" => self.pick_color().await,
                #[cfg(feature = "tier2")]
                "openUri" => self.open_uri(params).await,
                #[cfg(feature = "tier2")]
                "inhibit" => self.inhibit(params).await,
                #[cfg(feature = "tier2")]
                "uninhibit" => self.uninhibit(params).await,
                #[cfg(feature = "tier2")]
                "requestBackground" => self.request_background(params).await,
                #[cfg(feature = "tier2")]
                "setWallpaper" => self.set_wallpaper(params).await,
                #[cfg(feature = "tier2")]
                "location" => self.location().await,

                #[cfg(not(feature = "tier2"))]
                _ if true => {
                    let _ = &params;
                    Err(RpcError::unsupported("this build has no portal support"))
                }

                other => Err(RpcError::not_found(&format!("portal.{other}"))),
            }
        })
    }

    #[cfg(feature = "tier2")]
    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "screenCast" {
                return Err(RpcError::not_found(&format!("portal.{method}")));
            }
            self.check(PortalCapability::ScreenCast)?;
            let _ = params;

            // ScreenCast hands back a PipeWire node id; consuming it needs a PipeWire client, which
            // belongs with the `video` native view rather than here. Saying so is better than
            // returning a node id the page has no way to use.
            Err(RpcError::unsupported(
                "portal.screenCast needs the PipeWire consumer that ships with the `video` native \
                 view (PRD §10), which is not implemented yet",
            ))
        })
    }
}
