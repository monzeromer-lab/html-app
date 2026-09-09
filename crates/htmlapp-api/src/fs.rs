//! `fs` — filesystem access (PRD §9.3 Tier 1).
//!
//! Every path is checked against the manifest's globs with symlinks resolved first (§11.2 rule 5),
//! and the *resolved* path is what gets opened. Re-resolving between the check and the open would
//! leave a window in which a symlink could be swapped underneath the decision.

use std::path::{Path, PathBuf};

use async_stream::try_stream;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use htmlapp_bridge::RpcError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct FsModule {
    ctx: Ctx,
    /// Shared with the origin resolver, which is what actually serves the bytes.
    blobs: htmlapp_bridge::BlobStore,
}

impl FsModule {
    pub fn new(ctx: Ctx, blobs: htmlapp_bridge::BlobStore) -> Self {
        Self { ctx, blobs }
    }

    /// Look up a path previously minted as a `blob:` URL, for the host to serve.
    pub fn blob_path(&self, token: &str) -> Option<PathBuf> {
        self.blobs.get(token)
    }
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

#[derive(Deserialize)]
struct WriteParams {
    path: String,
    contents: String,
    #[serde(default)]
    encoding: Encoding,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Encoding {
    #[default]
    Utf8,
    /// Base64 in JSON, raw bytes on disk.
    Binary,
}

#[derive(Deserialize)]
struct ReadParams {
    path: String,
    #[serde(default)]
    encoding: Encoding,
}

#[derive(Deserialize)]
struct AppendParams {
    path: String,
    contents: String,
}

#[derive(Deserialize)]
struct GlobParams {
    pattern: String,
}

#[derive(Deserialize)]
struct MkdirParams {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct RemoveParams {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct MoveParams {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct ReadStreamParams {
    path: String,
    #[serde(default)]
    chunk_size: Option<usize>,
}

#[derive(Deserialize)]
struct TailParams {
    path: String,
    #[serde(default)]
    lines: Option<usize>,
}

#[derive(Deserialize)]
struct WatchParams {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileStat {
    path: String,
    size: u64,
    is_file: bool,
    is_directory: bool,
    is_symlink: bool,
    modified: Option<u64>,
    created: Option<u64>,
    mode: u32,
    readonly: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DirEntry {
    name: String,
    path: String,
    is_file: bool,
    is_directory: bool,
    is_symlink: bool,
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> RpcError {
    RpcError::new(
        htmlapp_bridge::ErrorCode::OperationFailed,
        format!("could not {action} {}: {error}", path.display()),
    )
}

fn millis(time: std::io::Result<std::time::SystemTime>) -> Option<u64> {
    time.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

impl FsModule {
    async fn read(&self, params: Value) -> Result<Value, RpcError> {
        let params: ReadParams = decode("fs.read", params)?;
        let path = self.ctx.check_read(&params.path)?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| io_error("read", &path, e))?;

        Ok(match params.encoding {
            Encoding::Utf8 => json!(String::from_utf8_lossy(&bytes)),
            Encoding::Binary => {
                use base64::Engine as _;
                json!(base64::engine::general_purpose::STANDARD.encode(&bytes))
            }
        })
    }

    async fn write(&self, params: Value) -> Result<Value, RpcError> {
        let params: WriteParams = decode("fs.write", params)?;
        let path = self.ctx.check_write(&params.path)?;
        let bytes = match params.encoding {
            Encoding::Utf8 => params.contents.into_bytes(),
            Encoding::Binary => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(params.contents.as_bytes())
                    .map_err(|e| RpcError::invalid_params(format!("contents is not base64: {e}")))?
            }
        };
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| io_error("write", &path, e))?;
        Ok(Value::Null)
    }

