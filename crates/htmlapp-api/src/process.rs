//! `process` — running other programs (PRD §9.3 Tier 1).
//!
//! The executable allow-list comes from the manifest and is checked before anything is spawned. A
//! grant of `rg` means the bare name `rg`, resolved through the user's `PATH` by the OS — it does
//! not mean `/tmp/attacker/rg`, which is why [`ApiContext::check_program`] refuses to treat a path
//! and a name as interchangeable.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use async_stream::try_stream;
use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use crate::context::Ctx;
use crate::params::decode;

/// A process this document started, and the handles needed to talk to it.
struct Child {
    program: String,
    args: Vec<String>,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
    stdin: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    /// `Some` for a PTY-backed child, which resizes rather than being a plain pipe.
    resize: Option<tokio::sync::mpsc::UnboundedSender<(u16, u16)>>,
    running: bool,
}

pub struct ProcessModule {
    ctx: Ctx,
    children: Arc<Mutex<HashMap<i32, Child>>>,
    /// PTY children have no real pid until they start, so a negative synthetic id is handed out
    /// immediately and mapped to the real one when it is known.
    next_synthetic: AtomicI32,
}

impl ProcessModule {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            children: Arc::new(Mutex::new(HashMap::new())),
            next_synthetic: AtomicI32::new(-1),
        }
    }

    /// Terminate everything this document started. Called when the document closes, so a page
    /// cannot outlive its window by leaving a subprocess running.
    pub fn kill_all(&self) {
        let mut children = self.children.lock();
        for (_, child) in children.iter_mut() {
            if let Some(kill) = child.kill.take() {
                let _ = kill.send(());
            }
        }
        children.clear();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnParams {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    stdin: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PtyParams {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
}

#[derive(Deserialize)]
struct PidParams {
    pid: i32,
}

#[derive(Deserialize)]
struct SignalParams {
    pid: i32,
    signal: String,
}

#[derive(Deserialize)]
struct WriteParams {
    pid: i32,
    data: String,
}

#[derive(Deserialize)]
struct ResizeParams {
    pid: i32,
    cols: u16,
    rows: u16,
}

impl ProcessModule {
    /// Validate a spawn request against the manifest, and resolve its working directory.
    fn prepare(
        &self,
        program: &str,
        cwd: Option<&String>,
    ) -> Result<Option<std::path::PathBuf>, RpcError> {
        self.ctx.check_program(program)?;
        // A cwd is a filesystem reference like any other, so it goes through the read scope rather
        // than being taken on trust.
        match cwd {
            Some(dir) => Ok(Some(self.ctx.check_read(dir)?)),
            None => Ok(None),
        }
    }

    async fn exec(&self, params: Value) -> Result<Value, RpcError> {
        let params: SpawnParams = decode("process.exec", params)?;
        let cwd = self.prepare(&params.program, params.cwd.as_ref())?;

        let mut command = tokio::process::Command::new(&params.program);
        command
            .args(&params.args)
            .envs(&params.env)
            .stdin(if params.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        let mut child = command
            .spawn()
            .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not run {}: {e}", params.program)))?;

        if let Some(input) = params.stdin
            && let Some(mut stdin) = child.stdin.take()
        {
            let _ = stdin.write_all(input.as_bytes()).await;
            drop(stdin);
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?;

        Ok(json!({
            "code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }

    fn kill(&self, params: Value) -> Result<Value, RpcError> {
        let params: PidParams = decode("process.kill", params)?;
        let mut children = self.children.lock();
        // Only processes this document started are addressable; a bare pid from elsewhere is not
        // a handle the page ever had.
        let Some(child) = children.get_mut(&params.pid) else {
            return Err(RpcError::denied(
                "no such process was started by this document",
            ));
        };
        if let Some(kill) = child.kill.take() {
            let _ = kill.send(());
        }
        child.running = false;
        Ok(Value::Null)
    }

    fn signal(&self, params: Value) -> Result<Value, RpcError> {
        let params: SignalParams = decode("process.signal", params)?;
        if !self.children.lock().contains_key(&params.pid) {
            return Err(RpcError::denied(
                "no such process was started by this document",
            ));
        }
        if params.pid <= 0 {
            return Err(RpcError::invalid_params("cannot signal a pty by pid"));
        }

        let signal = match params.signal.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
            "TERM" => 15,
            "KILL" => 9,
            "INT" => 2,
            "HUP" => 1,
            "QUIT" => 3,
            "USR1" => 10,
            "USR2" => 12,
            "STOP" => 19,
            "CONT" => 18,
            other => return Err(RpcError::invalid_params(format!("unknown signal {other}"))),
        };

        // `kill` is used rather than a raw libc call so this crate can keep forbidding unsafe.
        let status = std::process::Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(params.pid.to_string())
            .status()
            .map_err(|e| RpcError::internal(e.to_string()))?;
        if status.success() {
            Ok(Value::Null)
        } else {
            Err(RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not signal {}", params.pid),
            ))
        }
    }

    fn list(&self) -> Result<Value, RpcError> {
        let children = self.children.lock();
        let listed: Vec<Value> = children
            .iter()
            .map(|(pid, child)| {
                json!({
                    "pid": pid,
                    "program": child.program,
                    "args": child.args,
                    "running": child.running,
                })
            })
            .collect();
        Ok(json!(listed))
    }

    fn write(&self, params: Value) -> Result<Value, RpcError> {
        let params: WriteParams = decode("process.write", params)?;
        let children = self.children.lock();
        let Some(child) = children.get(&params.pid) else {
            return Err(RpcError::denied(
                "no such process was started by this document",
            ));
        };
        let Some(stdin) = &child.stdin else {
            return Err(RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                "that process has no open stdin",
            ));
        };
        stdin
            .send(params.data.into_bytes())
            .map_err(|_| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, "process has exited"))?;
        Ok(Value::Null)
    }

    fn resize(&self, params: Value) -> Result<Value, RpcError> {
        let params: ResizeParams = decode("process.resize", params)?;
        let children = self.children.lock();
        let Some(child) = children.get(&params.pid) else {
            return Err(RpcError::denied(
                "no such process was started by this document",
            ));
        };
        let Some(resize) = &child.resize else {
            return Err(RpcError::invalid_params("that process is not pty-backed"));
        };
        let _ = resize.send((params.cols, params.rows));
        Ok(Value::Null)
    }
}

impl ApiHandler for ProcessModule {
    fn name(&self) -> &'static str {
        "process"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "exec" => self.exec(params).await,
                "kill" => self.kill(params),
                "signal" => self.signal(params),
                "list" => self.list(),
                "write" => self.write(params),
                "resize" => self.resize(params),
                other => Err(RpcError::not_found(&format!("process.{other}"))),
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
                "spawn" => {
                    let params: SpawnParams = decode("process.spawn", params)?;
                    let cwd = self.prepare(&params.program, params.cwd.as_ref())?;
                    Ok(Box::pin(spawn_stream(Arc::clone(&self.children), params, cwd)) as ValueStream)
                }
                "pty" => {
                    let params: PtyParams = decode("process.pty", params)?;
                    self.ctx.check_pty()?;
                    let cwd = self.prepare(&params.program, params.cwd.as_ref())?;
                    let id = self.next_synthetic.fetch_sub(1, Ordering::SeqCst);
                    Ok(Box::pin(pty_stream(Arc::clone(&self.children), params, cwd, id)) as ValueStream)
                }
                other => Err(RpcError::not_found(&format!("process.{other}"))),
            }
        })
    }
}

