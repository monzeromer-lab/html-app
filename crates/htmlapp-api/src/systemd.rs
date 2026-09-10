//! `systemd` — journal reading and user unit control (PRD §9.3 Tier 2).
//!
//! Unit control goes over D-Bus to the *user* manager, not the system one: a document should be
//! able to restart its own services without becoming a way to stop `sshd`.
//!
//! The journal is read by streaming `journalctl --output=json` rather than binding
//! `libsystemd`. That keeps the dependency to a binary every systemd machine already has, and it
//! gets follow mode, filtering, and the cursor protocol for free.

use async_stream::try_stream;
use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::context::Ctx;
use crate::params::decode;

const MANAGER: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";

pub struct SystemdModule {
    #[allow(dead_code)]
    ctx: Ctx,
}

#[derive(Deserialize)]
struct UnitParams {
    unit: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunParams {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    properties: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct JournalParams {
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    priority: Option<u8>,
    #[serde(default)]
    follow: bool,
    #[serde(default)]
    lines: Option<u32>,
}

impl SystemdModule {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }

    /// Reject anything that is not a plain unit name.
    ///
    /// The name goes into a D-Bus call and, for `journalctl`, into an argument vector. It is never
    /// shell-interpolated, but a name containing a path separator or a leading dash would still let
    /// a caller address something other than a unit.
    fn check_unit(unit: &str) -> Result<(), RpcError> {
        let plausible = !unit.is_empty()
            && unit.len() <= 256
            && !unit.starts_with('-')
            && unit.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '\\' | ':')
            });
        if plausible {
            Ok(())
        } else {
            Err(RpcError::invalid_params(format!(
                "`{unit}` is not a valid unit name"
            )))
        }
    }
}

#[cfg(feature = "tier2")]
impl SystemdModule {
    async fn connect(&self) -> Result<zbus::Connection, RpcError> {
        // The *user* manager. `Connection::system()` would reach the system one, where stopping a
        // unit is a very different act.
        zbus::Connection::session()
            .await
            .map_err(|e| RpcError::internal(format!("no session bus: {e}")))
    }

    async fn status(&self, params: Value) -> Result<Value, RpcError> {
        let params: UnitParams = decode("systemd.status", params)?;
        Self::check_unit(&params.unit)?;

        let connection = self.connect().await?;
        let reply = connection
            .call_method(
                Some(MANAGER),
                MANAGER_PATH,
                Some(MANAGER_IFACE),
                "LoadUnit",
                &(params.unit.as_str(),),
            )
            .await
            .map_err(failed)?;
        let path: zbus::zvariant::OwnedObjectPath =
            reply.body().deserialize().map_err(internal)?;

        let read = |property: &'static str| {
            let connection = connection.clone();
            let path = path.clone();
            async move {
                let reply = connection
                    .call_method(
                        Some(MANAGER),
                        path.as_ref(),
                        Some("org.freedesktop.DBus.Properties"),
                        "Get",
                        &(UNIT_IFACE, property),
                    )
                    .await
                    .ok()?;
                let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
                String::try_from(value).ok()
            }
        };

        Ok(json!({
            "unit": params.unit,
            "activeState": read("ActiveState").await.unwrap_or_default(),
            "subState": read("SubState").await.unwrap_or_default(),
            "loadState": read("LoadState").await.unwrap_or_default(),
            "description": read("Description").await,
        }))
    }

    /// Start, stop, or restart a user unit.
    async fn lifecycle(&self, action: &str, params: Value) -> Result<Value, RpcError> {
        let params: UnitParams = decode("systemd", params)?;
        Self::check_unit(&params.unit)?;

        let method = match action {
            "start" => "StartUnit",
            "stop" => "StopUnit",
            "restart" => "RestartUnit",
            other => return Err(RpcError::not_found(other)),
        };

        let connection = self.connect().await?;
        connection
            .call_method(
                Some(MANAGER),
                MANAGER_PATH,
                Some(MANAGER_IFACE),
                method,
                // "replace" is the mode every systemctl invocation uses by default.
                &(params.unit.as_str(), "replace"),
            )
            .await
            .map_err(failed)?;
        Ok(Value::Null)
    }

    /// `systemd-run --user` for a scoped transient unit.
    async fn run(&self, params: Value) -> Result<Value, RpcError> {
        let params: RunParams = decode("systemd.run", params)?;
        // A transient unit runs a program, so it is governed by the same allow-list `process` is.
        self.ctx.check_program(&params.program)?;

        let mut command = tokio::process::Command::new("systemd-run");
        command.arg("--user").arg("--collect");
        for (name, value) in &params.properties {
            command.arg("--property").arg(format!("{name}={value}"));
        }
        command.arg("--").arg(&params.program).args(&params.args);

        let output = command.output().await.map_err(|e| {
            RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not run systemd-run: {e}"),
            )
        })?;

        if !output.status.success() {
            return Err(RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(json!(String::from_utf8_lossy(&output.stdout).trim()))
    }
}

