//! `dialog` — the desktop's own file pickers (docs/api-reference.md, Tier 1).
//!
//! Routed through `xdg-desktop-portal`, so the picker is the system's and the choice is the user's.
//! The security model, rule 4: a file the user picked in their own file chooser is authorised by that act, so it
//! is registered with the context and becomes readable and writable even though no manifest glob
//! covers it. Only the exact file, never its directory.

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::host::HostBridge;
use crate::params::decode;
use std::sync::Arc;

pub struct DialogModule {
    ctx: Ctx,
    /// Message, confirm, and prompt are painted by GPUI so they are modal to the document's own
    /// window; the portal has no equivalent.
    host: Arc<dyn HostBridge>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OpenParams {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    filters: Vec<Filter>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SaveParams {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    default_name: Option<String>,
    #[serde(default)]
    filters: Vec<Filter>,
}

#[derive(Deserialize, Clone)]
struct Filter {
    name: String,
    extensions: Vec<String>,
}

impl DialogModule {
    pub fn new(ctx: Ctx, host: Arc<dyn HostBridge>) -> Self {
        Self { ctx, host }
    }

    /// Turn portal URIs into paths and authorise each one.
    fn accept(&self, uris: Vec<String>) -> Vec<String> {
        uris.into_iter()
            .filter_map(|uri| {
                let path = uri.strip_prefix("file://").unwrap_or(&uri).to_string();
                let decoded = percent_decode(&path);
                self.ctx.grant_from_portal(&decoded);
                Some(decoded)
            })
            .collect()
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(feature = "tier2")]
impl DialogModule {
    async fn open(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

        let params: OpenParams = decode("dialog.open", params)?;
        let mut request = SelectedFiles::open_file()
            .title(params.title.as_deref().unwrap_or("Open"))
            .multiple(params.multiple);
        for filter in &params.filters {
            let mut file_filter = FileFilter::new(&filter.name);
            for extension in &filter.extensions {
                file_filter = file_filter.glob(&format!("*.{extension}"));
            }
            request = request.filter(file_filter);
        }

        let response = request
            .send()
            .await
            .and_then(|r| r.response())
            .map_err(portal_error)?;

        let uris: Vec<String> = response.uris().iter().map(|u| u.to_string()).collect();
        Ok(json!(self.accept(uris)))
    }

    async fn save(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

        let params: SaveParams = decode("dialog.save", params)?;
        let mut request = SelectedFiles::save_file().title(params.title.as_deref().unwrap_or("Save"));
        if let Some(name) = &params.default_name {
            request = request.current_name(name.as_str());
        }
        for filter in &params.filters {
            let mut file_filter = FileFilter::new(&filter.name);
            for extension in &filter.extensions {
                file_filter = file_filter.glob(&format!("*.{extension}"));
            }
            request = request.filter(file_filter);
        }

        let response = request
            .send()
            .await
            .and_then(|r| r.response())
            .map_err(portal_error)?;

        let uris: Vec<String> = response.uris().iter().map(|u| u.to_string()).collect();
        Ok(json!(self.accept(uris).into_iter().next()))
    }

    async fn pick_folder(&self, params: Value) -> Result<Value, RpcError> {
        use ashpd::desktop::file_chooser::SelectedFiles;

        let params: OpenParams = decode("dialog.pickFolder", params)?;
        let response = SelectedFiles::open_file()
            .title(params.title.as_deref().unwrap_or("Choose a folder"))
            .directory(true)
            .send()
            .await
            .and_then(|r| r.response())
            .map_err(portal_error)?;

        let uris: Vec<String> = response.uris().iter().map(|u| u.to_string()).collect();
        Ok(json!(self.accept(uris).into_iter().next()))
    }
}

#[cfg(feature = "tier2")]
fn portal_error(error: ashpd::Error) -> RpcError {
    match error {
        // The user closing the picker is an ordinary outcome, not a failure to report as one.
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            RpcError::new(htmlapp_bridge::ErrorCode::Cancelled, "the user cancelled")
        }
        other => RpcError::new(
            htmlapp_bridge::ErrorCode::OperationFailed,
            format!("the desktop portal refused: {other}"),
        ),
    }
}

impl ApiHandler for DialogModule {
    fn name(&self) -> &'static str {
        "dialog"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier2")]
                "open" => self.open(params).await,
                #[cfg(feature = "tier2")]
                "save" => self.save(params).await,
                #[cfg(feature = "tier2")]
                "pickFolder" => self.pick_folder(params).await,

                // Painted by the host so they are modal to this document's window.
                "message" | "confirm" | "prompt" => {
                    self.host.call("dialog", method, params).await
                }

                #[cfg(not(feature = "tier2"))]
                "open" | "save" | "pickFolder" => {
                    let _ = &params;
                    Err(RpcError::unsupported("this build has no portal support"))
                }
                other => Err(RpcError::not_found(&format!("dialog.{other}"))),
            }
        })
    }
}

/// Show the portal file chooser for a `.hta`, outside any document's context.
///
/// Used by the launcher's "Open an .hta file…" (docs/building.md). The security model, rule 4 applies here too: the file the
/// user picks in their own desktop's chooser is authorised by that act, which is why the launcher
/// can hand it straight to a new process without a manifest glob covering it.
#[cfg(feature = "tier2")]
pub async fn choose_hta() -> Result<Option<std::path::PathBuf>, RpcError> {
    use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

    let response = SelectedFiles::open_file()
        .title("Open an .hta file")
        .filter(FileFilter::new("HTML App documents").glob("*.hta"))
        .filter(FileFilter::new("All files").glob("*"))
        .send()
        .await
        .and_then(|r| r.response())
        .map_err(portal_error)?;

    Ok(response
        .uris()
        .first()
        .map(|uri| std::path::PathBuf::from(percent_decode(uri.path()))))
}

/// Ask where to save a new document (the launcher, "New blank app").
#[cfg(feature = "tier2")]
pub async fn choose_save_location(
    default_name: &str,
) -> Result<Option<std::path::PathBuf>, RpcError> {
    use ashpd::desktop::file_chooser::SelectedFiles;

    let response = SelectedFiles::save_file()
        .title("Save your new app")
        .current_name(default_name)
        .send()
        .await
        .and_then(|r| r.response())
        .map_err(portal_error)?;

    Ok(response
        .uris()
        .first()
        .map(|uri| std::path::PathBuf::from(percent_decode(uri.path()))))
}
