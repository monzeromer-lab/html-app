//! `plugin` — a wasmtime host for third-party extensions (docs/api-reference.md, Tier 4).
//!
//! The API catalog calls this "the recommended alternative to `ffi` for anything redistributable", and the
//! reason is the sandbox: a WebAssembly module cannot reach the filesystem, the network, or the
//! host's memory except through what it is explicitly given. Loading someone else's `.wasm` is a
//! bounded act in a way that `dlopen`ing their `.so` is not.
//!
//! Modules are given no WASI imports at all. A plugin that wants to do something outside its own
//! memory has to be called by the document, which is itself governed by the manifest — so a plugin
//! can never exceed the permissions of the document hosting it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use parking_lot::Mutex;
use serde_json::Value;

use crate::context::Ctx;

#[cfg(feature = "plugin")]
use {
    crate::params::decode, serde::Deserialize, serde_json::json, std::sync::atomic::Ordering,
};

/// Bounds on what a plugin may consume, so a runaway module cannot take the document with it.
#[cfg(feature = "plugin")]
const FUEL_PER_CALL: u64 = 100_000_000;
#[cfg(feature = "plugin")]
const MAX_MEMORY_BYTES: usize = 256 * 1024 * 1024;

pub struct PluginModule {
    #[cfg_attr(not(feature = "plugin"), allow(dead_code))]
    ctx: Ctx,
    #[cfg(feature = "plugin")]
    engine: wasmtime::Engine,
    #[cfg(feature = "plugin")]
    loaded: Arc<Mutex<HashMap<u64, LoadedPlugin>>>,
    #[cfg(not(feature = "plugin"))]
    #[allow(dead_code)]
    loaded: Arc<Mutex<HashMap<u64, ()>>>,
    #[cfg_attr(not(feature = "plugin"), allow(dead_code))]
    next: AtomicU64,
}

#[cfg(feature = "plugin")]
struct LoadedPlugin {
    store: wasmtime::Store<()>,
    instance: wasmtime::Instance,
    #[allow(dead_code)]
    path: String,
}

#[cfg(feature = "plugin")]
#[derive(Deserialize)]
struct LoadParams {
    path: String,
}

#[cfg(feature = "plugin")]
#[derive(Deserialize)]
struct HandleParams {
    handle: u64,
}

#[cfg(feature = "plugin")]
#[derive(Deserialize)]
struct CallParams {
    handle: u64,
    #[serde(rename = "function")]
    function: String,
    #[serde(default)]
    args: Vec<Value>,
}

impl PluginModule {
    #[cfg(feature = "plugin")]
    pub fn new(ctx: Ctx) -> Self {
        let mut config = wasmtime::Config::new();
        // Fuel is what makes an infinite loop in a plugin a recoverable error rather than a hung
        // document.
        config.consume_fuel(true);

        Self {
            ctx,
            engine: wasmtime::Engine::new(&config).unwrap_or_default(),
            loaded: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        }
    }

    #[cfg(not(feature = "plugin"))]
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            loaded: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        }
    }

    /// Check a module path against the manifest's `plugin` list.
    #[cfg_attr(not(feature = "plugin"), allow(dead_code))]
    ///
    /// The path is also checked against the `fs` read scope, so a document cannot load a `.wasm`
    /// from somewhere it could not otherwise read.
    fn check_plugin(&self, path: &str) -> Result<std::path::PathBuf, RpcError> {
        let allowed = self.ctx.permissions().plugin.clone();
        if allowed.is_empty() {
            return Err(RpcError::denied("this document was not granted `plugin`"));
        }

        let expanded = htmlapp_caps::permissions::expand_tilde_str(path);
        let permitted = allowed.iter().any(|pattern| {
            let pattern = htmlapp_caps::permissions::expand_tilde_str(pattern);
            pattern == expanded
                || globset::Glob::new(&pattern)
                    .map(|glob| glob.compile_matcher().is_match(&expanded))
                    .unwrap_or(false)
        });
        if !permitted {
            return Err(RpcError::denied(format!(
                "`{path}` is not in this document's `plugin` list ({})",
                allowed.join(", ")
            )));
        }

        Ok(std::path::PathBuf::from(expanded))
    }
}

/// Translate a JSON argument into a wasm value, using the function's declared parameter type.
#[cfg(feature = "plugin")]
fn to_wasm(value: &Value, kind: wasmtime::ValType) -> Result<wasmtime::Val, RpcError> {
    use wasmtime::{Val, ValType};
    Ok(match kind {
        ValType::I32 => Val::I32(
            value
                .as_i64()
                .ok_or_else(|| RpcError::invalid_params("expected an i32"))? as i32,
        ),
        ValType::I64 => Val::I64(
            value
                .as_i64()
                .ok_or_else(|| RpcError::invalid_params("expected an i64"))?,
        ),
        ValType::F32 => Val::F32(
            (value
                .as_f64()
                .ok_or_else(|| RpcError::invalid_params("expected an f32"))? as f32)
                .to_bits(),
        ),
        ValType::F64 => Val::F64(
            value
                .as_f64()
                .ok_or_else(|| RpcError::invalid_params("expected an f64"))?
                .to_bits(),
        ),
        other => {
            return Err(RpcError::invalid_params(format!(
                "{other} parameters are not supported"
            )));
        }
    })
}