    async fn append(&self, params: Value) -> Result<Value, RpcError> {
        use tokio::io::AsyncWriteExt as _;
        let params: AppendParams = decode("fs.append", params)?;
        let path = self.ctx.check_write(&params.path)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| io_error("open", &path, e))?;
        file.write_all(params.contents.as_bytes())
            .await
            .map_err(|e| io_error("append to", &path, e))?;
        Ok(Value::Null)
    }

    async fn stat(&self, params: Value) -> Result<Value, RpcError> {
        let params: PathParams = decode("fs.stat", params)?;
        let path = self.ctx.check_read(&params.path)?;
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|e| io_error("stat", &path, e))?;

        use std::os::unix::fs::PermissionsExt as _;
        Ok(json!(FileStat {
            path: path.to_string_lossy().into_owned(),
            size: metadata.len(),
            is_file: metadata.is_file(),
            is_directory: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            modified: millis(metadata.modified()),
            created: millis(metadata.created()),
            mode: metadata.permissions().mode(),
            readonly: metadata.permissions().readonly(),
        }))
    }

    async fn list(&self, params: Value) -> Result<Value, RpcError> {
        let params: PathParams = decode("fs.list", params)?;
        let path = self.ctx.check_read(&params.path)?;
        let mut dir = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| io_error("list", &path, e))?;

        let mut entries = Vec::new();
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| io_error("read", &path, e))?
        {
            let entry_path = entry.path();
            // A directory the document may list can still contain entries it may not read; each
            // one is checked on its own rather than inherited from the parent.
            if self.ctx.check_read(&entry_path).is_err() {
                continue;
            }
            let file_type = entry.file_type().await.ok();
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry_path.to_string_lossy().into_owned(),
                is_file: file_type.is_some_and(|t| t.is_file()),
                is_directory: file_type.is_some_and(|t| t.is_dir()),
                is_symlink: file_type.is_some_and(|t| t.is_symlink()),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(json!(entries))
    }

    async fn glob(&self, params: Value) -> Result<Value, RpcError> {
        let params: GlobParams = decode("fs.glob", params)?;
        let expanded = htmlapp_caps::permissions::expand_tilde_str(&params.pattern);

        // Walk from the pattern's fixed prefix so a broad pattern does not walk the whole disk.
        let root = fixed_prefix(&expanded);
        let matcher = globset::GlobBuilder::new(&expanded)
            .literal_separator(!expanded.contains("**"))
            .build()
            .map_err(|e| RpcError::invalid_params(format!("bad glob: {e}")))?
            .compile_matcher();

        let mut matches = Vec::new();
        let mut stack = vec![root];
        // Bounded so a pathological pattern cannot spin forever inside one call.
        let mut budget = 100_000usize;

        while let Some(dir) = stack.pop() {
            if budget == 0 {
                break;
            }
            let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                budget = budget.saturating_sub(1);
                let path = entry.path();
                if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(path.clone());
                }
                // The glob narrows; the manifest decides. A match the document may not read is
                // simply not reported, so `glob` cannot be used to enumerate outside the grant.
                if matcher.is_match(&path) && self.ctx.check_read(&path).is_ok() {
                    matches.push(path.to_string_lossy().into_owned());
                }
            }
        }

        matches.sort();
        Ok(json!(matches))
    }

    async fn mkdir(&self, params: Value) -> Result<Value, RpcError> {
        let params: MkdirParams = decode("fs.mkdir", params)?;
        let path = self.ctx.check_write(&params.path)?;
        let result = if params.recursive {
            tokio::fs::create_dir_all(&path).await
        } else {
            tokio::fs::create_dir(&path).await
        };
        result.map_err(|e| io_error("create", &path, e))?;
        Ok(Value::Null)
    }

    async fn remove(&self, params: Value) -> Result<Value, RpcError> {
        let params: RemoveParams = decode("fs.remove", params)?;
        let path = self.ctx.check_write(&params.path)?;
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|e| io_error("stat", &path, e))?;

