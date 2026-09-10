//! `serial` — serial ports (PRD §9.3 Tier 4).
//!
//! "Turns HTML App into a viable host for hardware and embedded tooling" — UC7. The manifest's
//! `serial` list names the ports a document may open; an empty list grants none, because a serial
//! console that can open any port is a serial console that can talk to your BMC.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_stream::stream;
use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

/// An open port: a writer, and a broadcast of what has been read.
struct OpenPort {
    #[cfg(feature = "tier4")]
    writer: Box<dyn serialport::SerialPort>,
    reader: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>,
    #[allow(dead_code)]
    port: String,
}

pub struct SerialModule {
    ctx: Ctx,
    ports: Arc<Mutex<HashMap<u64, OpenPort>>>,
    next: AtomicU64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenParams {
    port: String,
    #[serde(default)]
    baud_rate: Option<u32>,
    #[serde(default)]
    data_bits: Option<u8>,
    #[serde(default)]
    parity: Option<String>,
    #[serde(default)]
    stop_bits: Option<u8>,
}

#[derive(Deserialize)]
struct HandleParams {
    handle: u64,
}

#[derive(Deserialize)]
struct WriteParams {
    handle: u64,
    data: String,
}

impl SerialModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            ports: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        }
    }

    /// Check a port against the manifest's `serial` list.
    ///
    /// Entries may be exact paths (`/dev/ttyUSB0`) or globs (`/dev/ttyUSB*`), because which number
    /// a USB adapter is assigned is not stable across reboots.
    fn check_port(&self, port: &str) -> Result<(), RpcError> {
        let allowed = self.ctx.permissions().serial.clone();
        if allowed.is_empty() {
            return Err(RpcError::denied(
                "this document was not granted any serial ports",
            ));
        }
        let permitted = allowed.iter().any(|pattern| {
            globset::Glob::new(pattern)
                .map(|glob| glob.compile_matcher().is_match(port))
                .unwrap_or(false)
                || pattern == port
        });
        if permitted {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "`{port}` is not in this document's `serial` list ({})",
                allowed.join(", ")
            )))
        }
    }
}

impl ApiHandler for SerialModule {
    fn name(&self) -> &'static str {
        "serial"
    }

    #[cfg(feature = "tier4")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "list" => {
                    let ports = serialport::available_ports()
                        .map_err(|e| RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    // Only ports the manifest covers are reported: enumeration should not be a way
                    // to discover what else is attached.
                    let listed: Vec<Value> = ports
                        .into_iter()
                        .filter(|port| self.check_port(&port.port_name).is_ok())
                        .map(|port| {
                            let (kind, manufacturer, product) = match &port.port_type {
                                serialport::SerialPortType::UsbPort(info) => (
                                    "usb",
                                    info.manufacturer.clone(),
                                    info.product.clone(),
                                ),
                                serialport::SerialPortType::BluetoothPort => ("bluetooth", None, None),
                                serialport::SerialPortType::PciPort => ("pci", None, None),
                                serialport::SerialPortType::Unknown => ("unknown", None, None),
                            };
                            json!({
                                "port": port.port_name,
                                "kind": kind,
                                "manufacturer": manufacturer,
                                "product": product,
                            })
                        })
                        .collect();
                    Ok(json!(listed))
                }

                "open" => {
                    let params: OpenParams = decode("serial.open", params)?;
                    self.check_port(&params.port)?;

                    let builder = serialport::new(&params.port, params.baud_rate.unwrap_or(115_200))
                        .data_bits(match params.data_bits.unwrap_or(8) {
                            5 => serialport::DataBits::Five,
                            6 => serialport::DataBits::Six,
                            7 => serialport::DataBits::Seven,
                            _ => serialport::DataBits::Eight,
                        })
                        .parity(match params.parity.as_deref() {
                            Some("odd") => serialport::Parity::Odd,
                            Some("even") => serialport::Parity::Even,
                            _ => serialport::Parity::None,
                        })
                        .stop_bits(match params.stop_bits.unwrap_or(1) {
                            2 => serialport::StopBits::Two,
                            _ => serialport::StopBits::One,
                        })
                        // A read timeout rather than a block, so the reader thread can notice the
                        // port has been closed instead of sitting in the kernel forever.
                        .timeout(std::time::Duration::from_millis(200));

                    let writer = builder.open().map_err(|e| {
                        RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed,
                            format!("could not open {}: {e}", params.port),
                        )
                    })?;
                    let mut reader = writer.try_clone().map_err(|e| {
                        RpcError::internal(format!("could not clone the port: {e}"))
                    })?;

                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    std::thread::spawn(move || {
                        use std::io::Read as _;
                        let mut buffer = [0u8; 4096];
                        loop {
                            match reader.read(&mut buffer) {
                                Ok(0) => continue,
                                Ok(n) => {
                                    if tx.send(buffer[..n].to_vec()).is_err() {
                                        return;
                                    }
                                }
                                // A timeout is the normal idle case, not a failure.
                                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                                    if tx.is_closed() {
                                        return;
                                    }
                                }
                                Err(_) => return,
                            }
                        }
                    });

                    let handle = self.next.fetch_add(1, Ordering::SeqCst);
                    self.ports.lock().insert(
                        handle,
                        OpenPort { writer, reader: Some(rx), port: params.port },
                    );
                    Ok(json!(handle))
                }

                "write" => {
                    let params: WriteParams = decode("serial.write", params)?;
                    use std::io::Write as _;
                    let mut ports = self.ports.lock();
                    let port = ports.get_mut(&params.handle).ok_or_else(|| {
                        RpcError::invalid_params("no such serial handle")
                    })?;
                    port.writer.write_all(params.data.as_bytes()).map_err(|e| {
                        RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string())
                    })?;
                    let _ = port.writer.flush();
                    Ok(Value::Null)
                }

                "close" => {
                    let params: HandleParams = decode("serial.close", params)?;
                    self.ports.lock().remove(&params.handle);
                    Ok(Value::Null)
                }

                other => Err(RpcError::not_found(&format!("serial.{other}"))),
            }
        })
    }

    #[cfg(not(feature = "tier4"))]
    fn invoke<'a>(&'a self, _method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async { Err(RpcError::unsupported("this build has no serial support")) })
    }

    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "read" {
                return Err(RpcError::not_found(&format!("serial.{method}")));
            }
            let params: HandleParams = decode("serial.read", params)?;

            // The receiver is moved out: one reader per open port, so two concurrent `read`
            // streams cannot each consume half the bytes.
            let mut receiver = {
                let mut ports = self.ports.lock();
                let port = ports
                    .get_mut(&params.handle)
                    .ok_or_else(|| RpcError::invalid_params("no such serial handle"))?;
                port.reader
                    .take()
                    .ok_or_else(|| RpcError::invalid_params("that port is already being read"))?
            };

            let bytes = stream! {
                while let Some(chunk) = receiver.recv().await {
                    yield Ok(json!(String::from_utf8_lossy(&chunk)));
                }
            };
            Ok(Box::pin(bytes) as ValueStream)
        })
    }
}
