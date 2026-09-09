//! Headless mode (PRD §9.4).
//!
//! ```sh
//! cat access.log | htmlapp report.hta --headless --format json > summary.json
//! ```
//!
//! The document runs with no surface, `htmlapp.stdin` and `htmlapp.stdout` are available, and the
//! process exits when the page calls `htmlapp.exit(code)`. This is what makes a `.hta` a legitimate
//! participant in a shell pipeline rather than something you can only double-click.

use std::sync::Arc;

use htmlapp_bridge::Dispatcher;
use htmlapp_engine::{EngineCallbacks, EngineEvent, wry_backend};

use crate::host::HeadlessHost;
use crate::session::{Result, Session, SessionError};
use crate::transport::QueueTransport;

/// Run a document with no surface, returning its exit code.
pub fn run(session: Session, runtime: &tokio::runtime::Runtime) -> Result<i32> {
    // A headless document still runs page script, so a pending consent decision is still a
    // decision. There is no window to attach a sheet to, so the safe default applies (§11.2 rule 1)
    // and the user is told how to grant it deliberately.
    let session = if session.needs_prompt() {
        eprintln!(
            "htmlapp: {} requests permissions but there is no window to ask in; \
             running without them.\n\
             Run it once with a window, or `htmlapp permissions allow <file>`, to grant them.",
            session.document.manifest.display_name()
        );
        let mut session = session;
        session.granted = None;
        session
    } else {
        session
    };

    wry_backend::init_toolkit()?;

    let transport = QueueTransport::new();
    let mut dispatcher = Dispatcher::new(
        Arc::clone(&transport) as Arc<dyn htmlapp_bridge::Transport>,
        session.granted.clone(),
        true,
    );

    let registered = session
        .register_modules(&mut dispatcher, Arc::new(HeadlessHost))
        .map_err(|e| SessionError::Refused(e.to_string()))?;
    let stdio = registered.stdio.clone();
    let dispatcher = Arc::new(dispatcher);

    // Messages from the page arrive on the GTK thread and are handled on the Tokio runtime.
    let handle = runtime.handle().clone();
    let for_ipc = Arc::clone(&dispatcher);
    let callbacks = EngineCallbacks {
        on_event: Arc::new(move |event| {
            if let EngineEvent::Ipc(raw) = event {
                let dispatcher = Arc::clone(&for_ipc);
                handle.spawn(async move { dispatcher.handle_raw(&raw).await });
            }
        }),
    };

    let engine = wry_backend::WryEngine::new_headless(session.engine_config(), callbacks)?;

    // The GTK loop is pumped here rather than by `gtk::main()` so the outbound queue can be
    // flushed between iterations and the exit condition checked.
    loop {
        wry_backend::pump_events();
        transport.flush(&engine);

        if let Some(stdio) = &stdio
            && let Some(code) = stdio.requested_exit()
        {
            // One more pass so anything the page wrote just before exiting still lands.
            wry_backend::pump_events();
            transport.flush(&engine);
            registered.process.kill_all();
            return Ok(code);
        }

        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}
