//! `ffi` — load a shared object and call into it (PRD §9.3 Tier 4).
//!
//! §9.3 calls this "the loudest permission in the system, off by default, and never granted
//! implicitly", and §11.3 says it "is exempt from none of this and is documented as the escape
//! hatch that voids the model". Both are accurate. A document with `ffi` can do anything the user
//! can do; the manifest's library list and the consent sheet's `Extreme` badge are the only things
//! standing between a page and arbitrary native code.
//!
//! Prefer [`crate::plugin`], which runs third-party code in a WebAssembly sandbox.
//!
//! # What is callable
//!
//! Calling an arbitrary C function needs the callee's exact signature, and getting it wrong is
//! undefined behaviour rather than an error. Rather than pretend to infer it, this supports two
//! explicit, uniform shapes:
//!
//! - **integer/pointer**: up to six arguments, each passed in an integer register
//! - **floating point**: up to four arguments, each passed in an SSE register
//!
//! Between them these cover the overwhelming majority of C entry points. A *mixed* signature
//! cannot be expressed, because on the SysV x86-64 ABI integers and floats are passed in different
//! register files and a single uniform function pointer type cannot describe both. Such a function
//! needs a shim, or `plugin`.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

/// An open library, plus the C strings any call marshalled — kept alive until the library is
/// closed, since a callee may retain a pointer it was handed.
struct OpenLibrary {
    #[cfg(feature = "tier4")]
    library: libloading::Library,
    #[allow(dead_code)]
    path: String,
    retained: Vec<CString>,
}

pub struct FfiModule {
    ctx: Ctx,
    libraries: Arc<Mutex<HashMap<u64, OpenLibrary>>>,
    next: AtomicU64,
}

#[derive(Deserialize)]
struct OpenParams {
    library: String,
}

#[derive(Deserialize)]
struct HandleParams {
    handle: u64,
}

#[derive(Deserialize)]
struct CallParams {
    handle: u64,
    symbol: String,
    #[serde(default)]
    args: Vec<Value>,
    #[serde(default)]
    returns: Option<String>,
}

impl FfiModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            libraries: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
        }
    }

    /// Check a library against the manifest's `ffi` list.
    fn check_library(&self, library: &str) -> Result<(), RpcError> {
        let allowed = self.ctx.permissions().ffi.clone();
        if allowed.is_empty() {
            return Err(RpcError::denied("this document was not granted `ffi`"));
        }
        // Matched on the exact string the manifest named, not on a resolved path: `ffi` is the
        // one place where being generous would be indefensible.
        if allowed.iter().any(|entry| entry == library) {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "`{library}` is not in this document's `ffi` list ({})",
                allowed.join(", ")
            )))
        }
    }
}

/// How an argument will be passed.
#[derive(PartialEq, Eq, Clone, Copy)]
enum ArgClass {
    Integer,
    Float,
}

/// Marshal JSON arguments into one uniform register class.
fn classify(args: &[Value]) -> Result<(ArgClass, Vec<i64>, Vec<f64>, Vec<CString>), RpcError> {
    let any_float = args
        .iter()
        .any(|arg| arg.as_f64().is_some_and(|f| f.fract() != 0.0));
    let class = if any_float {
        ArgClass::Float
    } else {
        ArgClass::Integer
    };

    let mut integers = Vec::new();
    let mut floats = Vec::new();
    let mut retained = Vec::new();

    for arg in args {
        match (class, arg) {
            (ArgClass::Float, value) => floats.push(
                value
                    .as_f64()
                    .ok_or_else(|| RpcError::invalid_params("mixed integer and float arguments are not supported"))?,
            ),
            (ArgClass::Integer, Value::Bool(b)) => integers.push(*b as i64),
            (ArgClass::Integer, Value::Number(n)) => integers.push(
                n.as_i64()
                    .ok_or_else(|| RpcError::invalid_params("integer argument out of range"))?,
            ),
            (ArgClass::Integer, Value::Null) => integers.push(0),
            (ArgClass::Integer, Value::String(text)) => {
                // A string is passed as a `const char *`. The CString must outlive the call, and
                // may outlive it if the callee keeps the pointer, so it is retained.
                let owned = CString::new(text.as_str())
                    .map_err(|_| RpcError::invalid_params("string arguments may not contain NUL"))?;
                integers.push(owned.as_ptr() as i64);
                retained.push(owned);
            }
            (ArgClass::Integer, other) => {
                return Err(RpcError::invalid_params(format!(
                    "cannot pass {other} to a native function"
                )));
            }
        }
    }

    Ok((class, integers, floats, retained))
}

