//! Window lifecycle and app orchestration (docs/architecture.md).
//!
//! This crate is where the layers meet: it loads a document through `htmlapp-caps`, settles its
//! consent, brings up an engine from `htmlapp-engine`, wires the `htmlapp-bridge` dispatcher to the
//! `htmlapp-api` modules the manifest granted, and hosts the whole thing in a `htmlapp-shell`
//! window. Nothing below it knows about any of the others.

#![forbid(unsafe_code)]

pub mod app;
pub mod config;
pub mod fetcher;
pub mod headless;
pub mod host;
#[cfg(feature = "layer-shell")]
pub mod layer;
pub mod sandbox;
pub mod session;
pub mod single_instance;
pub mod transport;

pub use config::Config;
pub use host::{HeadlessHost, HostCommand, HostState, RuntimeHost, ViewState};
pub use session::{ConsentOutcome, Session, SessionError, SessionOptions, VERSION};
