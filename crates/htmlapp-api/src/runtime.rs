//! `runtime` — bridge bookkeeping.
//!
//! Ambient rather than granted: the page needs a way to say "the shim finished initialising"
//! regardless of what its manifest asked for, and the host needs somewhere to answer questions
//! about itself that carry no capability.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde_json::{Value, json};

pub struct RuntimeModule {
    version: String,
    granted: Vec<String>,
    ready: Arc<AtomicBool>,
}

impl RuntimeModule {
    pub fn new(version: impl Into<String>, granted: Vec<String>) -> Self {
        Self {
            version: version.into(),
            granted,
            ready: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the page's shim has finished initialising.
    ///
    /// Headless mode uses this to tell "the document is still starting" apart from "the document
    /// finished without calling exit", which would otherwise look identical.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
}

impl ApiHandler for RuntimeModule {
    fn name(&self) -> &'static str {
        "runtime"
    }

    fn invoke<'a>(&'a self, method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "ready" => {
                    self.ready.store(true, Ordering::SeqCst);
                    tracing::debug!("the page's bridge is ready");
                    Ok(Value::Null)
                }
                "version" => Ok(json!({
                    "version": self.version,
                    "granted": self.granted,
                })),
                other => Err(RpcError::not_found(&format!("runtime.{other}"))),
            }
        })
    }
}
