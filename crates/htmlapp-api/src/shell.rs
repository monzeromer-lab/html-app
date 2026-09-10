//! `shell` — hand things to the desktop (docs/api-reference.md, Tier 4).

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde::Deserialize;
use serde_json::Value;

use crate::context::Ctx;
use crate::params::decode;

pub struct ShellModule {
    ctx: Ctx,
}

#[derive(Deserialize)]
struct TargetParams {
    target: String,
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

impl ShellModule {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

impl ApiHandler for ShellModule {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "open" => {
                    let params: TargetParams = decode("shell.open", params)?;
                    // `shell.open` hands a target to whatever handler the desktop has registered,
                    // which for some schemes means executing something. Only the schemes a user
                    // would recognise from a link are allowed through.
                    const ALLOWED: &[&str] = &["http://", "https://", "mailto:", "tel:"];
                    let is_url = params.target.contains("://") || params.target.contains(':');
                    if is_url && !ALLOWED.iter().any(|s| params.target.starts_with(s)) {
                        return Err(RpcError::denied(format!(
                            "refusing to open `{}`: only http, https, mailto, and tel targets \
                             may be handed to the desktop",
                            params.target
                        )));
                    }
                    if !is_url {
                        // A bare path is a filesystem reference and is scoped like one.
                        self.ctx.check_read(&params.target)?;
                    }
                    #[cfg(feature = "tier4")]
                    open::that_detached(&params.target)
                        .map_err(|e| RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    Ok(Value::Null)
                }
                "trash" => {
                    let params: PathParams = decode("shell.trash", params)?;
                    // Trashing removes the file from where it was, so it needs write, not read.
                    let path = self.ctx.check_write(&params.path)?;
                    #[cfg(feature = "tier4")]
                    trash::delete(&path).map_err(|e| RpcError::new(
                        htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    let _ = &path;
                    Ok(Value::Null)
                }
                "showInFileManager" => {
                    let params: PathParams = decode("shell.showInFileManager", params)?;
                    let path = self.ctx.check_read(&params.path)?;
                    #[cfg(feature = "tier4")]
                    {
                        let target = if path.is_dir() {
                            path.clone()
                        } else {
                            path.parent().map(std::path::Path::to_path_buf).unwrap_or(path.clone())
                        };
                        open::that_detached(&target).map_err(|e| RpcError::new(
                            htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;
                    }
                    let _ = &path;
                    Ok(Value::Null)
                }
                other => Err(RpcError::not_found(&format!("shell.{other}"))),
            }
        })
    }
}
