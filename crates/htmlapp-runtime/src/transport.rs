//! Getting host→page messages onto the thread that can deliver them.
//!
//! `evaluate_script` has to run on the thread that owns the webview — the GTK main thread, which is
//! also GPUI's main thread. The bridge, meanwhile, answers calls from Tokio worker threads. So
//! outbound messages are queued here and drained by the frame loop, which is the one place that is
//! guaranteed to be on the right thread.

use std::collections::VecDeque;
use std::sync::Arc;

use htmlapp_bridge::{HostMessage, Transport};
use htmlapp_engine::WebEngine;
use parking_lot::Mutex;

/// A thread-safe outbox drained on the main thread.
#[derive(Default)]
pub struct QueueTransport {
    queue: Mutex<VecDeque<HostMessage>>,
}

impl QueueTransport {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Deliver everything queued so far. Must be called on the engine's own thread.
    ///
    /// The queue is swapped out under the lock rather than drained while holding it, so a handler
    /// that queues another message from inside `evaluate` cannot deadlock.
    pub fn flush(&self, engine: &dyn WebEngine) {
        let pending: Vec<HostMessage> = {
            let mut queue = self.queue.lock();
            if queue.is_empty() {
                return;
            }
            queue.drain(..).collect()
        };

        for message in pending {
            let script = htmlapp_bridge::dispatch_script(&message.to_json());
            if let Err(error) = engine.evaluate(&script) {
                tracing::warn!(%error, "could not deliver a message to the page");
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }
}

impl Transport for QueueTransport {
    fn send(&self, message: HostMessage) {
        self.queue.lock().push_back(message);
    }
}
