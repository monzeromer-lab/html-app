//! Finding the host window's X11 id.
//!
//! # Why this exists
//!
//! `wry`'s `build_as_child` needs the parent window's `RawWindowHandle`. GPUI creates a perfectly
//! ordinary X11 window — but `gpui` 0.2.2 does not hand it out:
//!
//! ```ignore
//! impl rwh::HasWindowHandle for X11Window {
//!     fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
//!         unimplemented!()
//!     }
//! }
//! ```
//!
//! Calling it panics, and the window id it wraps is `pub(crate)`. So the id is recovered from the X
//! server instead: the window is already there and already labelled with this process's pid, which
//! is exactly what `_NET_WM_PID` is for.
//!
//! This is a workaround for a gap in one dependency version, not a design choice. Implementing
//! `HasWindowHandle` for `X11Window` upstream would delete this file, and the offscreen backend in
//! PRD §6.3 would delete the need for it entirely.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, ConnectionExt as _};

/// A reusable X11 connection for locating this process's window.
///
/// Kept as a struct rather than a one-shot function so the caller can retry across frames instead
/// of blocking the UI thread in a sleep loop. That matters: the window is only guaranteed to appear
/// in the client list once the compositor has processed its map request, and the map request is
/// only processed while the host's event loop keeps running.
pub struct WindowFinder {
    connection: x11rb::rust_connection::RustConnection,
    root: xproto::Window,
    net_wm_pid: xproto::Atom,
    net_client_list: xproto::Atom,
    our_pid: u32,
}

impl WindowFinder {
    pub fn new() -> Option<Self> {
        let (connection, screen_number) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen_number)?.root;
        let net_wm_pid = intern(&connection, b"_NET_WM_PID")?;
        let net_client_list = intern(&connection, b"_NET_CLIENT_LIST")?;

        Some(Self {
            connection,
            root,
            net_wm_pid,
            net_client_list,
            our_pid: std::process::id(),
        })
    }

    /// One non-blocking pass. Returns `None` if the window is not visible to the X server yet.
    ///
    /// A document runs in its own process (§7.4) and has exactly one window, so matching on pid is
    /// unambiguous. `expected_title` only disambiguates if that ever stops being true.
    pub fn try_find(&self, expected_title: Option<&str>) -> Option<u32> {
        // The window manager's client list is the cheap path and covers every mapped toplevel.
        let mut candidates =
            client_list(&self.connection, self.root, self.net_client_list).unwrap_or_default();
        if candidates.is_empty() {
            // No WM, or one that does not maintain the list: walk the root's children instead.
            candidates = children(&self.connection, self.root).unwrap_or_default();
        }

        let mut matches: Vec<u32> = candidates
            .into_iter()
            .filter(|window| {
                pid_of(&self.connection, *window, self.net_wm_pid) == Some(self.our_pid)
            })
            .collect();

        if matches.len() > 1
            && let Some(title) = expected_title
        {
            let titled: Vec<u32> = matches
                .iter()
                .copied()
                .filter(|window| title_of(&self.connection, *window).as_deref() == Some(title))
                .collect();
            if !titled.is_empty() {
                matches = titled;
            }
        }

        // The most recently created window has the highest id, which is the one just opened.
        matches.into_iter().max().inspect(|window| {
            tracing::debug!(window, "found the host X11 window");
        })
    }
}

fn intern(connection: &impl Connection, name: &[u8]) -> Option<xproto::Atom> {
    connection
        .intern_atom(true, name)
        .ok()?
        .reply()
        .ok()
        .map(|reply| reply.atom)
        .filter(|atom| *atom != 0)
}

fn client_list(
    connection: &impl Connection,
    root: xproto::Window,
    atom: xproto::Atom,
) -> Option<Vec<u32>> {
    let reply = connection
        .get_property(false, root, atom, xproto::AtomEnum::WINDOW, 0, u32::MAX)
        .ok()?
        .reply()
        .ok()?;
    Some(reply.value32()?.collect())
}

fn children(connection: &impl Connection, root: xproto::Window) -> Option<Vec<u32>> {
    let reply = connection.query_tree(root).ok()?.reply().ok()?;
    Some(reply.children)
}

fn pid_of(
    connection: &impl Connection,
    window: xproto::Window,
    net_wm_pid: xproto::Atom,
) -> Option<u32> {
    let reply = connection
        .get_property(
            false,
            window,
            net_wm_pid,
            xproto::AtomEnum::CARDINAL,
            0,
            1,
        )
        .ok()?
        .reply()
        .ok()?;
    reply.value32()?.next()
}

fn title_of(connection: &impl Connection, window: xproto::Window) -> Option<String> {
    let reply = connection
        .get_property(
            false,
            window,
            xproto::AtomEnum::WM_NAME,
            xproto::AtomEnum::STRING,
            0,
            256,
        )
        .ok()?
        .reply()
        .ok()?;
    String::from_utf8(reply.value).ok()
}
