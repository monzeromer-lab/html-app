//! The native API modules (PRD §9.3).
//!
//! `htmlapp-caps` decides what a document *may* do. This crate is where that decision is enforced,
//! once per call, at the point the capability is actually exercised. Every module holds an
//! [`ApiContext`] and every path, program, origin, database, and bus name goes through it.
//!
//! Modules are registered by [`register_all`] according to what the manifest granted. A module that
//! was not granted is never registered and never injected, so §9.3's promise that "the property
//! does not exist" holds at both ends of the bridge.

#![forbid(unsafe_code)]

pub mod clipboard;
pub mod context;
pub mod dbus;
pub mod dialog;
pub mod fs;
pub mod host;
pub mod http;
pub mod notify;
pub mod os;
pub mod params;
pub mod process;
pub mod runtime;
pub mod shell;
pub mod sql;
pub mod stdio;
pub mod store;

use std::sync::Arc;

use htmlapp_bridge::{Dispatcher, RpcError};
use htmlapp_caps::Permissions;

pub use context::{ApiContext, Ctx};
pub use host::{HostBridge, HostModule, NullHost};
pub use process::ProcessModule;
pub use runtime::RuntimeModule;
pub use stdio::StdioModule;

/// An unguessable token, used for `blob:` URLs.
///
/// Sourced from the kernel rather than a counter or the clock: a blob URL is a capability, and a
/// predictable one could be constructed by page script that was never granted the underlying path.
///
/// Note the `read_exact` — `/dev/urandom` is an endless stream, so anything that reads to EOF
/// never returns.
pub fn random_token() -> String {
    use std::io::Read as _;

    let mut bytes = [0u8; 16];
    let filled = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok();

    if !filled {
        // Weaker, but still not derivable from the path, and only reachable if the kernel's
        // entropy source is unavailable — at which point there are larger problems.
        use std::hash::{BuildHasher, Hasher};
        tracing::warn!("could not read /dev/urandom; falling back to a hashed time source");
        for chunk in bytes.chunks_mut(8) {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            );
            let seed = hasher.finish().to_le_bytes();
            chunk.copy_from_slice(&seed[..chunk.len()]);
        }
    }

    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the runtime hands over when wiring up a document's modules.
pub struct Registration {
    pub app_id: String,
    pub app_name: String,
    pub permissions: Option<Permissions>,
    /// The shell, for modules only it can implement.
    pub host: Arc<dyn HostBridge>,
    /// §9.4: registers `stdio` and makes `htmlapp.exit` available.
    pub headless: bool,
    /// Shared with the engine's origin resolver so `fs.blob` URLs can be served.
    pub blobs: htmlapp_bridge::BlobStore,
}

/// Everything the runtime needs to keep a handle on after registration.
pub struct Registered {
    pub ctx: Ctx,
    pub runtime: Arc<RuntimeModule>,
    pub process: Arc<ProcessModule>,
    pub fs: Arc<fs::FsModule>,
    /// `Some` only in headless mode.
    pub stdio: Option<Arc<StdioModule>>,
}

/// Register exactly the modules the manifest granted.
///
/// The gate is applied twice on purpose: once here, so an ungranted module has no handler at all,
/// and again in the dispatcher, which refuses a module the manifest did not grant even if one were
/// somehow registered. Neither check is redundant — this one bounds what exists, the other bounds
/// what is reachable.
pub fn register_all(
    dispatcher: &mut Dispatcher,
    registration: Registration,
) -> Result<Registered, RpcError> {
    let permissions = registration.permissions.clone().unwrap_or_default();
    let granted = permissions.granted_modules();
    let ctx: Ctx = Arc::new(ApiContext::new(&registration.app_id, permissions)?);

    // Ambient: carries no capability, so it is registered whatever the manifest said.
    let runtime_module = Arc::new(RuntimeModule::new(
        env!("CARGO_PKG_VERSION"),
        granted.iter().map(|m| m.to_string()).collect(),
    ));
    dispatcher.register(Arc::clone(&runtime_module) as Arc<dyn htmlapp_bridge::ApiHandler>);

    let fs_module = Arc::new(fs::FsModule::new(Arc::clone(&ctx), registration.blobs.clone()));
    let process_module = Arc::new(ProcessModule::new(Arc::clone(&ctx)));

    if granted.contains("fs") {
        dispatcher.register(Arc::clone(&fs_module) as Arc<dyn htmlapp_bridge::ApiHandler>);
    }
    if granted.contains("process") {
        dispatcher.register(Arc::clone(&process_module) as Arc<dyn htmlapp_bridge::ApiHandler>);
    }
    if granted.contains("dialog") {
        dispatcher.register(Arc::new(dialog::DialogModule::new(
            Arc::clone(&ctx),
            Arc::clone(&registration.host),
        )));
    }
    if granted.contains("http") {
        dispatcher.register(Arc::new(http::HttpModule::new(Arc::clone(&ctx))));
    }
    if granted.contains("sql") {
        dispatcher.register(Arc::new(sql::SqlModule::new(Arc::clone(&ctx))));
    }
    if granted.contains("store") {
        dispatcher.register(Arc::new(store::StoreModule::new(Arc::clone(&ctx))));
    }
    if granted.contains("notify") {
        dispatcher.register(Arc::new(notify::NotifyModule::new(
            Arc::clone(&ctx),
            registration.app_name.clone(),
        )));
    }
    if granted.contains("dbus") {
        dispatcher.register(Arc::new(dbus::DbusModule::new(Arc::clone(&ctx))));
    }
    if granted.contains("clipboard") {
        dispatcher.register(Arc::new(clipboard::ClipboardModule::new(Arc::clone(&ctx))));
    }
    if granted.contains("os") {
        dispatcher.register(Arc::new(os::OsModule::new(Arc::clone(&ctx))));
    }
    if granted.contains("shell") {
        dispatcher.register(Arc::new(shell::ShellModule::new(Arc::clone(&ctx))));
    }

    // Modules the shell owns. `view` is ambient — a document places native views in its own layout
    // and needs no permission to address what it placed there itself.
    for module in ["window", "layer", "menu", "palette", "tray", "shortcut", "dnd", "portal"] {
        if granted.contains(module) {
            dispatcher.register(Arc::new(HostModule::new(
                leak_name(module),
                Arc::clone(&registration.host),
            )));
        }
    }
    dispatcher.register(Arc::new(HostModule::new(
        "view",
        Arc::clone(&registration.host),
    )));

    let stdio = if registration.headless {
        let module = Arc::new(StdioModule::new());
        dispatcher.register(Arc::clone(&module) as Arc<dyn htmlapp_bridge::ApiHandler>);
        Some(module)
    } else {
        None
    };

    Ok(Registered {
        ctx,
        runtime: runtime_module,
        process: process_module,
        fs: fs_module,
        stdio,
    })
}

/// `ApiHandler::name` returns `&'static str`, and the module list above is static, so this only
/// ever hands back one of a fixed, known set of names.
fn leak_name(name: &str) -> &'static str {
    match name {
        "window" => "window",
        "layer" => "layer",
        "menu" => "menu",
        "palette" => "palette",
        "tray" => "tray",
        "shortcut" => "shortcut",
        "dnd" => "dnd",
        "portal" => "portal",
        other => unreachable!("unexpected host module {other}"),
    }
}