#[cfg(feature = "tier4")]
impl FfiModule {
    /// Call a symbol. Every path through here is `unsafe` by construction.
    ///
    /// SAFETY: there is none to establish. The caller has asserted, via the manifest and the
    /// consent sheet, that this library and this signature are correct. If either is wrong the
    /// behaviour is undefined. That is what §11.3 means by "the escape hatch that voids the model",
    /// and it is why the consent sheet reports `ffi` as `Extreme` and lists it first.
    fn call_symbol(
        &self,
        params: CallParams,
    ) -> Result<Value, RpcError> {
        let (class, integers, floats, retained) = classify(&params.args)?;

        let mut libraries = self.libraries.lock();
        let entry = libraries
            .get_mut(&params.handle)
            .ok_or_else(|| RpcError::invalid_params("no such library handle"))?;

        let symbol_name = CString::new(params.symbol.as_str())
            .map_err(|_| RpcError::invalid_params("symbol names may not contain NUL"))?;

        let address = unsafe {
            entry
                .library
                .get::<*const ()>(symbol_name.as_bytes_with_nul())
                .map_err(|e| {
                    RpcError::new(
                        htmlapp_bridge::ErrorCode::OperationFailed,
                        format!("no symbol `{}`: {e}", params.symbol),
                    )
                })?
                .into_raw()
                .into_raw() as *const ()
        };

        let returns = params.returns.as_deref().unwrap_or("i64");

        let raw: f64 = match class {
            ArgClass::Integer => {
                let call = |n: usize| -> i64 {
                    let a = integers.first().copied().unwrap_or(0);
                    let b = integers.get(1).copied().unwrap_or(0);
                    let c = integers.get(2).copied().unwrap_or(0);
                    let d = integers.get(3).copied().unwrap_or(0);
                    let e = integers.get(4).copied().unwrap_or(0);
                    let f = integers.get(5).copied().unwrap_or(0);
                    unsafe {
                        match n {
                            0 => std::mem::transmute::<_, extern "C" fn() -> i64>(address)(),
                            1 => std::mem::transmute::<_, extern "C" fn(i64) -> i64>(address)(a),
                            2 => std::mem::transmute::<_, extern "C" fn(i64, i64) -> i64>(address)(a, b),
                            3 => std::mem::transmute::<_, extern "C" fn(i64, i64, i64) -> i64>(address)(a, b, c),
                            4 => std::mem::transmute::<_, extern "C" fn(i64, i64, i64, i64) -> i64>(address)(a, b, c, d),
                            5 => std::mem::transmute::<_, extern "C" fn(i64, i64, i64, i64, i64) -> i64>(address)(a, b, c, d, e),
                            _ => std::mem::transmute::<_, extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64>(address)(a, b, c, d, e, f),
                        }
                    }
                };
                if integers.len() > 6 {
                    return Err(RpcError::invalid_params(
                        "at most six integer arguments are supported",
                    ));
                }
                call(integers.len()) as f64
            }
            ArgClass::Float => {
                if floats.len() > 4 {
                    return Err(RpcError::invalid_params(
                        "at most four floating-point arguments are supported",
                    ));
                }
                let a = floats.first().copied().unwrap_or(0.0);
                let b = floats.get(1).copied().unwrap_or(0.0);
                let c = floats.get(2).copied().unwrap_or(0.0);
                let d = floats.get(3).copied().unwrap_or(0.0);
                unsafe {
                    match floats.len() {
                        0 => std::mem::transmute::<_, extern "C" fn() -> f64>(address)(),
                        1 => std::mem::transmute::<_, extern "C" fn(f64) -> f64>(address)(a),
                        2 => std::mem::transmute::<_, extern "C" fn(f64, f64) -> f64>(address)(a, b),
                        3 => std::mem::transmute::<_, extern "C" fn(f64, f64, f64) -> f64>(address)(a, b, c),
                        _ => std::mem::transmute::<_, extern "C" fn(f64, f64, f64, f64) -> f64>(address)(a, b, c, d),
                    }
                }
            }
        };

        entry.retained.extend(retained);

        Ok(match returns {
            "void" => Value::Null,
            "f64" | "double" | "f32" | "float" => json!(raw),
            "string" | "char*" => {
                let pointer = raw as i64 as *const std::ffi::c_char;
                if pointer.is_null() {
                    Value::Null
                } else {
                    // SAFETY: the caller declared this returns a NUL-terminated string. If it does
                    // not, this reads out of bounds — see the note on this function.
                    json!(unsafe { std::ffi::CStr::from_ptr(pointer) }
                        .to_string_lossy()
                        .into_owned())
                }
            }
            "bool" => json!(raw as i64 != 0),
            _ => json!(raw as i64),
        })
    }
}

impl ApiHandler for FfiModule {
    fn name(&self) -> &'static str {
        "ffi"
    }

    #[cfg(feature = "tier4")]
    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "open" => {
                    let params: OpenParams = decode("ffi.open", params)?;
                    self.check_library(&params.library)?;

                    // SAFETY: dlopen runs the library's initialisers. There is no way to make that
                    // safe; the manifest and the consent sheet are the control.
                    let library = unsafe { libloading::Library::new(&params.library) }.map_err(|e| {
                        RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed,
                            format!("could not load {}: {e}", params.library),
                        )
                    })?;

                    let handle = self.next.fetch_add(1, Ordering::SeqCst);
                    tracing::warn!(
                        library = %params.library,
                        "loaded a native library through `ffi`; this document can now run \
                         arbitrary native code"
                    );
                    self.libraries.lock().insert(
                        handle,
                        OpenLibrary { library, path: params.library, retained: Vec::new() },
                    );
                    Ok(json!(handle))
                }

                "call" => {
                    let params: CallParams = decode("ffi.call", params)?;
                    self.call_symbol(params)
                }

                "close" => {
                    let params: HandleParams = decode("ffi.close", params)?;
                    self.libraries.lock().remove(&params.handle);
                    Ok(Value::Null)
                }

                other => Err(RpcError::not_found(&format!("ffi.{other}"))),
            }
        })
    }

    #[cfg(not(feature = "tier4"))]
    fn invoke<'a>(&'a self, _method: &'a str, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async { Err(RpcError::unsupported("this build has no ffi support")) })
    }
}
