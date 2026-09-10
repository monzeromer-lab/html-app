//! Modules whose implementation lives in the GPUI shell rather than here.
//!
//! `window`, `layer`, `menu`, `palette`, and the native views of the native views design are all things only the host's
//! UI thread can do. Rather than give this crate a dependency on GPUI — which would drag the whole
//! renderer into every build, including headless — those modules forward across a [`HostBridge`]
//! that `htmlapp-runtime` implements.

use std::sync::Arc;

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde_json::Value;

/// Implemented by the shell for the modules it owns.
///
/// Async because some of these calls resolve on a *person*: `dialog.confirm` cannot answer until
/// The user clicks something. A synchronous signature would force the bridge worker to block on the
/// render loop, which is the one thing that must never happen — the render loop is what draws the
/// dialog being waited on.
pub trait HostBridge: Send + Sync + 'static {
    /// Perform one host-side call. Returning `MethodNotFound` is the correct answer for a method
    /// this host does not implement — a headless host implements almost none of them.
    fn call<'a>(
        &'a self,
        module: &'a str,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>>;
}

/// Forwards one module's calls to the host.
pub struct HostModule {
    name: &'static str,
    host: Arc<dyn HostBridge>,
}

impl HostModule {
    pub fn new(name: &'static str, host: Arc<dyn HostBridge>) -> Self {
        Self { name, host }
    }
}

impl ApiHandler for HostModule {
    fn name(&self) -> &'static str {
        self.name
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move { self.host.call(self.name, method, params).await })
    }
}

/// A host that implements nothing — used in headless mode, where there is no window to control.
pub struct NullHost;

impl HostBridge for NullHost {
    fn call<'a>(
        &'a self,
        module: &'a str,
        method: &'a str,
        _params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            Err(RpcError::unsupported(format!(
                "`{module}.{method}` needs a window; this document is running headless"
            )))
        })
    }
}
