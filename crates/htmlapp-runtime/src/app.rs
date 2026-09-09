//! The GPUI application: launcher, consent, and document windows (PRD §7).

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Application, Bounds, Context, Focusable as _, IntoElement, ParentElement,
    Render, Styled, TitlebarOptions, Window, WindowBounds, WindowKind, WindowOptions, div, point,
    px, size,
};
use htmlapp_bridge::Dispatcher;
use htmlapp_caps::{ConsentStore, Titlebar, WindowMode};
use htmlapp_engine::{EngineCallbacks, EngineEvent, WebEngine, wry_backend};
use htmlapp_shell::consent::{ConsentChoice, ConsentRequest, ConsentSheet};
use htmlapp_shell::launcher::{Diagnostics, Launcher, LauncherDelegate};
use htmlapp_shell::{Theme, WebView};

use crate::host::{HostCommand, HostState, RuntimeHost};
use crate::session::{ConsentOutcome, Result, Session, SessionError};
use crate::transport::QueueTransport;

/// How often the GTK main context is drained while a window is up.
///
/// GPUI's own loop is event-driven and can idle indefinitely, but WebKit needs its context pumped
/// to paint, run timers, and process input. 8 ms keeps the page responsive at roughly 120 Hz
/// without spinning a core when nothing is happening.
const PUMP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(8);

/// Diagnostics for the launcher's strip (§7.2).
///
/// §7.2 wants this "copyable as a single block, so bug reports arrive with it" — so it reports what
/// the *compositor* offers as well as what this build does. "your compositor has no layer-shell"
/// and "this build cannot render into one" are different problems with different fixes.
pub fn diagnostics(engine: Option<&dyn WebEngine>) -> Diagnostics {
    let capabilities = htmlapp_wayland::Capabilities::detect();
    let session = if capabilities.layer_shell {
        format!("{} (layer-shell available)", htmlapp_api::os::session_type())
    } else {
        htmlapp_api::os::session_type().to_string()
    };

    Diagnostics {
        session_type: session,
        compositor: htmlapp_api::os::compositor(),
        engine: engine
            .map(|e| e.engine_version())
            .unwrap_or_else(|| "not started".into()),
        render_path: engine
            .map(|e| e.render_path().as_str().to_string())
            .unwrap_or_else(|| "n/a".into()),
        gpu: None,
        version: crate::session::VERSION.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The launcher (§7.2)
// ---------------------------------------------------------------------------

/// Run the launcher. §7.1: a bare invocation is a normal launch, not a usage error.
pub fn run_launcher(delegate: Arc<dyn LauncherDelegate>) -> Result<()> {
    let diagnostics = diagnostics(None);

    Application::new().run(move |cx: &mut App| {
        htmlapp_shell::launcher::bind_keys(cx);

        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(760.0), px(680.0)),
        };

        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("HTML App".into()),
                    ..Default::default()
                }),
                focus: true,
                ..Default::default()
            },
            |window, cx| {
                let launcher =
                    cx.new(|cx| Launcher::new(delegate.clone(), diagnostics.clone(), cx));
                window.focus(&launcher.focus_handle(cx));
                launcher
            },
        );

        if let Err(error) = opened {
            tracing::error!(%error, "could not open the launcher window");
            cx.quit();
            return;
        }
        cx.activate(true);
    });

    Ok(())
}

/// Open the consent manager (§11.2 rule 6).
pub fn run_permissions_manager() -> Result<()> {
    struct RevealDelegate;
    impl htmlapp_shell::PermissionsDelegate for RevealDelegate {
        fn reveal(&self, path: &std::path::Path) {
            let target = if path.is_dir() {
                path.to_path_buf()
            } else {
                path.parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| path.to_path_buf())
            };
            let _ = std::process::Command::new("xdg-open").arg(target).spawn();
        }
    }

    Application::new().run(|cx: &mut App| {
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(px(720.0), px(560.0)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("Permissions".into()),
                    ..Default::default()
                }),
                focus: true,
                ..Default::default()
            },
            |window, cx| {
                let manager =
                    cx.new(|cx| htmlapp_shell::PermissionsManager::new(Arc::new(RevealDelegate), cx));
                window.focus(&manager.focus_handle(cx));
                manager
            },
        );

        if opened.is_err() {
            cx.quit();
            return;
        }
        cx.activate(true);
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// A document window, with consent resolved first (§11.2 rule 3, §7.1)
// ---------------------------------------------------------------------------

/// The engine, which cannot be built until the host window exists on the X server.
struct PendingEngine {
    config: htmlapp_engine::EngineConfig,
    callbacks: EngineCallbacks,
    title: String,
    finder: Option<htmlapp_engine::x11_window::WindowFinder>,
    /// Frames spent looking. Bounded so a failure surfaces as an error rather than a blank window.
    attempts: u32,
}