#[cfg(feature = "plugin")]
fn from_wasm(value: &wasmtime::Val) -> Value {
    use wasmtime::Val;
    match value {
        Val::I32(v) => json!(v),
        Val::I64(v) => json!(v),
        Val::F32(bits) => json!(f32::from_bits(*bits)),
        Val::F64(bits) => json!(f64::from_bits(*bits)),
        other => json!(format!("{other:?}")),
    }
}

impl ApiHandler for PluginModule {
    fn name(&self) -> &'static str {
        "plugin"
    }

    #[cfg(feature = "plugin")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "load" => {
                    let params: LoadParams = decode("plugin.load", params)?;
                    let path = self.check_plugin(&params.path)?;

                    let bytes = std::fs::read(&path).map_err(|e| {
                        RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed,
                            format!("could not read {}: {e}", path.display()),
                        )
                    })?;
                    if bytes.len() > MAX_MEMORY_BYTES {
                        return Err(RpcError::invalid_params("that module is implausibly large"));
                    }

                    let module = wasmtime::Module::new(&self.engine, &bytes).map_err(|e| {
                        RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed,
                            format!("not a valid WebAssembly module: {e}"),
                        )
                    })?;

                    let mut store = wasmtime::Store::new(&self.engine, ());
                    store.set_fuel(FUEL_PER_CALL).map_err(internal)?;

                    // No imports: a plugin gets no ambient authority whatsoever.
                    let instance = wasmtime::Instance::new(&mut store, &module, &[]).map_err(|e| {
                        RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed,
                            format!(
                                "could not instantiate the module: {e}. Plugins are given no \
                                 imports, so a module needing WASI will not load."
                            ),
                        )
                    })?;

                    let handle = self.next.fetch_add(1, Ordering::SeqCst);
                    self.loaded.lock().insert(
                        handle,
                        LoadedPlugin { store, instance, path: params.path },
                    );
                    Ok(json!(handle))
                }

                "call" => {
                    let params: CallParams = decode("plugin.call", params)?;
                    let mut loaded = self.loaded.lock();
                    let plugin = loaded
                        .get_mut(&params.handle)
                        .ok_or_else(|| RpcError::invalid_params("no such plugin handle"))?;

                    let function = plugin
                        .instance
                        .get_func(&mut plugin.store, &params.function)
                        .ok_or_else(|| {
                            RpcError::invalid_params(format!(
                                "the module exports no function `{}`",
                                params.function
                            ))
                        })?;

                    let signature = function.ty(&plugin.store);
                    let parameters: Vec<wasmtime::ValType> = signature.params().collect();
                    if parameters.len() != params.args.len() {
                        return Err(RpcError::invalid_params(format!(
                            "`{}` takes {} argument(s), {} given",
                            params.function,
                            parameters.len(),
                            params.args.len()
                        )));
                    }

                    let arguments: Vec<wasmtime::Val> = params
                        .args
                        .iter()
                        .zip(parameters)
                        .map(|(value, kind)| to_wasm(value, kind))
                        .collect::<Result<_, _>>()?;

                    let mut results: Vec<wasmtime::Val> = signature
                        .results()
                        .map(|kind| match kind {
                            wasmtime::ValType::I32 => wasmtime::Val::I32(0),
                            wasmtime::ValType::I64 => wasmtime::Val::I64(0),
                            wasmtime::ValType::F32 => wasmtime::Val::F32(0),
                            _ => wasmtime::Val::F64(0),
                        })
                        .collect();

                    // Refuelled per call, so one expensive call does not starve the next.
                    let _ = plugin.store.set_fuel(FUEL_PER_CALL);
                    function
                        .call(&mut plugin.store, &arguments, &mut results)
                        .map_err(|e| {
                            RpcError::new(
                                htmlapp_bridge::ErrorCode::OperationFailed,
                                format!("the plugin trapped: {e}"),
                            )
                        })?;

                    Ok(match results.len() {
                        0 => Value::Null,
                        1 => from_wasm(&results[0]),
                        _ => json!(results.iter().map(from_wasm).collect::<Vec<_>>()),
                    })
                }

                "unload" => {
                    let params: HandleParams = decode("plugin.unload", params)?;
                    self.loaded.lock().remove(&params.handle);
                    Ok(Value::Null)
                }

                other => Err(RpcError::not_found(&format!("plugin.{other}"))),
            }
        })
    }

    #[cfg(not(feature = "plugin"))]
    fn invoke<'a>(&'a self, _method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async {
            Err(RpcError::unsupported(
                "this build was compiled without the `plugin` feature",
            ))
        })
    }
}

#[cfg(feature = "plugin")]
fn internal(error: impl std::fmt::Display) -> RpcError {
    RpcError::internal(error.to_string())
}
