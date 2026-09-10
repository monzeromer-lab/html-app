//! `usb` — USB device access (PRD §9.3 Tier 4).
//!
//! `rusb` builds libusb from source here, so this works on a machine without a `libusb-1.0-dev`
//! package. A hardware API that exists only where the right `-dev` package happens to be installed
//! is not much of an API.
//!
//! Transfers are blocking, so each one runs on the blocking pool rather than on a Tokio worker.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use parking_lot::Mutex;
use serde_json::Value;

use crate::context::Ctx;

#[cfg(feature = "usb")]
use {
    crate::params::decode, serde::Deserialize, serde_json::json,
    std::sync::atomic::Ordering, std::time::Duration,
};

#[cfg(feature = "usb")]
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);

pub struct UsbModule {
    #[cfg_attr(not(feature = "usb"), allow(dead_code))]
    ctx: Ctx,
    #[cfg(feature = "usb")]
    devices: Arc<Mutex<HashMap<u64, rusb::DeviceHandle<rusb::GlobalContext>>>>,
    #[cfg(not(feature = "usb"))]
    #[allow(dead_code)]
    devices: Arc<Mutex<HashMap<u64, ()>>>,
    #[cfg_attr(not(feature = "usb"), allow(dead_code))]
    next: AtomicU64,
}

#[cfg(feature = "usb")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceParams {
    vendor_id: u16,
    product_id: u16,
}

#[cfg(feature = "usb")]
#[derive(Deserialize)]
struct HandleParams {
    handle: u64,
}

#[cfg(feature = "usb")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferParams {
    handle: u64,
    endpoint: u8,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    length: Option<usize>,
}

impl UsbModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            devices: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        }
    }

    #[cfg_attr(not(feature = "usb"), allow(dead_code))]
    fn check(&self) -> Result<(), RpcError> {
        if self.ctx.permissions().usb {
            Ok(())
        } else {
            Err(RpcError::denied("this document was not granted `usb`"))
        }
    }
}

impl ApiHandler for UsbModule {
    fn name(&self) -> &'static str {
        "usb"
    }

    #[cfg(feature = "usb")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            self.check()?;
            match method {
                "list" => {
                    let devices = tokio::task::spawn_blocking(|| {
                        let list = rusb::devices()?;
                        let mut out = Vec::new();
                        for device in list.iter() {
                            let Ok(descriptor) = device.device_descriptor() else {
                                continue;
                            };
                            // Opening a device to read its strings needs permission the user may
                            // not have granted at the udev level; the ids are always readable.
                            let (manufacturer, product, serial) = match device.open() {
                                Ok(handle) => (
                                    handle.read_manufacturer_string_ascii(&descriptor).ok(),
                                    handle.read_product_string_ascii(&descriptor).ok(),
                                    handle.read_serial_number_string_ascii(&descriptor).ok(),
                                ),
                                Err(_) => (None, None, None),
                            };
                            out.push(json!({
                                "vendorId": descriptor.vendor_id(),
                                "productId": descriptor.product_id(),
                                "manufacturer": manufacturer,
                                "product": product,
                                "serial": serial,
                            }));
                        }
                        Ok::<_, rusb::Error>(out)
                    })
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
                    .map_err(|e| RpcError::new(
                        htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    Ok(json!(devices))
                }

                "open" => {
                    let params: DeviceParams = decode("usb.open", params)?;
                    let handle = tokio::task::spawn_blocking(move || {
                        rusb::open_device_with_vid_pid(params.vendor_id, params.product_id)
                    })
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
                    .ok_or_else(|| {
                        RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed,
                            format!(
                                "no USB device {:04x}:{:04x}, or it could not be opened \
                                 (a udev rule may be needed)",
                                params.vendor_id, params.product_id
                            ),
                        )
                    })?;

                    let id = self.next.fetch_add(1, Ordering::SeqCst);
                    self.devices.lock().insert(id, handle);
                    Ok(json!(id))
                }

                "transfer" => {
                    use base64::Engine as _;
                    let params: TransferParams = decode("usb.transfer", params)?;

                    let devices = Arc::clone(&self.devices);
                    let result = tokio::task::spawn_blocking(move || {
                        let devices = devices.lock();
                        let handle = devices
                            .get(&params.handle)
                            .ok_or_else(|| "no such USB handle".to_string())?;

                        // The direction bit of the endpoint address decides which way this goes,
                        // which is how libusb itself is addressed.
                        if params.endpoint & 0x80 == 0 {
                            let payload = params.data.unwrap_or_default();
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(payload.as_bytes())
                                .map_err(|e| format!("data is not base64: {e}"))?;
                            let written = handle
                                .write_bulk(params.endpoint, &bytes, TRANSFER_TIMEOUT)
                                .map_err(|e| e.to_string())?;
                            Ok::<String, String>(
                                base64::engine::general_purpose::STANDARD
                                    .encode(&bytes[..written]),
                            )
                        } else {
                            let mut buffer = vec![0u8; params.length.unwrap_or(512).min(1 << 20)];
                            let read = handle
                                .read_bulk(params.endpoint, &mut buffer, TRANSFER_TIMEOUT)
                                .map_err(|e| e.to_string())?;
                            Ok(base64::engine::general_purpose::STANDARD.encode(&buffer[..read]))
                        }
                    })
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
                    .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e))?;
                    Ok(json!(result))
                }

                "close" => {
                    let params: HandleParams = decode("usb.close", params)?;
                    self.devices.lock().remove(&params.handle);
                    Ok(Value::Null)
                }

                other => Err(RpcError::not_found(&format!("usb.{other}"))),
            }
        })
    }

    #[cfg(not(feature = "usb"))]
    fn invoke<'a>(&'a self, _method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async {
            Err(RpcError::unsupported(
                "this build was compiled without the `usb` feature",
            ))
        })
    }
}