        let result = if metadata.is_dir() {
            if params.recursive {
                tokio::fs::remove_dir_all(&path).await
            } else {
                tokio::fs::remove_dir(&path).await
            }
        } else {
            tokio::fs::remove_file(&path).await
        };
        result.map_err(|e| io_error("remove", &path, e))?;
        Ok(Value::Null)
    }

    async fn rename(&self, params: Value) -> Result<Value, RpcError> {
        let params: MoveParams = decode("fs.rename", params)?;
        // Both ends need write: a rename removes the source as surely as a delete does.
        let from = self.ctx.check_write(&params.from)?;
        let to = self.ctx.check_write(&params.to)?;
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| io_error("rename", &from, e))?;
        Ok(Value::Null)
    }

    async fn copy(&self, params: Value) -> Result<Value, RpcError> {
        let params: MoveParams = decode("fs.copy", params)?;
        let from = self.ctx.check_read(&params.from)?;
        let to = self.ctx.check_write(&params.to)?;
        tokio::fs::copy(&from, &to)
            .await
            .map_err(|e| io_error("copy", &from, e))?;
        Ok(Value::Null)
    }

    /// §9.1: "For genuine bulk transfer the host mints a `blob://` URL the page fetches directly,
    /// so bytes never pass through JSON."
    async fn blob(&self, params: Value) -> Result<Value, RpcError> {
        let params: PathParams = decode("fs.blob", params)?;
        let path = self.ctx.check_read(&params.path)?;

        // An unguessable token, so a blob URL cannot be derived from a path by page script — the
        // URL is a capability in its own right once it exists.
        let token = crate::random_token();
        self.blobs.insert(token.clone(), path);
        Ok(json!(htmlapp_bridge::BlobStore::url_for(&token)))
    }
}

/// The longest literal directory prefix of a glob — where a walk can start.
fn fixed_prefix(pattern: &str) -> PathBuf {
    let mut prefix = PathBuf::new();
    for component in Path::new(pattern).components() {
        let text = component.as_os_str().to_string_lossy();
        if text.contains(['*', '?', '[', '{']) {
            break;
        }
        prefix.push(component.as_os_str());
    }
    if prefix.as_os_str().is_empty() {
        PathBuf::from("/")
    } else if prefix.is_dir() {
        prefix
    } else {
        prefix.parent().map(Path::to_path_buf).unwrap_or(prefix)
    }
}

impl ApiHandler for FsModule {
    fn name(&self) -> &'static str {
        "fs"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "read" => self.read(params).await,
                "write" => self.write(params).await,
                "append" => self.append(params).await,
                "stat" => self.stat(params).await,
                "list" => self.list(params).await,
                "glob" => self.glob(params).await,
                "mkdir" => self.mkdir(params).await,
                "remove" => self.remove(params).await,
                "rename" => self.rename(params).await,
                "copy" => self.copy(params).await,
                "blob" => self.blob(params).await,
                other => Err(RpcError::not_found(&format!("fs.{other}"))),
            }
        })
    }

    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            match method {
                "readStream" => {
                    let params: ReadStreamParams = decode("fs.readStream", params)?;
                    let path = self.ctx.check_read(&params.path)?;
                    let chunk_size = params.chunk_size.unwrap_or(64 * 1024).clamp(1, 4 * 1024 * 1024);
                    Ok(Box::pin(read_stream(path, chunk_size)) as ValueStream)
                }
                "tail" => {
                    let params: TailParams = decode("fs.tail", params)?;
                    let path = self.ctx.check_read(&params.path)?;
                    Ok(Box::pin(tail_stream(path, params.lines.unwrap_or(10))) as ValueStream)
                }
                "watch" => {
                    let params: WatchParams = decode("fs.watch", params)?;
                    let path = self.ctx.check_read(&params.path)?;
                    Ok(Box::pin(watch_stream(path, params.recursive)) as ValueStream)
                }
                other => Err(RpcError::not_found(&format!("fs.{other}"))),
            }
        })
    }
}