/// The root view of a document window: the page, with the shell around it.
struct DocumentRoot {
    webview: Option<WebView>,
    pending: Option<PendingEngine>,
    theme: Theme,
    transport: Arc<QueueTransport>,
    state: Arc<HostState>,
    exit_code: Arc<parking_lot::Mutex<i32>>,
}

/// How many frames to spend looking for the host window before giving up.
///
/// At roughly one attempt per frame this is a couple of seconds — generous for a window that is
/// usually visible to the X server within a frame or two, and short enough that a genuine failure
/// does not leave an empty window sitting there indefinitely.
const MAX_DISCOVERY_FRAMES: u32 = 240;

impl DocumentRoot {
    /// Try to bring the engine up. Runs once per frame until it succeeds or gives up.
    ///
    /// This is deliberately not done inside `open_window`: the window has to be mapped before the X
    /// server will report it, and it is only mapped once the event loop has run.
    fn try_start_engine(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };

        if pending.finder.is_none() {
            pending.finder = htmlapp_engine::x11_window::WindowFinder::new();
            if pending.finder.is_none() {
                tracing::error!("could not open an X11 connection to locate the host window");
                *self.exit_code.lock() = 1;
                self.pending = None;
                cx.quit();
                return;
            }
        }

        pending.attempts += 1;
        let found = pending
            .finder
            .as_ref()
            .and_then(|finder| finder.try_find(Some(&pending.title)));

        let Some(window_id) = found else {
            if pending.attempts >= MAX_DISCOVERY_FRAMES {
                tracing::error!(
                    "gave up looking for this process's X11 window after {} frames; \
                     the page cannot be displayed",
                    pending.attempts
                );
                *self.exit_code.lock() = 1;
                self.pending = None;
                cx.quit();
            }
            return;
        };

        let pending = self.pending.take().expect("checked above");
        let bounds = htmlapp_engine::ViewRect::new(
            0.0,
            0.0,
            pending.config.manifest.window.width as f32,
            pending.config.manifest.window.height as f32,
        );

        match wry_backend::WryEngine::new_in_x11_window(
            window_id,
            pending.config,
            pending.callbacks,
            bounds,
        ) {
            Ok(engine) => {
                tracing::info!(window_id, "engine attached to the host window");
                self.webview = Some(WebView::new(Rc::new(engine) as Rc<dyn WebEngine>));
                cx.notify();
            }
            Err(error) => {
                tracing::error!(%error, "the engine could not start");
                *self.exit_code.lock() = 1;
                cx.quit();
            }
        }
    }

    /// Drain GTK, deliver queued bridge messages, and apply host commands — once per frame.
    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = &window;
        self.try_start_engine(cx);

        wry_backend::pump_events();
        if let Some(webview) = &self.webview {
            self.transport.flush(webview.engine().as_ref());
        }

        for command in self.state.drain_commands() {
            match command {
                HostCommand::SetTitle(title) => window.set_window_title(&title),
                HostCommand::Fullscreen(enabled) => {
                    if enabled != window.is_fullscreen() {
                        window.toggle_fullscreen();
                    }
                }
                HostCommand::Minimize => window.minimize_window(),
                HostCommand::Maximize(_) => window.zoom_window(),
                HostCommand::Close => cx.quit(),
                other => tracing::debug!(?other, "host command is not wired to the window yet"),
            }
        }

        // Keep the loop alive: GPUI idles when nothing changes, and WebKit would then stop
        // painting, stop running timers, and stop seeing input.
        //
        // `Window::request_animation_frame` would be the obvious call, but it resolves the current
        // view through `Window::current_view`, which asserts it is inside a layout or paint phase.
        // This runs from an `on_next_frame` callback, which is outside all of them. Marking the
        // entity dirty schedules the next render directly, which re-arms the callback.
        cx.notify();
    }
}

impl Render for DocumentRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `on_next_frame` hands back only the window and the app, so the entity is captured to
        // get back to `self` on the other side.
        let entity = cx.entity();
        window.on_next_frame(move |window, cx| {
            entity.update(cx, |this, cx| this.tick(window, cx));
        });

        let mut root = div().size_full().bg(self.theme.background);
        if let Some(webview) = &self.webview {
            root = root.child(webview.element());
        }
        root
    }
}

/// Everything needed to bring a document's window up, once consent is settled.
struct DocumentLaunch {
    session: Session,
    runtime_handle: tokio::runtime::Handle,
    state: Arc<HostState>,
    exit_code: Arc<parking_lot::Mutex<i32>>,
    processes: Arc<parking_lot::Mutex<Option<Arc<htmlapp_api::ProcessModule>>>>,
}

