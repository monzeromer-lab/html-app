//! Running a document as a layer-shell surface (docs/document-format.md).
//!
//! No GPUI: the page owns the whole surface. See [`htmlapp_engine::layer_backend`] for why that is
//! The right shape for a bar rather than a compromise.

use std::sync::Arc;

use htmlapp_bridge::Dispatcher;
use htmlapp_caps::WindowMode;
use htmlapp_engine::layer_backend::LayerEngine;
use htmlapp_engine::{EngineCallbacks, EngineEvent, wry_backend};

use crate::host::{HostState, RuntimeHost};
use crate::session::{Result, Session, SessionError};
use crate::transport::QueueTransport;

/// Run a document as a bar, dock, widget, overlay, or lock surface.
pub fn run(session: Session, runtime: tokio::runtime::Runtime) -> Result<i32> {
    if !LayerEngine::is_available() {
        return Err(SessionError::Refused(
            "this compositor does not implement wlr-layer-shell".into(),
        ));
    }

    session.record_recent();

    let state = HostState::new();
    let transport = QueueTransport::new();

    let mut dispatcher = Dispatcher::new(
        Arc::clone(&transport) as Arc<dyn htmlapp_bridge::Transport>,
        session.granted.clone(),
        false,
    );
    // `has_window` is false: there is no GPUI scene, so `menu`, `palette`, and the native views
    // report unsupported rather than silently doing nothing.
    let host = Arc::new(RuntimeHost::new(Arc::clone(&state), false));
    let registered = session
        .register_modules(&mut dispatcher, Arc::clone(&host) as _)
        .map_err(|e| SessionError::Refused(e.to_string()))?;
    host.set_context(Arc::clone(&registered.ctx));
    let dispatcher = Arc::new(dispatcher);

    let handle = runtime.handle().clone();
    let for_ipc = Arc::clone(&dispatcher);
    let callbacks = EngineCallbacks {
        on_event: Arc::new(move |event| match event {
            EngineEvent::Ipc(raw) => {
                let dispatcher = Arc::clone(&for_ipc);
                handle.spawn(async move { dispatcher.handle_raw(&raw).await });
            }
            EngineEvent::NavigationBlocked(url) => {
                tracing::warn!(%url, "navigation blocked by the manifest");
            }
            _ => {}
        }),
    };

    let engine = LayerEngine::new(
        session.engine_config(),
        callbacks,
        &session.document.manifest.window,
        session.window_mode(),
    )?;

    if session.window_mode() == WindowMode::Lock {
        tracing::warn!(
            "`mode: \"lock\"` uses an exclusive-focus overlay, not ext-session-lock. It is not a \
             security boundary: a compositor crash or a VT switch exposes the session."
        );
    }

    // GTK owns the loop here, pumped directly so the outbound bridge queue can be flushed between
    // iterations — the same shape as headless mode, for the same reason.
    loop {
        wry_backend::pump_events();
        transport.flush(&engine);

        for command in state.drain_commands() {
            tracing::debug!(?command, "host command is not available on a layer surface");
        }

        std::thread::sleep(std::time::Duration::from_millis(8));
        if registered.runtime.is_ready() && state.has_overlay() {
            // Overlays need a GPUI scene; nothing to do but note it.
            state.close_overlay(serde_json::Value::Null);
        }
    }
}
