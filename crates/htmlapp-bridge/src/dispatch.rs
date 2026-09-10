//! Routing page calls to native handlers (docs/bridge.md).
//!
//! The dispatcher is the single choke point between the page and every native capability, which is
//! what the threat model means by "the bridge is the only channel". It gates on the manifest before a handler
//! is ever reached, so a handler cannot be invoked for a module the document was not granted even
//! if it is registered.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::stream::StreamExt;
use htmlapp_caps::Permissions;
use parking_lot::Mutex;
use serde_json::Value;

use crate::protocol::{ClientMessage, ErrorCode, HostMessage, RequestId, RpcError, split_method};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type ValueStream = Pin<Box<dyn futures::Stream<Item = Result<Value, RpcError>> + Send>>;

/// Modules that exist regardless of the manifest.
///
/// `runtime` is bridge bookkeeping, `view` addresses native views the page itself placed in its own
/// layout, and `stdio` is only ever registered when the document is headless. None of them reach
/// anything the manifest would otherwise gate.
const AMBIENT_MODULES: &[&str] = &["runtime", "view", "stdio"];

/// One native API module (`fs`, `process`, `dbus`, …).
pub trait ApiHandler: Send + Sync + 'static {
    /// The property name this handler serves on the `htmlapp` global.
    fn name(&self) -> &'static str;

    /// Handle a request/response call.
    fn invoke<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<Value, RpcError>>;

    /// Open a stream. The default refuses, so a module with no streaming methods says so honestly
    /// rather than hanging a `for await` loop forever.
    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        _params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        let qualified = format!("{}.{}", self.name(), method);
        Box::pin(async move { Err(RpcError::not_found(&qualified)) })
    }

    /// Called when the page subscribes to one of this module's events. Handlers that emit events
    /// use this to start whatever produces them.
    fn subscribe(&self, _event: &str) -> Result<(), RpcError> {
        Ok(())
    }

    /// Called when the last subscriber for an event goes away.
    fn unsubscribe(&self, _event: &str) {}
}

/// How host→page messages get delivered. Implemented over the engine's script evaluation, and over
/// an in-memory queue in tests.
pub trait Transport: Send + Sync + 'static {
    fn send(&self, message: HostMessage);
}

/// Tracks an open stream so it can be cancelled from the page.
struct OpenStream {
    cancelled: Arc<AtomicBool>,
}

/// A handle a module can hold to push events to the page (docs/bridge.md).
///
/// Separate from [`Dispatcher`] because events flow the other way: a tray click, a global hotkey,
/// or a udev hotplug originates in a module and has no request to reply to. Sharing the
/// subscription table means an event with no listener costs nothing rather than being serialised
/// and thrown away.
#[derive(Clone)]
pub struct Events {
    transport: Arc<dyn Transport>,
    subscriptions: Arc<Mutex<HashMap<RequestId, String>>>,
}

impl Events {
    /// Emit an event, if anything is listening for it.
    pub fn emit(&self, event: &str, payload: Value) {
        let listening = self
            .subscriptions
            .lock()
            .values()
            .any(|subscribed| subscribed == event);
        if listening {
            self.transport.send(HostMessage::Event {
                event: event.to_string(),
                payload,
            });
        }
    }

    /// Whether anything is subscribed to this event.
    ///
    /// Lets a module avoid doing the work behind an event nobody wants — polling battery state, or
    /// keeping a clipboard watcher open.
    pub fn has_listener(&self, event: &str) -> bool {
        self.subscriptions
            .lock()
            .values()
            .any(|subscribed| subscribed == event)
    }
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events")
            .field("subscriptions", &self.subscriptions.lock().len())
            .finish()
    }
}

/// The bridge's request router.
pub struct Dispatcher {
    handlers: HashMap<&'static str, Arc<dyn ApiHandler>>,
    /// What this document was granted. `None` for a document with no manifest.
    granted: Option<Permissions>,
    transport: Arc<dyn Transport>,
    streams: Mutex<HashMap<RequestId, OpenStream>>,
    subscriptions: Arc<Mutex<HashMap<RequestId, String>>>,
    headless: bool,
}

impl Dispatcher {
    pub fn new(
        transport: Arc<dyn Transport>,
        granted: Option<Permissions>,
        headless: bool,
    ) -> Self {
        Self {
            handlers: HashMap::new(),
            granted,
            transport,
            streams: Mutex::new(HashMap::new()),
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
            headless,
        }
    }

    /// Register a native module. Registering does not grant it — [`is_granted`](Self::is_granted)
    /// still decides whether the page can reach it.
    pub fn register(&mut self, handler: Arc<dyn ApiHandler>) {
        self.handlers.insert(handler.name(), handler);
    }

    pub fn transport(&self) -> Arc<dyn Transport> {
        Arc::clone(&self.transport)
    }

    /// A handle modules use to push events to the page.
    pub fn events(&self) -> Events {
        Events {
            transport: Arc::clone(&self.transport),
            subscriptions: Arc::clone(&self.subscriptions),
        }
    }

