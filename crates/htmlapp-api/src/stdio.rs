//! `stdio` — headless mode (docs/bridge.md).
//!
//! What makes a `.hta` a legitimate participant in a shell pipeline:
//! `cat access.log | htmlapp report.hta --headless > summary.json`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_stream::try_stream;
use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;

use crate::params::decode;

pub struct StdioModule {
    /// Set when the page calls `htmlapp.exit`; the runtime polls this to know when to stop.
    exit: Arc<Mutex<Option<i32>>>,
    exited: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct WriteParams {
    data: String,
}

#[derive(Deserialize, Default)]
struct ExitParams {
    #[serde(default)]
    code: i32,
}

impl Default for StdioModule {
    fn default() -> Self {
        Self::new()
    }
}

impl StdioModule {
    pub fn new() -> Self {
        Self {
            exit: Arc::new(Mutex::new(None)),
            exited: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The exit code the page asked for, once it has asked.
    pub fn requested_exit(&self) -> Option<i32> {
        *self.exit.lock()
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }
}

impl ApiHandler for StdioModule {
    fn name(&self) -> &'static str {
        "stdio"
    }

    fn invoke<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "read" => {
                    use tokio::io::AsyncReadExt as _;
                    let mut buffer = String::new();
                    tokio::io::stdin()
                        .read_to_string(&mut buffer)
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?;
                    Ok(json!(buffer))
                }
                "write" => {
                    let params: WriteParams = decode("stdio.write", params)?;
                    let mut out = tokio::io::stdout();
                    out.write_all(params.data.as_bytes())
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?;
                    // Flushed every time: a pipeline consumer downstream should see output as it
                    // is produced, not when the process happens to exit.
                    out.flush()
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?;
                    Ok(Value::Null)
                }
                "writeErr" => {
                    let params: WriteParams = decode("stdio.writeErr", params)?;
                    let mut err = tokio::io::stderr();
                    err.write_all(params.data.as_bytes())
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?;
                    err.flush()
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?;
                    Ok(Value::Null)
                }
                "exit" => {
                    let params: ExitParams = decode("stdio.exit", params)?;
                    *self.exit.lock() = Some(params.code);
                    self.exited.store(true, Ordering::SeqCst);
                    Ok(Value::Null)
                }
                other => Err(RpcError::not_found(&format!("stdio.{other}"))),
            }
        })
    }

    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "lines" {
                return Err(RpcError::not_found(&format!("stdio.{method}")));
            }
            let stream = try_stream! {
                use tokio::io::AsyncBufReadExt as _;
                let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
                while let Some(line) = lines
                    .next_line()
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
                {
                    yield json!(line);
                }
            };
            Ok(Box::pin(stream) as ValueStream)
        })
    }
}