impl DocumentLaunch {
    /// Wire the bridge, start the engine, and open the window.
    fn open(&self, cx: &mut App) {
        let session = &self.session;
        let manifest_window = session.document.manifest.window.clone();
        let title = manifest_window
            .title
            .clone()
            .unwrap_or_else(|| session.document.manifest.display_name().to_string());

        let transport = QueueTransport::new();
        let mut dispatcher = Dispatcher::new(
            Arc::clone(&transport) as Arc<dyn htmlapp_bridge::Transport>,
            session.granted.clone(),
            false,
        );

        let registered = match session.register_modules(
            &mut dispatcher,
            Arc::new(RuntimeHost::new(Arc::clone(&self.state), true)),
        ) {
            Ok(registered) => registered,
            Err(error) => {
                tracing::error!(%error, "could not register the document's API modules");
                *self.exit_code.lock() = 1;
                cx.quit();
                return;
            }
        };
        *self.processes.lock() = Some(Arc::clone(&registered.process));

        let dispatcher = Arc::new(dispatcher);
        let engine_config = session.engine_config();
        let handle = self.runtime_handle.clone();
        let state = Arc::clone(&self.state);
        let exit_code = Arc::clone(&self.exit_code);

        let callbacks = EngineCallbacks {
            on_event: {
                let handle = handle.clone();
                let for_ipc = Arc::clone(&dispatcher);
                Arc::new(move |event| match event {
                    EngineEvent::Ipc(raw) => {
                        let dispatcher = Arc::clone(&for_ipc);
                        handle.spawn(async move { dispatcher.handle_raw(&raw).await });
                    }
                    EngineEvent::NavigationBlocked(url) => {
                        tracing::warn!(%url, "navigation blocked by the manifest");
                    }
                    _ => {}
                })
            },
        };

        let pending = PendingEngine {
            config: engine_config,
            callbacks,
            title: title.clone(),
            finder: None,
            attempts: 0,
        };

        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(
                        px(manifest_window.width as f32),
                        px(manifest_window.height as f32),
                    ),
                })),
                titlebar: match manifest_window.titlebar {
                    Titlebar::Native => Some(TitlebarOptions {
                        title: Some(title.clone().into()),
                        ..Default::default()
                    }),
                    Titlebar::None => None,
                },
                focus: true,
                is_resizable: manifest_window.resizable,
                ..Default::default()
            },
            move |_window, cx| {
                cx.new(|_| DocumentRoot {
                    webview: None,
                    pending: Some(pending),
                    theme: Theme::detect(),
                    transport: Arc::clone(&transport),
                    state: Arc::clone(&state),
                    exit_code: Arc::clone(&exit_code),
                })
            },
        );

        if let Err(error) = opened {
            tracing::error!(%error, "could not open the document window");
            *self.exit_code.lock() = 1;
            cx.quit();
            return;
        }
        cx.activate(true);
    }
}

