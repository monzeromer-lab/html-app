//! `os` — platform, paths, battery, theme, locale (PRD §9.3 Tier 4).

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct OsModule {
    #[allow(dead_code)]
    ctx: Ctx,
}

#[derive(Deserialize)]
struct EnvParams {
    name: String,
}

impl OsModule {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

/// Which display server this session is actually using — reported on the launcher's diagnostics
/// strip (§7.2) and used by the runtime to decide whether layer-shell is even available.
pub fn session_type() -> &'static str {
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => "wayland",
        Ok("x11") => "x11",
        _ if std::env::var_os("WAYLAND_DISPLAY").is_some() => "wayland",
        _ if std::env::var_os("DISPLAY").is_some() => "x11",
        _ => "unknown",
    }
}

/// The compositor or desktop, when it identifies itself.
pub fn compositor() -> Option<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("DESKTOP_SESSION").ok().filter(|s| !s.is_empty()))
}

/// Read `PRETTY_NAME` out of `/etc/os-release`.
fn distro() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim_matches('"').to_string())
}

fn read_trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

impl OsModule {
    fn info(&self) -> Value {
        json!({
            "platform": "linux",
            "distro": distro(),
            "kernel": read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_default(),
            "arch": std::env::consts::ARCH,
            "hostname": read_trimmed("/proc/sys/kernel/hostname").unwrap_or_default(),
            "sessionType": session_type(),
            "compositor": compositor(),
        })
    }

    fn paths(&self) -> Value {
        json!({
            "home": dirs::home_dir(),
            "config": dirs::config_dir(),
            "data": dirs::data_dir(),
            "cache": dirs::cache_dir(),
            "runtime": dirs::runtime_dir(),
            "documents": dirs::document_dir(),
            "downloads": dirs::download_dir(),
        })
    }

    fn uptime(&self) -> Value {
        let seconds = read_trimmed("/proc/uptime")
            .and_then(|s| s.split_whitespace().next().map(str::to_string))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        json!(seconds)
    }

    /// Battery state, read from sysfs so this needs neither UPower nor a D-Bus grant.
    fn battery(&self) -> Value {
        let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
            return Value::Null;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if read_trimmed(&path.join("type").to_string_lossy()).as_deref() != Some("Battery") {
                continue;
            }
            let capacity = read_trimmed(&path.join("capacity").to_string_lossy())
                .and_then(|s| s.parse::<f64>().ok());
            let status = read_trimmed(&path.join("status").to_string_lossy());
            if let Some(percentage) = capacity {
                return json!({
                    "percentage": percentage,
                    "charging": status.as_deref() == Some("Charging"),
                    "secondsRemaining": Value::Null,
                });
            }
        }
        Value::Null
    }

    /// The desktop's colour scheme. Falls back to light when nothing announces a preference.
    fn theme(&self) -> Value {
        let dark = std::env::var("GTK_THEME")
            .map(|t| t.to_ascii_lowercase().contains("dark"))
            .unwrap_or(false);
        json!({
            "colorScheme": if dark { "dark" } else { "light" },
            "accent": Value::Null,
        })
    }

    fn locale(&self) -> Value {
        for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(name)
                && !value.is_empty()
            {
                return json!(value.split('.').next().unwrap_or(&value));
            }
        }
        json!("C")
    }
}

impl ApiHandler for OsModule {
    fn name(&self) -> &'static str {
        "os"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                "info" => Ok(self.info()),
                "paths" => Ok(self.paths()),
                "uptime" => Ok(self.uptime()),
                "battery" => Ok(self.battery()),
                "theme" => Ok(self.theme()),
                "locale" => Ok(self.locale()),
                "idleTime" => Ok(json!(0)),
                "env" => {
                    let params: EnvParams = decode("os.env", params)?;
                    // Environment variables routinely carry tokens and keys, so the ones most
                    // likely to matter are withheld rather than handed over wholesale.
                    const WITHHELD: &[&str] = &[
                        "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "GITHUB_TOKEN",
                        "GH_TOKEN", "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "NPM_TOKEN",
                        "SSH_AUTH_SOCK", "GPG_AGENT_INFO",
                    ];
                    let upper = params.name.to_ascii_uppercase();
                    if WITHHELD.contains(&upper.as_str())
                        || upper.ends_with("_TOKEN")
                        || upper.ends_with("_SECRET")
                        || upper.ends_with("_PASSWORD")
                        || upper.ends_with("_API_KEY")
                    {
                        return Err(RpcError::denied(format!(
                            "`{}` is withheld because it commonly holds a credential",
                            params.name
                        )));
                    }
                    Ok(json!(std::env::var(&params.name).ok()))
                }
                other => Err(RpcError::not_found(&format!("os.{other}"))),
            }
        })
    }
}
