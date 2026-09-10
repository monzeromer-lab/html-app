//! `notify` — desktop notifications (docs/api-reference.md, Tier 2).
//!
//! `org.freedesktop.Notifications` directly rather than through the portal, because the portal's
//! Notification interface has no replace-id and no progress hint, and both are what a long-running
//! tool actually needs.

use std::collections::HashMap;

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct NotifyModule {
    #[allow(dead_code)]
    ctx: Ctx,
    app_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotificationParams {
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    urgency: Option<String>,
    #[serde(default)]
    timeout_ms: Option<i32>,
    #[serde(default)]
    actions: Vec<Action>,
    #[serde(default)]
    replace_id: Option<u32>,
    #[serde(default)]
    progress: Option<i32>,
}

#[derive(Deserialize)]
struct Action {
    id: String,
    label: String,
}

#[derive(Deserialize)]
struct CloseParams {
    id: u32,
}

impl NotifyModule {
    pub fn new(ctx: Ctx, app_name: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            ctx,
        }
    }
}

#[cfg(feature = "tier2")]
impl NotifyModule {
    async fn send(&self, params: Value) -> Result<Value, RpcError> {
        let params: NotificationParams = decode("notify.send", params)?;
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| RpcError::internal(format!("no session bus: {e}")))?;

        let mut hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
        if let Some(urgency) = params.urgency.as_deref() {
            let level: u8 = match urgency {
                "low" => 0,
                "critical" => 2,
                _ => 1,
            };
            hints.insert("urgency", zbus::zvariant::Value::U8(level));
        }
        if let Some(progress) = params.progress {
            hints.insert("value", zbus::zvariant::Value::I32(progress.clamp(0, 100)));
        }

        // The wire format is a flat [id, label, id, label, ...] list.
        let actions: Vec<String> = params
            .actions
            .iter()
            .flat_map(|a| [a.id.clone(), a.label.clone()])
            .collect();

        let reply: u32 = connection
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    self.app_name.as_str(),
                    params.replace_id.unwrap_or(0),
                    params.icon.as_deref().unwrap_or(""),
                    params.title.as_str(),
                    params.body.as_deref().unwrap_or(""),
                    actions,
                    hints,
                    params.timeout_ms.unwrap_or(-1),
                ),
            )
            .await
            .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?
            .body()
            .deserialize()
            .map_err(|e| RpcError::internal(e.to_string()))?;

        Ok(json!(reply))
    }

    async fn close(&self, params: Value) -> Result<Value, RpcError> {
        let params: CloseParams = decode("notify.close", params)?;
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| RpcError::internal(format!("no session bus: {e}")))?;
        connection
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "CloseNotification",
                &(params.id,),
            )
            .await
            .map_err(|e| {
                RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
            })?;
        Ok(Value::Null)
    }
}

impl ApiHandler for NotifyModule {
    fn name(&self) -> &'static str {
        "notify"
    }

    fn invoke<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier2")]
                "send" => self.send(params).await,
                #[cfg(feature = "tier2")]
                "close" => self.close(params).await,
                #[cfg(not(feature = "tier2"))]
                "send" | "close" => {
                    let _ = &params;
                    Err(RpcError::unsupported("this build has no D-Bus support"))
                }
                other => Err(RpcError::not_found(&format!("notify.{other}"))),
            }
        })
    }
}
