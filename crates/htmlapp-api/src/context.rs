//! The enforcement context shared by every API module.
//!
//! `htmlapp-caps` decides *what a document may do*; this decides *whether this particular call is
//! within it*. Every module holds an [`ApiContext`] and every path, program, origin, and database
//! passes through it, so there is exactly one place where a scope check can be forgotten.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use htmlapp_bridge::RpcError;
use htmlapp_caps::{PathScope, Permissions};

/// Compiled, enforceable scopes derived once from a document's granted permissions.
#[derive(Debug)]
pub struct ApiContext {
    /// The app id, used to scope `store` and per-app state. Falls back to the document hash so an
    /// unnamed document still gets private storage rather than sharing a global namespace.
    app_id: String,
    permissions: Permissions,
    fs_read: PathScope,
    fs_write: PathScope,
    sql_databases: PathScope,
    data_dir: PathBuf,
    /// Paths the user chose through a portal file picker. §11.2 rule 4: the picker *is* the
    /// consent, so these are readable and writable regardless of the manifest's globs.
    portal_grants: parking_lot::RwLock<Vec<PathBuf>>,
}

impl ApiContext {
    pub fn new(app_id: impl Into<String>, permissions: Permissions) -> Result<Self, RpcError> {
        let app_id = app_id.into();

        let (read, write, watch) = permissions
            .fs
            .as_ref()
            .map(|fs| (fs.read.clone(), fs.write.clone(), fs.watch))
            .unwrap_or_default();
        let _ = watch;

        let fs_read = PathScope::new(&read).map_err(|e| RpcError::internal(e.to_string()))?;
        let fs_write = PathScope::new(&write).map_err(|e| RpcError::internal(e.to_string()))?;

        let databases = permissions
            .sql
            .as_ref()
            .map(|sql| sql.databases.clone())
            .unwrap_or_default();
        let sql_databases =
            PathScope::new(&databases).map_err(|e| RpcError::internal(e.to_string()))?;

        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("htmlapp")
            .join("apps")
            .join(&app_id);

        Ok(Self {
            app_id,
            permissions,
            fs_read,
            fs_write,
            sql_databases,
            data_dir,
            portal_grants: parking_lot::RwLock::new(Vec::new()),
        })
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub fn permissions(&self) -> &Permissions {
        &self.permissions
    }

    /// This document's own private directory, created on first use.
    pub fn data_dir(&self) -> Result<&Path, RpcError> {
        std::fs::create_dir_all(&self.data_dir)
            .map_err(|e| RpcError::internal(format!("could not create app data directory: {e}")))?;
        Ok(&self.data_dir)
    }

    /// Record a path the user picked through a portal dialog.
    ///
    /// §11.2 rule 4: "files arrive pre-authorised outside the granted globs". The user selecting a
    /// file in their desktop's own picker is a stronger, more specific act of consent than a glob
    /// in a manifest, so it is honoured — but only for the exact file chosen, never its directory.
    pub fn grant_from_portal(&self, path: impl AsRef<Path>) {
        let canonical = htmlapp_caps::permissions::resolve_for_check(path.as_ref());
        let mut grants = self.portal_grants.write();
        if !grants.contains(&canonical) {
            tracing::debug!(path = %canonical.display(), "granting portal-chosen path");
            grants.push(canonical);
        }
    }

    fn portal_allows(&self, resolved: &Path) -> bool {
        self.portal_grants.read().iter().any(|p| p == resolved)
    }

    /// Check a path against the read scope, returning the canonical path to actually operate on.
    ///
    /// Callers must use the returned path rather than the one they passed in: the check was made
    /// about the resolved path, and re-resolving at open time would leave a window in which a
    /// symlink could be swapped underneath.
    pub fn check_read(&self, path: impl AsRef<Path>) -> Result<PathBuf, RpcError> {
        let resolved = htmlapp_caps::permissions::resolve_for_check(path.as_ref());
        if self.portal_allows(&resolved) {
            return Ok(resolved);
        }
        self.fs_read.check(path.as_ref()).map_err(|denied| {
            RpcError::denied(denied.to_string()).with_data(serde_json::json!({
                "path": denied.requested,
                "resolved": denied.resolved,
                "granted": denied.patterns,
                "mode": "read",
            }))
        })
    }

    /// Check a path against the write scope.
    pub fn check_write(&self, path: impl AsRef<Path>) -> Result<PathBuf, RpcError> {
        let resolved = htmlapp_caps::permissions::resolve_for_check(path.as_ref());
        if self.portal_allows(&resolved) {
            return Ok(resolved);
        }
        self.fs_write.check(path.as_ref()).map_err(|denied| {
            RpcError::denied(denied.to_string()).with_data(serde_json::json!({
                "path": denied.requested,
                "resolved": denied.resolved,
                "granted": denied.patterns,
                "mode": "write",
            }))
        })
    }

    /// Check a database path against `sql.databases`.
    pub fn check_database(&self, path: impl AsRef<Path>) -> Result<PathBuf, RpcError> {
        self.sql_databases.check(path.as_ref()).map_err(|denied| {
            RpcError::denied(format!(
                "database {} is not listed in this document's `sql.databases`",
                denied.requested.display()
            ))
        })
    }

    /// Check a program against `process.exec`.
    ///
    /// An empty allow-list means unrestricted, which the consent sheet reports as Extreme risk.
    /// Matching accepts either a bare name (`rg`) or an absolute path whose file name matches, so
    /// that granting `rg` does not accidentally also grant `/tmp/evil/rg`.
    pub fn check_program(&self, program: &str) -> Result<(), RpcError> {
        let Some(process) = self.permissions.process.as_ref() else {
            return Err(RpcError::denied("this document was not granted `process`"));
        };
        if process.exec.is_empty() {
            return Ok(());
        }

        let requested = Path::new(program);
        let allowed = process.exec.iter().any(|entry| {
            if entry.contains('/') {
                // An absolute or relative path in the manifest must match exactly.
                Path::new(entry) == requested
            } else {
                // A bare name matches only a bare name, so PATH resolution stays the shell's job
                // and an attacker cannot satisfy `rg` with `/tmp/rg`.
                requested.as_os_str() == std::ffi::OsStr::new(entry)
            }
        });

        if allowed {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "`{program}` is not in this document's `process.exec` allow-list ({})",
                process.exec.join(", ")
            )))
        }
    }

    /// Whether the document may open a PTY.
    pub fn check_pty(&self) -> Result<(), RpcError> {
        match self.permissions.process.as_ref() {
            Some(process) if process.pty => Ok(()),
            _ => Err(RpcError::denied(
                "this document was not granted `process.pty`",
            )),
        }
    }

    /// Check a URL against `net.fetch`.
    pub fn check_origin(&self, url: &str) -> Result<(), RpcError> {
        let Some(net) = self.permissions.net.as_ref() else {
            return Err(RpcError::denied("this document was not granted `net`"));
        };
        if net.fetch.iter().any(|pattern| url_matches(pattern, url)) {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "{url} is not in this document's `net.fetch` allow-list ({})",
                net.fetch.join(", ")
            )))
        }
    }

    /// Check clipboard access in one direction.
    pub fn check_clipboard(
        &self,
        access: htmlapp_caps::ClipboardAccess,
    ) -> Result<(), RpcError> {
        if self.permissions.clipboard.contains(&access) {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "this document was not granted clipboard {access:?}"
            )))
        }
    }

    /// Check a D-Bus destination against `dbus.destinations`.
    pub fn check_dbus(&self, bus_is_system: bool, destination: &str) -> Result<(), RpcError> {
        let Some(dbus) = self.permissions.dbus.as_ref() else {
            return Err(RpcError::denied("this document was not granted `dbus`"));
        };
        if bus_is_system && !dbus.system {
            return Err(RpcError::denied("this document was not granted the system bus"));
        }
        if !bus_is_system && !dbus.session {
            return Err(RpcError::denied("this document was not granted the session bus"));
        }
        if dbus.destinations.is_empty() || dbus.destinations.iter().any(|d| d == destination) {
            Ok(())
        } else {
            Err(RpcError::denied(format!(
                "`{destination}` is not in this document's `dbus.destinations`"
            )))
        }
    }
}

