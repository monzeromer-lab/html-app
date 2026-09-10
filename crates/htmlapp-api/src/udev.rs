//! `udev` — device enumeration and hotplug (docs/api-reference.md, Tier 2).

use async_stream::stream;
use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct UdevModule {
    #[allow(dead_code)]
    ctx: Ctx,
}

#[derive(Deserialize, Default)]
struct SubsystemParams {
    #[serde(default)]
    subsystem: Option<String>,
}

impl UdevModule {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

#[cfg(feature = "tier2")]
fn device_to_json(device: &udev::Device) -> Value {
    let properties: serde_json::Map<String, Value> = device
        .properties()
        .filter_map(|property| {
            Some((
                property.name().to_str()?.to_string(),
                Value::String(property.value().to_str()?.to_string()),
            ))
        })
        .collect();

    json!({
        "path": device.syspath().to_string_lossy(),
        "subsystem": device.subsystem().and_then(|s| s.to_str()).map(str::to_string),
        "devtype": device.devtype().and_then(|s| s.to_str()).map(str::to_string),
        "properties": properties,
    })
}

impl ApiHandler for UdevModule {
    fn name(&self) -> &'static str {
        "udev"
    }

    #[cfg(feature = "tier2")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            if method != "list" {
                return Err(RpcError::not_found(&format!("udev.{method}")));
            }
            let params: SubsystemParams = decode("udev.list", params)?;

            let mut enumerator = udev::Enumerator::new()
                .map_err(|e| RpcError::internal(format!("could not open udev: {e}")))?;
            if let Some(subsystem) = &params.subsystem {
                enumerator
                    .match_subsystem(subsystem)
                    .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            }

            let devices: Vec<Value> = enumerator
                .scan_devices()
                .map_err(|e| RpcError::internal(e.to_string()))?
                .map(|device| device_to_json(&device))
                .collect();
            Ok(json!(devices))
        })
    }

    #[cfg(not(feature = "tier2"))]
    fn invoke<'a>(&'a self, _method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async { Err(RpcError::unsupported("this build has no udev support")) })
    }

    #[cfg(feature = "tier2")]
    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "monitor" {
                return Err(RpcError::not_found(&format!("udev.{method}")));
            }
            let params: SubsystemParams = decode("udev.monitor", params)?;

            // The udev monitor socket is not `Send`, so it is both created and polled on the
            // same dedicated thread; only the decoded JSON crosses back.
            let subsystem = params.subsystem.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

            std::thread::spawn(move || {
                let build = || -> Result<udev::MonitorSocket, String> {
                    let mut builder =
                        udev::MonitorBuilder::new().map_err(|e| e.to_string())?;
                    if let Some(subsystem) = &subsystem {
                        builder = builder.match_subsystem(subsystem).map_err(|e| e.to_string())?;
                    }
                    builder.listen().map_err(|e| e.to_string())
                };

                let socket = match build() {
                    Ok(socket) => {
                        let _ = ready_tx.send(Ok(()));
                        socket
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };

                loop {
                    for event in socket.iter() {
                        let payload = json!({
                            "action": event.action().and_then(|a| a.to_str()).unwrap_or(""),
                            "device": device_to_json(&event.device()),
                        });
                        if tx.send(payload).is_err() {
                            return;
                        }
                    }
                    if tx.is_closed() {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });

            // Surface a setup failure as an error on the call rather than a stream that silently
            // never yields.
            match ready_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    return Err(RpcError::internal(format!(
                        "could not open a udev monitor: {error}"
                    )));
                }
                Err(_) => return Err(RpcError::internal("the udev monitor thread did not start")),
            }

            let events = stream! {
                while let Some(event) = rx.recv().await {
                    yield Ok(event);
                }
            };
            Ok(Box::pin(events) as ValueStream)
        })
    }
}