/// Run a program, streaming stdout and stderr as they arrive.
fn spawn_stream(
    children: Arc<Mutex<HashMap<i32, Child>>>,
    params: SpawnParams,
    cwd: Option<std::path::PathBuf>,
) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    try_stream! {
        let mut command = tokio::process::Command::new(&params.program);
        command
            .args(&params.args)
            .envs(&params.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        let mut child = command.spawn().map_err(|e| {
            RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not run {}: {e}", params.program),
            )
        })?;

        let pid = child.id().unwrap_or(0) as i32;
        let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel();
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        let mut stdin = child.stdin.take();
        tokio::spawn(async move {
            while let Some(data) = stdin_rx.recv().await {
                let Some(pipe) = stdin.as_mut() else { break };
                if pipe.write_all(&data).await.is_err() {
                    break;
                }
                let _ = pipe.flush().await;
            }
        });

        children.lock().insert(pid, Child {
            program: params.program.clone(),
            args: params.args.clone(),
            kill: Some(kill_tx),
            stdin: Some(stdin_tx),
            resize: None,
            running: true,
        });

        yield json!({ "kind": "started", "pid": pid });

        let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<(bool, String)>();
        if let Some(stdout) = child.stdout.take() {
            let tx = line_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if tx.send((false, line)).is_err() { break }
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            let tx = line_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if tx.send((true, line)).is_err() { break }
                }
            });
        }
        drop(line_tx);

        let exit = loop {
            tokio::select! {
                // Biased so that queued output is drained before the exit is reported; otherwise a
                // fast program's last lines race its own termination and get dropped.
                biased;
                line = line_rx.recv() => match line {
                    Some((is_stderr, line)) => {
                        yield json!({
                            "kind": if is_stderr { "stderr" } else { "stdout" },
                            "data": line,
                        });
                    }
                    None => break child.wait().await.ok(),
                },
                _ = &mut kill_rx => {
                    let _ = child.start_kill();
                    break child.wait().await.ok();
                }
            }
        };

        if let Some(entry) = children.lock().get_mut(&pid) {
            entry.running = false;
            entry.stdin = None;
        }

        yield json!({
            "kind": "exit",
            "code": exit.as_ref().and_then(|s| s.code()),
            "signal": Value::Null,
        });
    }
}

