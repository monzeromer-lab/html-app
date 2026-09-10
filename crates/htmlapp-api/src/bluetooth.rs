//! `bluetooth` — adapters and devices, via BlueZ (docs/api-reference.md, Tier 4).
//!
//! `bluer` talks to `bluetoothd` over D-Bus, so this needs a running BlueZ but no `libbluetooth`
//! development package.

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde_json::Value;

use crate::context::Ctx;

#[cfg(feature = "bluetooth")]
use {
    async_stream::try_stream, htmlapp_bridge::dispatch::ValueStream, crate::params::decode,
    serde::Deserialize, serde_json::json,
};

pub struct BluetoothModule {
    #[cfg_attr(not(feature = "bluetooth"), allow(dead_code))]
    ctx: Ctx,
}

#[cfg(feature = "bluetooth")]
#[derive(Deserialize)]
struct AddressParams {
    address: String,
}

#[cfg(feature = "bluetooth")]
#[derive(Deserialize, Default)]
struct DiscoverParams {
    #[serde(default)]
    timeout: Option<u64>,
}

impl BluetoothModule {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    #[cfg_attr(not(feature = "bluetooth"), allow(dead_code))]
    fn check(&self) -> Result<(), RpcError> {
        if self.ctx.permissions().bluetooth {
            Ok(())
        } else {
            Err(RpcError::denied("this document was not granted `bluetooth`"))
        }
    }
}

#[cfg(feature = "bluetooth")]
fn parse_address(address: &str) -> Result<bluer::Address, RpcError> {
    address
        .parse()
        .map_err(|_| RpcError::invalid_params(format!("`{address}` is not a Bluetooth address")))
}

#[cfg(feature = "bluetooth")]
fn unavailable(error: bluer::Error) -> RpcError {
    RpcError::new(
        htmlapp_bridge::ErrorCode::OperationFailed,
        format!("BlueZ refused: {error}"),
    )
}

impl ApiHandler for BluetoothModule {
    fn name(&self) -> &'static str {
        "bluetooth"
    }

    #[cfg(feature = "bluetooth")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            self.check()?;
            let session = bluer::Session::new().await.map_err(unavailable)?;

            match method {
                "adapters" => {
                    let mut adapters = Vec::new();
                    for name in session.adapter_names().await.map_err(unavailable)? {
                        let Ok(adapter) = session.adapter(&name) else {
                            continue;
                        };
                        adapters.push(json!({
                            "address": adapter.address().await.map(|a| a.to_string()).unwrap_or_default(),
                            "name": name,
                            "powered": adapter.is_powered().await.unwrap_or(false),
                        }));
                    }
                    Ok(json!(adapters))
                }

                "connect" | "disconnect" => {
                    let params: AddressParams = decode("bluetooth", params)?;
                    let address = parse_address(&params.address)?;
                    let adapter = session.default_adapter().await.map_err(unavailable)?;
                    let device = adapter.device(address).map_err(unavailable)?;

                    if method == "connect" {
                        device.connect().await.map_err(unavailable)?;
                    } else {
                        device.disconnect().await.map_err(unavailable)?;
                    }
                    Ok(Value::Null)
                }

                other => Err(RpcError::not_found(&format!("bluetooth.{other}"))),
            }
        })
    }

    #[cfg(not(feature = "bluetooth"))]
    fn invoke<'a>(&'a self, _method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async {
            Err(RpcError::unsupported(
                "this build was compiled without the `bluetooth` feature",
            ))
        })
    }

    #[cfg(feature = "bluetooth")]
    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "discover" {
                return Err(RpcError::not_found(&format!("bluetooth.{method}")));
            }
            self.check()?;
            let params: DiscoverParams = decode("bluetooth.discover", params)?;
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(params.timeout.unwrap_or(30));

            let found = try_stream! {
                use futures::StreamExt as _;

                let session = bluer::Session::new().await.map_err(unavailable)?;
                let adapter = session.default_adapter().await.map_err(unavailable)?;
                adapter.set_powered(true).await.map_err(unavailable)?;

                let mut events = adapter.discover_devices().await.map_err(unavailable)?;
                while let Some(event) = events.next().await {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    // Only additions are reported; a removal is not a discovery.
                    let bluer::AdapterEvent::DeviceAdded(address) = event else {
                        continue;
                    };
                    let Ok(device) = adapter.device(address) else { continue };
                    yield json!({
                        "address": address.to_string(),
                        "name": device.name().await.ok().flatten(),
                        "rssi": device.rssi().await.ok().flatten(),
                        "paired": device.is_paired().await.unwrap_or(false),
                    });
                }
            };
            Ok(Box::pin(found) as ValueStream)
        })
    }
}
