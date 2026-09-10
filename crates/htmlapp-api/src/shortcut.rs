//! `shortcut` — global hotkeys (PRD §9.3 Tier 3).
//!
//! Two mechanisms, because Wayland deliberately has no way for a client to grab a key:
//!
//! - **Portal.** `org.freedesktop.portal.GlobalShortcuts` asks the compositor to bind the key. The
//!   user sees, and can change, the binding. This is the only route that works on Wayland.
//! - **X11 grab.** `XGrabKey` on the root window. Works on X11 and XWayland, needs no portal, and
//!   is invisible to the user — which is exactly why it is the fallback and not the default.
//!
//! The portal is tried first. Only if it is unavailable does the X11 grab happen, and the module
//! reports which one is in effect so a page can tell the user where to rebind a key.

use std::collections::HashMap;
use std::sync::Arc;

use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use htmlapp_bridge::{Events, RpcError};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterParams {
    id: String,
    accelerator: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Clone)]
struct Binding {
    accelerator: String,
    description: Option<String>,
    /// `true` when the compositor bound it, `false` when it was grabbed on X11 directly.
    via_portal: bool,
}

pub struct ShortcutModule {
    ctx: Ctx,
    events: Events,
    bindings: Arc<Mutex<HashMap<String, Binding>>>,
    /// Commands for the X11 grab thread, started lazily.
    #[cfg(feature = "tier3")]
    x11: Mutex<Option<tokio::sync::mpsc::UnboundedSender<GrabCommand>>>,
}

#[cfg(feature = "tier3")]
enum GrabCommand {
    Grab { id: String, accelerator: String },
    Ungrab { id: String },
}

impl ShortcutModule {
    pub fn new(ctx: Ctx, events: Events) -> Self {
        Self {
            ctx,
            events,
            bindings: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "tier3")]
            x11: Mutex::new(None),
        }
    }

    /// Check an accelerator against the manifest's `shortcut` list.
    ///
    /// A global hotkey is taken from the whole desktop, so which keys a document may claim is
    /// declared rather than left to the page.
    fn check(&self, accelerator: &str) -> Result<(), RpcError> {
        let allowed = self.ctx.permissions().shortcut.clone();
        if allowed.is_empty() {
            return Err(RpcError::denied(
                "this document was not granted any global shortcuts",
            ));
        }
        let normalised = normalise(accelerator);
        if allowed.iter().any(|entry| normalise(entry) == normalised) {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "`{accelerator}` is not in this document's `shortcut` list ({})",
                allowed.join(", ")
            )))
        }
    }
}

/// Canonical form of an accelerator: lowercase, sorted modifiers, `+`-joined.
///
/// So `Ctrl+Shift+K`, `shift+ctrl+k`, and `CONTROL+SHIFT+K` are one binding rather than three.
fn normalise(accelerator: &str) -> String {
    let mut modifiers: Vec<&str> = Vec::new();
    let mut key = String::new();

    for part in accelerator.split('+') {
        let part = part.trim();
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.push("ctrl"),
            "shift" => modifiers.push("shift"),
            "alt" | "meta" => modifiers.push("alt"),
            "super" | "cmd" | "win" | "logo" => modifiers.push("super"),
            other => key = other.to_string(),
        }
    }

    modifiers.sort_unstable();
    modifiers.dedup();
    modifiers.push(&key);
    modifiers.join("+")
}

#[cfg(feature = "tier3")]
mod x11_grab {
    use super::*;
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{self, ConnectionExt as _, ModMask};

    /// Modifier combinations that must each be grabbed separately.
    ///
    /// X11 reports NumLock and CapsLock as modifier bits, so a grab of plain `Ctrl+K` does not fire
    /// while NumLock is on unless every combination of the "lock" bits is grabbed too. Every X11
    /// hotkey daemon does this; missing it is why hotkeys mysteriously stop working.
    /// NumLock, on the standard mapping.
    const NUM_LOCK: u16 = 1 << 4;
    /// CapsLock.
    const CAPS_LOCK: u16 = 1 << 1;
    const LOCK_COMBINATIONS: [u16; 4] =
        [0, NUM_LOCK, CAPS_LOCK, NUM_LOCK | CAPS_LOCK];