/// Run one document in its own window, prompting for consent first if it needs it.
///
/// Consent and the document share one GPUI application rather than two sequential ones, because the
/// engine cannot be constructed until the answer is known — a document must not run a single line
/// of script before its permissions are settled.
pub fn run_document(session: Session, runtime: tokio::runtime::Runtime) -> Result<i32> {
    // The wry backend attaches to an X11 window, so the display server has to be settled before
    // GPUI opens anything. See `htmlapp_engine::wry_backend` for why.
    if !wry_backend::force_x11_session() {
        return Err(SessionError::Refused(
            "the wry engine backend needs an X11 display (DISPLAY is unset). \
             On a Wayland-only session, install XWayland."
                .into(),
        ));
    }
    wry_backend::init_toolkit()?;

    let exit_code = Arc::new(parking_lot::Mutex::new(0i32));
    let processes: Arc<parking_lot::Mutex<Option<Arc<htmlapp_api::ProcessModule>>>> =
        Arc::new(parking_lot::Mutex::new(None));

    let final_code = Arc::clone(&exit_code);
    let final_processes = Arc::clone(&processes);
    let handle = runtime.handle().clone();

    Application::new().run(move |cx: &mut App| {
        let launch = Rc::new(DocumentLaunch {
            session,
            runtime_handle: handle,
            state: HostState::new(),
            exit_code: Arc::clone(&final_code),
            processes: Arc::clone(&final_processes),
        });

        if !launch.session.needs_prompt() {
            launch.session.record_recent();
            launch.open(cx);
            return;
        }

        let request = consent_request(&launch.session);
        let choice = Arc::new(parking_lot::Mutex::new(None::<ConsentChoice>));
        let recorder = Arc::clone(&choice);

        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(px(560.0), px(620.0)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("Permissions".into()),
                    ..Default::default()
                }),
                focus: true,
                is_resizable: false,
                kind: WindowKind::PopUp,
                ..Default::default()
            },
            move |window, cx| {
                let on_choice: Arc<dyn Fn(ConsentChoice) + Send + Sync> =
                    Arc::new(move |choice| *recorder.lock() = Some(choice));
                let sheet = cx.new(|cx| ConsentSheet::new(request.clone(), on_choice, cx));
                window.focus(&sheet.focus_handle(cx));
                sheet
            },
        );

        let Ok(consent_window) = opened else {
            *final_code.lock() = 1;
            cx.quit();
            return;
        };
        cx.activate(true);

        // The sheet's callback has no access to `cx`, so the answer is picked up here and acted on.
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(PUMP_INTERVAL).await;
                let answer = *choice.lock();
                let Some(answer) = answer else { continue };

                let _ = cx.update(|cx| {
                    let _ = consent_window.update(cx, |_, window, _| window.remove_window());

                    match answer {
                        ConsentChoice::Cancel => {
                            cx.quit();
                        }
                        ConsentChoice::Allow | ConsentChoice::RunPowerless => {
                            let launch = Rc::clone(&launch);
                            // `launch` holds the session by value, so the decision is applied to a
                            // clone that the window then takes ownership of.
                            let mut session = launch.session.clone_for_launch();

                            if answer == ConsentChoice::Allow {
                                match ConsentStore::load_default() {
                                    Ok(mut store) => {
                                        if let Err(error) = session.apply_consent(true, &mut store) {
                                            tracing::error!(%error, "could not record consent");
                                        }
                                    }
                                    Err(error) => tracing::error!(%error, "could not open the consent store"),
                                }
                            } else {
                                // Running once without permissions is not a refusal, so nothing is
                                // recorded: the next run asks again.
                                session.granted = None;
                                session.consent = ConsentOutcome::NotRequired;
                            }

                            session.record_recent();
                            let relaunch = DocumentLaunch {
                                session,
                                runtime_handle: launch.runtime_handle.clone(),
                                state: Arc::clone(&launch.state),
                                exit_code: Arc::clone(&launch.exit_code),
                                processes: Arc::clone(&launch.processes),
                            };
                            relaunch.open(cx);
                        }
                    }
                });
                return;
            }
        })
        .detach();
    });

    // §7.4: a page must not outlive its window by leaving a subprocess running.
    if let Some(processes) = processes.lock().as_ref() {
        processes.kill_all();
    }

    let code = *exit_code.lock();
    Ok(code)
}

fn consent_request(session: &Session) -> ConsentRequest {
    let (diff, previously_seen) = match &session.consent {
        ConsentOutcome::Prompt {
            diff,
            previously_seen,
        } => (Some(diff.clone()), *previously_seen),
        _ => (None, false),
    };

    ConsentRequest {
        app_name: session.document.manifest.display_name().to_string(),
        source: session.document.source.clone(),
        hash: session.document.hash.clone(),
        permissions: session
            .document
            .manifest
            .permissions
            .clone()
            .unwrap_or_default(),
        diff,
        previously_seen,
    }
}

/// Route a document to the right runner for its window mode (§8.3).
pub fn run(session: Session, runtime: tokio::runtime::Runtime) -> Result<i32> {
    match session.window_mode() {
        WindowMode::Headless => crate::headless::run(session, &runtime),
        WindowMode::Window => run_document(session, runtime),
        mode @ (WindowMode::Layer | WindowMode::Lock) => {
            // Say which of the two obstacles is actually in the way, since only one of them is
            // something the user can do anything about.
            let capabilities = htmlapp_wayland::Capabilities::detect();
            let available = match mode {
                WindowMode::Layer => capabilities.layer_shell,
                _ => capabilities.session_lock,
            };

            let detail = if !capabilities.wayland {
                "this is not a Wayland session, and these protocols are Wayland-only"
            } else if !available {
                "this compositor does not offer the protocol it needs"
            } else {
                "this compositor offers the protocol, but the engine backend that ships today \
                 attaches to an X11 window and cannot render into a layer surface. This mode is \
                 waiting on the offscreen backend described in PRD §6.3"
            };

            Err(SessionError::Refused(format!(
                "`{}` windows cannot run here: {detail}.",
                mode.as_str()
            )))
        }
        WindowMode::Tray => Err(SessionError::Refused(
            "`tray` windows are not implemented yet".into(),
        )),
    }
}