/// Read a file in chunks, so a 2 GB file does not have to become one JSON string.
fn read_stream(
    path: PathBuf,
    chunk_size: usize,
) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    try_stream! {
        use tokio::io::AsyncReadExt as _;
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| io_error("open", &path, e))?;
        let mut buffer = vec![0u8; chunk_size];
        loop {
            let read = file
                .read(&mut buffer)
                .await
                .map_err(|e| io_error("read", &path, e))?;
            if read == 0 {
                break;
            }
            yield json!(String::from_utf8_lossy(&buffer[..read]));
        }
    }
}

/// Follow a file as it grows — the log-tailing case from UC1.
fn tail_stream(
    path: PathBuf,
    initial_lines: usize,
) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    try_stream! {
        use tokio::io::{AsyncBufReadExt as _, AsyncSeekExt as _, BufReader};

        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| io_error("open", &path, e))?;
        let length = file
            .metadata()
            .await
            .map_err(|e| io_error("stat", &path, e))?
            .len();

        // Seek back far enough to cover the requested lines rather than reading the whole file,
        // which is the entire point of tailing something large.
        let window = (initial_lines as u64).saturating_mul(512).min(length);
        file.seek(std::io::SeekFrom::Start(length - window))
            .await
            .map_err(|e| io_error("seek in", &path, e))?;

        let mut reader = BufReader::new(file);
        let mut backlog = Vec::new();
        let mut line = String::new();
        while reader
            .read_line(&mut line)
            .await
            .map_err(|e| io_error("read", &path, e))?
            > 0
        {
            backlog.push(std::mem::take(&mut line));
        }
        // A partial first line from mid-file is not a line the caller asked for.
        if window < length && !backlog.is_empty() {
            backlog.remove(0);
        }
        for line in backlog.iter().rev().take(initial_lines).rev() {
            yield json!(line);
        }

        let mut position = length;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let Ok(metadata) = tokio::fs::metadata(&path).await else { continue };
            let current = metadata.len();
            if current < position {
                // Truncated or rotated: start again from the top rather than seeking past the end.
                position = 0;
            }
            if current == position {
                continue;
            }
            let Ok(mut file) = tokio::fs::File::open(&path).await else { continue };
            if file.seek(std::io::SeekFrom::Start(position)).await.is_err() {
                continue;
            }
            let mut reader = BufReader::new(file);
            let mut line = String::new();
            while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                yield json!(std::mem::take(&mut line));
            }
            position = current;
        }
    }
}

/// inotify watches, scoped to the read grant.
#[cfg(feature = "tier1")]
fn watch_stream(
    path: PathBuf,
    recursive: bool,
) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    use notify::{RecursiveMode, Watcher};

    try_stream! {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .map_err(|e| RpcError::internal(format!("could not start a watcher: {e}")))?;

        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        watcher
            .watch(&path, mode)
            .map_err(|e| io_error("watch", &path, std::io::Error::other(e)))?;

        // Held for as long as the stream lives; dropping it removes the inotify watch, which is
        // what makes cancelling a `for await` actually release the kernel resource.
        let _watcher = watcher;

        while let Some(event) = rx.recv().await {
            let Ok(event) = event else { continue };
            let kind = match event.kind {
                notify::EventKind::Create(_) => "created",
                notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => "renamed",
                notify::EventKind::Modify(_) => "modified",
                notify::EventKind::Remove(_) => "removed",
                _ => continue,
            };
            let paths: Vec<String> = event
                .paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            if paths.is_empty() {
                continue;
            }
            yield json!({ "kind": kind, "paths": paths });
        }
    }
}

#[cfg(not(feature = "tier1"))]
fn watch_stream(
    _path: PathBuf,
    _recursive: bool,
) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    futures::stream::once(async {
        Err(RpcError::unsupported("this build has no filesystem watcher"))
    })
}