    /// Whether the manifest granted this module.
    pub fn is_granted(&self, module: &str) -> bool {
        if module == "stdio" {
            return self.headless;
        }
        if AMBIENT_MODULES.contains(&module) {
            return true;
        }
        self.granted
            .as_ref()
            .is_some_and(|p| p.granted_modules().contains(module))
    }

    /// Resolve `module.method` to a handler, refusing ungranted modules.
    fn resolve(&self, qualified: &str) -> Result<(Arc<dyn ApiHandler>, String), RpcError> {
        let Some((module, method)) = split_method(qualified) else {
            return Err(RpcError::invalid_params(format!(
                "method must be `module.method`, got `{qualified}`"
            )));
        };

        if !self.is_granted(module) {
            // Deliberately does not distinguish "not granted" from "no such module": a page should
            // not be able to enumerate the runtime's capabilities by probing for error codes.
            return Err(RpcError::denied(format!(
                "`{module}` was not granted to this document"
            )));
        }

        let handler = self
            .handlers
            .get(module)
            .cloned()
            .ok_or_else(|| RpcError::not_found(qualified))?;

        Ok((handler, method.to_string()))
    }

    /// Handle one message from the page.
    pub async fn handle(self: &Arc<Self>, message: ClientMessage) {
        match message {
            ClientMessage::Invoke { id, method, params } => {
                // The shim posts `runtime.ready` with id 0 as its last act; there is no promise
                // waiting on it, so replying would be noise.
                let response = match self.resolve(&method) {
                    Ok((handler, name)) => handler.invoke(&name, params).await,
                    Err(error) => Err(error),
                };
                if id == 0 {
                    if let Err(error) = response {
                        tracing::warn!(%method, %error, "ambient call failed");
                    }
                    return;
                }
                self.transport.send(match response {
                    Ok(value) => HostMessage::Result { id, value },
                    Err(error) => HostMessage::Error { id, error },
                });
            }

            ClientMessage::StreamStart { id, method, params } => {
                let opened = match self.resolve(&method) {
                    Ok((handler, name)) => handler.open_stream(&name, params).await,
                    Err(error) => Err(error),
                };

                match opened {
                    Ok(stream) => self.clone().pump(id, stream),
                    Err(error) => self.transport.send(HostMessage::StreamError { id, error }),
                }
            }

            ClientMessage::StreamCancel { id } => {
                if let Some(open) = self.streams.lock().remove(&id) {
                    open.cancelled.store(true, Ordering::SeqCst);
                }
            }

            ClientMessage::Subscribe { id, event } => {
                // Events are namespaced `module:event`, so the module gate applies to them too.
                if let Some((module, _)) = event.split_once(':')
                    && !self.is_granted(module)
                {
                    tracing::debug!(%event, "refusing subscription to ungranted module");
                    return;
                }
                if let Some((module, name)) = event.split_once(':')
                    && let Some(handler) = self.handlers.get(module)
                    && let Err(error) = handler.subscribe(name)
                {
                    tracing::warn!(%event, %error, "subscribe failed");
                    return;
                }
                self.subscriptions.lock().insert(id, event);
            }

            ClientMessage::Unsubscribe { id } => {
                let event = self.subscriptions.lock().remove(&id);
                if let Some(event) = event
                    && let Some((module, name)) = event.split_once(':')
                    && let Some(handler) = self.handlers.get(module)
                {
                    handler.unsubscribe(name);
                }
            }
        }
    }

    /// Drive an open stream, forwarding chunks to the page until it ends or is cancelled.
    fn pump(self: Arc<Self>, id: RequestId, mut stream: ValueStream) {
        let cancelled = Arc::new(AtomicBool::new(false));
        self.streams.lock().insert(
            id,
            OpenStream {
                cancelled: Arc::clone(&cancelled),
            },
        );

        tokio::spawn(async move {
            while let Some(item) = stream.next().await {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }
                match item {
                    Ok(value) => self.transport.send(HostMessage::Chunk { id, value }),
                    Err(error) => {
                        self.transport.send(HostMessage::StreamError { id, error });
                        self.streams.lock().remove(&id);
                        return;
                    }
                }
            }

            // A cancelled stream needs no terminator: the page already stopped listening, and
            // sending one would race with a new stream reusing the id.
            if !cancelled.load(Ordering::SeqCst) {
                self.transport.send(HostMessage::End { id });
            }
            self.streams.lock().remove(&id);
        });
    }

    /// Emit an event to the page, if anything is listening for it.
    pub fn emit(&self, event: &str, payload: Value) {
        self.events().emit(event, payload);
    }

    /// Parse and handle a raw IPC string from the page.
    pub async fn handle_raw(self: &Arc<Self>, raw: &str) {
        match serde_json::from_str::<ClientMessage>(raw) {
            Ok(message) => self.handle(message).await,
            Err(e) => {
                tracing::warn!(error = %e, "malformed message from page");
                self.transport.send(HostMessage::Error {
                    id: 0,
                    error: RpcError::new(ErrorCode::InvalidParams, e.to_string()),
                });
            }
        }
    }
}
