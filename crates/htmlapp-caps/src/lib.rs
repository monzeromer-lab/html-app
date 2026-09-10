//! Manifest parsing, the permission model, and the consent store for HTML App.
//!
//! This crate is the whole of the security boundary's *policy* half: it decides what a document is
//! allowed to do. `htmlapp-api` is the *mechanism* half that enforces those decisions at each call
//! site. Nothing here touches an engine, a window, or a runtime, so the policy can be tested — and
//! audited — without starting a browser.
//!
//! The model, in one paragraph (docs/security.md): a document with no manifest gets no native APIs, and
//! that is silent rather than an error. Permissions are declared in the document and enforced in
//! Rust, so nothing in JS can widen them. Consent is pinned to `sha256(file)`, so editing the file
//! re-prompts with a diff of what changed. Paths are canonicalised before they are checked, so a
//! symlink cannot escape the granted globs.

#![forbid(unsafe_code)]

pub mod consent;
pub mod document;
pub mod error;
pub mod manifest;
pub mod permissions;
pub mod recents;

pub use consent::{ConsentDecision, ConsentRecord, ConsentStore, PermissionDiff};
pub use document::{Document, extract_manifest_json, hash_bytes};
pub use error::{CapsError, Result};
pub use manifest::{
    Anchor, AssetPolicy, Background, ImportSpec, KeyboardInteractivity, Layer, Manifest, Margin,
    Titlebar, WindowMode, WindowSpec,
};
pub use permissions::{
    ClipboardAccess, DbusPermission, DeniedPath, FsPermission, NetPermission, PathScope,
    PermissionDescription, Permissions, PortalCapability, ProcessPermission, Risk,
    SocketPermission, SqlPermission,
};
pub use recents::{MAX_RECENTS, RecentEntry, Recents};

/// The version of the runtime, surfaced to the page as `htmlapp.version` (docs/bridge.md).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
