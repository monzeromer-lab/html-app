//! The bridge between a document's JavaScript and the host's native capabilities (docs/bridge.md).
//!
//! Three message shapes rather than one — request/response, stream, and event — because a
//! single request/response shape forces every bulk payload into one base64 blob, and tailing a
//! log or reading a 2 GB file falls apart under that. For genuinely large transfers the host mints
//! a `blob:` URL the page fetches directly, so the bytes never pass through JSON at all.
//!
//! The catalog in [`catalog`] is the single source of truth for the API surface: both the injected
//! JS shim and the TypeScript declarations emitted by `htmlapp types` are generated from it, so
//! The promise of editor completion without a build step stays honest.

#![forbid(unsafe_code)]

pub mod blob;
pub mod catalog;
pub mod dispatch;
pub mod protocol;
pub mod shim;
pub mod typescript;

pub use blob::BlobStore;
pub use catalog::{ApiEvent, ApiMethod, ApiModule, MethodKind, Tier};
pub use dispatch::{ApiHandler, BoxFuture, Dispatcher, Events, Transport, ValueStream};
pub use protocol::{ClientMessage, ErrorCode, HostMessage, RequestId, RpcError};
pub use shim::{ShimConfig, dispatch_script, render as render_shim};
pub use typescript::emit as emit_typescript;