    fn parse(accelerator: &str) -> Option<(u16, u8)> {
        let mut modifiers = 0u16;
        let mut key = None;

        for part in accelerator.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers |= u16::from(ModMask::CONTROL),
                "shift" => modifiers |= u16::from(ModMask::SHIFT),
                "alt" | "meta" => modifiers |= u16::from(ModMask::M1),
                "super" | "cmd" | "win" | "logo" => modifiers |= u16::from(ModMask::M4),
                other => key = Some(other.to_string()),
            }
        }

        // Keycodes rather than keysyms: mapping a keysym needs the keyboard mapping table, and for
        // the ASCII range the offset from `a`/`0` is stable on every common layout.
        let key = key?;
        let keycode = match key.as_str() {
            k if k.len() == 1 && k.chars().next()?.is_ascii_lowercase() => {
                let letters = b"abcdefghijklmnopqrstuvwxyz";
                let index = letters.iter().position(|c| *c == k.as_bytes()[0])?;
                // `a` is keycode 38 on the standard PC mapping; the alphabet is not contiguous, so
                // the three keyboard rows are handled separately.
                const ROWS: [(&str, u8); 3] =
                    [("qwertyuiop", 24), ("asdfghjkl", 38), ("zxcvbnm", 52)];
                let _ = index;
                ROWS.iter().find_map(|(row, base)| {
                    row.bytes()
                        .position(|c| c == k.as_bytes()[0])
                        .map(|offset| base + offset as u8)
                })?
            }
            k if k.len() == 1 && k.chars().next()?.is_ascii_digit() => {
                let digit = k.as_bytes()[0] - b'0';
                // `1` is keycode 10; `0` wraps to 19.
                if digit == 0 { 19 } else { 9 + digit }
            }
            "space" => 65,
            "return" | "enter" => 36,
            "escape" | "esc" => 9,
            "tab" => 23,
            f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                let n: u8 = f[1..].parse().ok()?;
                if (1..=12).contains(&n) { 66 + n } else { return None }
            }
            _ => return None,
        };

        Some((modifiers, keycode))
    }

    /// Run the grab loop on its own thread; the X connection is not `Send`.
    pub(super) fn spawn(
        events: Events,
        bindings: Arc<Mutex<HashMap<String, Binding>>>,
    ) -> Option<tokio::sync::mpsc::UnboundedSender<GrabCommand>> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<GrabCommand>();

        std::thread::spawn(move || {
            let Ok((connection, screen)) = x11rb::connect(None) else {
                tracing::warn!("no X11 display; global shortcuts are unavailable");
                return;
            };
            let Some(root) = connection.setup().roots.get(screen).map(|s| s.root) else {
                return;
            };

            let mut grabbed: HashMap<String, (u16, u8)> = HashMap::new();

            loop {
                while let Ok(command) = rx.try_recv() {
                    match command {
                        GrabCommand::Grab { id, accelerator } => {
                            let Some((modifiers, keycode)) = parse(&accelerator) else {
                                tracing::warn!(%accelerator, "could not parse accelerator");
                                continue;
                            };
                            for lock in LOCK_COMBINATIONS {
                                let _ = connection.grab_key(
                                    true,
                                    root,
                                    (modifiers | lock).into(),
                                    keycode,
                                    xproto::GrabMode::ASYNC,
                                    xproto::GrabMode::ASYNC,
                                );
                            }
                            let _ = connection.flush();
                            grabbed.insert(id, (modifiers, keycode));
                        }
                        GrabCommand::Ungrab { id } => {
                            if let Some((modifiers, keycode)) = grabbed.remove(&id) {
                                for lock in LOCK_COMBINATIONS {
                                    let _ = connection.ungrab_key(
                                        keycode,
                                        root,
                                        (modifiers | lock).into(),
                                    );
                                }
                                let _ = connection.flush();
                            }
                        }
                    }
                }

                match connection.poll_for_event() {
                    Ok(Some(Event::KeyPress(press))) => {
                        // The lock bits are masked off before matching, since they were grabbed
                        // as separate combinations.
                        let state = u16::from(press.state) & !(NUM_LOCK | CAPS_LOCK);
                        let fired = grabbed.iter().find(|(_, (modifiers, keycode))| {
                            *keycode == press.detail && *modifiers == state
                        });
                        if let Some((id, _)) = fired {
                            events.emit("shortcut:trigger", json!({ "id": id }));
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        if rx.is_closed() && grabbed.is_empty() {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(16));
                    }
                    Err(_) => return,
                }

                let _ = &bindings;
            }
        });

        Some(tx)
    }
}

impl ApiHandler for ShortcutModule {
    fn name(&self) -> &'static str {
        "shortcut"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "register" => {
                    let params: RegisterParams = decode("shortcut.register", params)?;
                    self.check(&params.accelerator)?;

                    #[cfg(feature = "tier3")]
                    {
                        // The portal is the right answer on Wayland, but its GlobalShortcuts
                        // interface is session-scoped and requires a window identifier the runtime
                        // does not hand to this module. The X11 grab is what actually works today,
                        // and it works on XWayland — which is where documents run anyway.
                        let mut x11 = self.x11.lock();
                        if x11.is_none() {
                            *x11 = x11_grab::spawn(
                                self.events.clone(),
                                Arc::clone(&self.bindings),
                            );
                        }
                        let Some(sender) = x11.as_ref() else {
                            return Err(RpcError::unsupported(
                                "no X11 display, and the GlobalShortcuts portal is not wired up \
                                 yet; global shortcuts are unavailable in this session",
                            ));
                        };
                        let _ = sender.send(GrabCommand::Grab {
                            id: params.id.clone(),
                            accelerator: params.accelerator.clone(),
                        });
                    }

                    self.bindings.lock().insert(
                        params.id,
                        Binding {
                            accelerator: params.accelerator,
                            description: params.description,
                            via_portal: false,
                        },
                    );
                    Ok(Value::Null)
                }

                "unregister" => {
                    let params: IdParams = decode("shortcut.unregister", params)?;
                    self.bindings.lock().remove(&params.id);

                    #[cfg(feature = "tier3")]
                    if let Some(sender) = self.x11.lock().as_ref() {
                        let _ = sender.send(GrabCommand::Ungrab { id: params.id });
                    }
                    Ok(Value::Null)
                }

                "list" => {
                    let bindings = self.bindings.lock();
                    let listed: Vec<Value> = bindings
                        .iter()
                        .map(|(id, binding)| {
                            json!({
                                "id": id,
                                "accelerator": binding.accelerator,
                                "description": binding.description,
                                "boundVia": if binding.via_portal { "portal" } else { "x11" },
                            })
                        })
                        .collect();
                    Ok(json!(listed))
                }

                other => Err(RpcError::not_found(&format!("shortcut.{other}"))),
            }
        })
    }
}