/// Match a `net.fetch` pattern against a full URL, honouring the path portion.
///
/// Unlike the CSP source form in `htmlapp-engine`, this can and does enforce the path glob, so a
/// grant of `https://alerts.internal.corp/*` really does mean only that origin's paths.
pub fn url_matches(pattern: &str, url: &str) -> bool {
    let Some((pattern_scheme, pattern_rest)) = pattern.split_once("://") else {
        return false;
    };
    let Some((url_scheme, url_rest)) = url.split_once("://") else {
        return false;
    };
    if pattern_scheme != url_scheme {
        return false;
    }

    let (pattern_host, pattern_path) = split_host(pattern_rest);
    let (url_host, url_path) = split_host(url_rest);

    let host_ok = if let Some(suffix) = pattern_host.strip_prefix("*.") {
        // A label boundary is required, so `notexample.com` cannot satisfy `*.example.com`.
        url_host.ends_with(&format!(".{suffix}"))
    } else {
        pattern_host.eq_ignore_ascii_case(url_host)
    };
    if !host_ok {
        return false;
    }

    glob_match(pattern_path, url_path)
}

fn split_host(rest: &str) -> (&str, &str) {
    match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    }
}

/// A small glob matcher for URL paths, supporting `*` (within a segment) and `**` (across them).
fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern.is_empty() || pattern == "/*" || pattern == "/**" {
        return true;
    }
    globset::GlobBuilder::new(pattern)
        .literal_separator(!pattern.contains("**"))
        .build()
        .ok()
        .map(|g| g.compile_matcher().is_match(value))
        .unwrap_or(false)
}

/// A handle every module holds.
pub type Ctx = Arc<ApiContext>;