#[cfg(feature = "tier2")]
fn failed(error: zbus::Error) -> RpcError {
    RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, error.to_string())
}

#[cfg(feature = "tier2")]
fn internal(error: impl std::fmt::Display) -> RpcError {
    RpcError::internal(error.to_string())
}

/// Stream journal entries, optionally following.
fn journal_stream(params: JournalParams) -> impl futures::Stream<Item = Result<Value, RpcError>> {
    try_stream! {
        use tokio::io::AsyncBufReadExt as _;

        let mut command = tokio::process::Command::new("journalctl");
        command.arg("--user").arg("--output=json").arg("--no-pager");

        if let Some(unit) = &params.unit {
            SystemdModule::check_unit(unit)?;
            command.arg("--unit").arg(unit);
        }
        if let Some(since) = &params.since {
            command.arg("--since").arg(since);
        }
        if let Some(priority) = params.priority {
            command.arg("--priority").arg(priority.to_string());
        }
        command.arg("--lines").arg(params.lines.unwrap_or(200).to_string());
        if params.follow {
            command.arg("--follow");
        }

        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|e| {
            RpcError::new(
                htmlapp_bridge::ErrorCode::OperationFailed,
                format!("could not run journalctl: {e}"),
            )
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            RpcError::internal("journalctl produced no stdout")
        })?;
        let mut lines = tokio::io::BufReader::new(stdout).lines();

        while let Some(line) = lines.next_line().await.map_err(|e| RpcError::internal(e.to_string()))? {
            let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            yield entry_from_journal(raw);
        }

        // Dropping the child kills it, which is what stops `--follow` when the page cancels.
        let _ = child.kill().await;
    }
}

/// Reshape a journal record into the `JournalEntry` the TypeScript declares.
fn entry_from_journal(raw: Map<String, Value>) -> Value {
    let text = |key: &str| raw.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();

    // Journal timestamps are microseconds since the epoch, as a string.
    let timestamp = raw
        .get("__REALTIME_TIMESTAMP")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<u64>().ok())
        .map(|micros| micros / 1000)
        .unwrap_or(0);

    let fields: Map<String, Value> = raw
        .iter()
        .filter(|(key, _)| !key.starts_with('_'))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    json!({
        "timestamp": timestamp,
        "message": text("MESSAGE"),
        "priority": text("PRIORITY").parse::<u8>().unwrap_or(6),
        "unit": raw.get("_SYSTEMD_UNIT").and_then(|v| v.as_str()),
        "fields": fields,
    })
}

impl ApiHandler for SystemdModule {
    fn name(&self) -> &'static str {
        "systemd"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier2")]
                "status" => self.status(params).await,
                #[cfg(feature = "tier2")]
                "start" | "stop" | "restart" => self.lifecycle(method, params).await,
                #[cfg(feature = "tier2")]
                "run" => self.run(params).await,
                #[cfg(not(feature = "tier2"))]
                "status" | "start" | "stop" | "restart" | "run" => {
                    let _ = &params;
                    Err(RpcError::unsupported("this build has no D-Bus support"))
                }
                other => Err(RpcError::not_found(&format!("systemd.{other}"))),
            }
        })
    }

    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "journal" {
                return Err(RpcError::not_found(&format!("systemd.{method}")));
            }
            let params: JournalParams = decode("systemd.journal", params)?;
            Ok(Box::pin(journal_stream(params)) as ValueStream)
        })
    }
}