/// Run a program under a pseudo-terminal — what makes UC3's embedded terminal a real terminal.
#[cfg(feature = "tier1")]
fn pty_stream(
    children: Arc<Mutex<HashMap<i32, Child>>>,
    params: PtyParams,
    cwd: Option<std::path::PathBuf>,
    id: i32,
) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    try_stream! {
        let size = PtySize {
            rows: params.rows.unwrap_or(24),
            cols: params.cols.unwrap_or(80),
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| RpcError::internal(format!("could not open a pty: {e}")))?;

        let mut command = CommandBuilder::new(&params.program);
        command.args(&params.args);
        if let Some(cwd) = &cwd {
            command.cwd(cwd);
        }
        // Without this, curses programs run in the pty come up with no colour and a broken layout.
        command.env("TERM", "xterm-256color");

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not run {} under a pty: {e}", params.program),
            ))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| RpcError::internal(e.to_string()))?;

        let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel();
        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (resize_tx, mut resize_rx) = tokio::sync::mpsc::unbounded_channel::<(u16, u16)>();

        children.lock().insert(id, Child {
            program: params.program.clone(),
            args: params.args.clone(),
            kill: Some(kill_tx),
            stdin: Some(stdin_tx),
            resize: Some(resize_tx),
            running: true,
        });

        // portable-pty's reader and writer are blocking, so they live on blocking threads and
        // talk to the async side over channels.
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out_tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut writer = writer;
        std::thread::spawn(move || {
            use std::io::Write as _;
            while let Some(data) = stdin_rx.blocking_recv() {
                if writer.write_all(&data).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        });

        let master = pair.master;
        yield json!({ "kind": "started", "pid": id });

        loop {
            tokio::select! {
                biased;
                chunk = out_rx.recv() => match chunk {
                    Some(bytes) => yield json!({
                        "kind": "stdout",
                        "data": String::from_utf8_lossy(&bytes),
                    }),
                    None => break,
                },
                Some((cols, rows)) = resize_rx.recv() => {
                    let _ = master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                }
                _ = &mut kill_rx => {
                    let _ = child.kill();
                    break;
                }
            }
        }

        let status = child.wait().ok();
        if let Some(entry) = children.lock().get_mut(&id) {
            entry.running = false;
            entry.stdin = None;
        }

        yield json!({
            "kind": "exit",
            "code": status.map(|s| s.exit_code() as i64),
            "signal": Value::Null,
        });
    }
}

#[cfg(not(feature = "tier1"))]
fn pty_stream(
    _children: Arc<Mutex<HashMap<i32, Child>>>,
    _params: PtyParams,
    _cwd: Option<std::path::PathBuf>,
    _id: i32,
) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    futures::stream::once(async { Err(RpcError::unsupported("this build has no pty support")) })
}
