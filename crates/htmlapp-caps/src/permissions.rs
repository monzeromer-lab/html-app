//! The capability model (docs/api-reference.md and docs/security.md).
//!
//! Permissions are declared in the manifest and enforced in Rust. Nothing in JS can widen them:
//! The `htmlapp` object is frozen, and a module whose permission was not granted is never injected
//! at all, so `typeof htmlapp.fs === "undefined"` is a truthful feature test.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::error::{CapsError, Result};

/// The full permission block from the manifest, covering every tier of the API catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    // --- Tier 1: the core ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<FsPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessPermission>,
    /// Always available when declared; the picker itself is the user's consent (docs/security.md, rule 4).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dialog: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<NetPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<SqlPermission>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub store: bool,

    // --- Tier 2: Linux desktop integration ---
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub portal: Vec<PortalCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dbus: Option<DbusPermission>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tray: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notifications: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secrets: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub systemd: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub udev: bool,

    // --- Tier 3: windowing and shell ---
    /// `window` control is implied by having a window, so this only gates the *mutating* calls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub window: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub layer: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub menu: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub palette: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortcut: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clipboard: Vec<ClipboardAccess>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dnd: bool,

    // --- Tier 4: reach ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sockets: Option<SocketPermission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub serial: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub usb: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bluetooth: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub os: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shell: bool,
    /// The API catalog: "the loudest permission in the system, off by default, never granted implicitly."
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ffi: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin: Vec<String>,
}

impl Permissions {
    /// True when nothing at all was requested — the document is just a page in a window.
    pub fn is_empty(&self) -> bool {
        *self == Permissions::default()
    }

    /// Reject permission sets that cannot be safely enforced.
    pub fn validate(&self) -> Result<()> {
        if let Some(fs) = &self.fs {
            for glob in fs.read.iter().chain(&fs.write) {
                compile_glob(glob)?;
            }
        }
        if let Some(net) = &self.net {
            for origin in &net.fetch {
                validate_origin(origin)?;
            }
        }
        Ok(())
    }

    /// The set of JS module names to inject (docs/api-reference.md). A module absent from this set is not
    /// injected at all, so the property genuinely does not exist on `htmlapp`.
    pub fn granted_modules(&self) -> BTreeSet<&'static str> {
        let mut modules = BTreeSet::new();
        if self.fs.is_some() {
            modules.insert("fs");
        }
        if self.process.is_some() {
            modules.insert("process");
        }
        if self.dialog {
            modules.insert("dialog");
        }
        if self.net.is_some() {
            modules.insert("http");
        }
        if self.sql.is_some() {
            modules.insert("sql");
        }
        if self.store {
            modules.insert("store");
        }
        if !self.portal.is_empty() {
            modules.insert("portal");
        }
        if self.dbus.is_some() {
            modules.insert("dbus");
        }
        if self.tray {
            modules.insert("tray");
        }
        if self.notifications {
            modules.insert("notify");
        }
        if self.secrets {
            modules.insert("secrets");
        }
        if self.systemd {
            modules.insert("systemd");
        }
        if self.udev {
            modules.insert("udev");
        }
        if self.window {
            modules.insert("window");
        }
        if self.layer {
            modules.insert("layer");
        }
        if self.menu {
            modules.insert("menu");
        }
        if self.palette {
            modules.insert("palette");
        }
        if !self.shortcut.is_empty() {
            modules.insert("shortcut");
        }
        if !self.clipboard.is_empty() {
            modules.insert("clipboard");
        }
        if self.dnd {
            modules.insert("dnd");
        }
        if self.sockets.is_some() {
            modules.insert("net");
        }
        if !self.serial.is_empty() {
            modules.insert("serial");
        }
        if self.usb {
            modules.insert("usb");
        }
        if self.bluetooth {
            modules.insert("bluetooth");
        }
        if self.os {
            modules.insert("os");
        }
        if self.shell {
            modules.insert("shell");
        }
        if !self.ffi.is_empty() {
            modules.insert("ffi");
        }
        if !self.plugin.is_empty() {
            modules.insert("plugin");
        }
        modules
    }

    /// Plain-language lines for the consent sheet (docs/security.md, rule 3). Ordered most-alarming-first so
    /// The risky grants are never below the fold.
    pub fn describe(&self) -> Vec<PermissionDescription> {
        let mut out = Vec::new();

        if !self.ffi.is_empty() {
            out.push(PermissionDescription {
                module: "ffi",
                risk: Risk::Extreme,
                summary: "Load and run arbitrary native code".into(),
                detail: format!(
                    "Can call into these shared libraries directly: {}. This bypasses every other \
                     protection listed here.",
                    self.ffi.join(", ")
                ),
            });
        }
        if let Some(process) = &self.process {
            let detail = if process.exec.is_empty() {
                "Can run any program on this system.".to_string()
            } else {
                format!("Can run these programs: {}.", process.exec.join(", "))
            };
            out.push(PermissionDescription {
                module: "process",
                risk: if process.exec.is_empty() { Risk::Extreme } else { Risk::High },
                summary: "Run other programs".into(),
                detail: if process.pty {
                    format!("{detail} Can also open interactive terminal sessions.")
                } else {
                    detail
                },
            });
        }
        if let Some(fs) = &self.fs {
            if !fs.write.is_empty() {
                out.push(PermissionDescription {
                    module: "fs",
                    risk: Risk::High,
                    summary: "Change files on this computer".into(),
                    detail: format!("Can create, modify, and delete: {}.", fs.write.join(", ")),
                });
            }
            if !fs.read.is_empty() {
                out.push(PermissionDescription {
                    module: "fs",
                    risk: Risk::Medium,
                    summary: "Read files on this computer".into(),
                    detail: format!("Can read: {}.", fs.read.join(", ")),
                });
            }
        }
        if self.secrets {
            out.push(PermissionDescription {
                module: "secrets",
                risk: Risk::High,
                summary: "Read and store passwords in your keyring".into(),
                detail: "Can access saved credentials through the system Secret Service.".into(),
            });
        }
        if let Some(net) = &self.net {
            out.push(PermissionDescription {
                module: "http",
                risk: Risk::Medium,
                summary: "Send data over the network".into(),
                detail: format!("Can contact: {}.", net.fetch.join(", ")),
            });
        }
        if let Some(dbus) = &self.dbus {
            out.push(PermissionDescription {
                module: "dbus",
                risk: Risk::High,
                summary: "Talk to system services".into(),
                detail: format!(
                    "Can reach these D-Bus destinations: {}.",
                    if dbus.destinations.is_empty() {
                        "any".to_string()
                    } else {
                        dbus.destinations.join(", ")
                    }
                ),
            });
        }
        if let Some(sql) = &self.sql {
            out.push(PermissionDescription {
                module: "sql",
                risk: Risk::Medium,
                summary: "Open databases".into(),
                detail: format!("Can query and modify: {}.", sql.databases.join(", ")),
            });
        }
        if !self.clipboard.is_empty() {
            let reads = self.clipboard.contains(&ClipboardAccess::Read);
            out.push(PermissionDescription {
                module: "clipboard",
                risk: if reads { Risk::Medium } else { Risk::Low },
                summary: if reads {
                    "Read and write the clipboard".into()
                } else {
                    "Write to the clipboard".into()
                },
                detail: if reads {
                    "Can see whatever you copy while this app is running.".into()
                } else {
                    "Can place content on the clipboard.".into()
                },
            });
        }
        if !self.serial.is_empty() || self.usb || self.bluetooth {
            out.push(PermissionDescription {
                module: "devices",
                risk: Risk::Medium,
                summary: "Communicate with connected devices".into(),
                detail: "Can open serial, USB, or Bluetooth devices attached to this computer."
                    .into(),
            });
        }
        if self.notifications {
            out.push(PermissionDescription {
                module: "notify",
                risk: Risk::Low,
                summary: "Show desktop notifications".into(),
                detail: "Can post notifications to your desktop.".into(),
            });
        }
        if self.store {
            out.push(PermissionDescription {
                module: "store",
                risk: Risk::Low,
                summary: "Save its own settings".into(),
                detail: "Can keep private data in its own folder.".into(),
            });
        }
        out
    }
}

/// How loudly the consent sheet should present a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
    /// Voids the model. Currently only unrestricted `process` and any `ffi`.
    Extreme,
}

/// One plain-language line in the consent sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDescription {
    pub module: &'static str,
    pub risk: Risk,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FsPermission {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    /// inotify watches are scoped to the read set.
    #[serde(default)]
    pub watch: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessPermission {
    /// Executable allow-list. Empty means unrestricted, which `describe()` flags as Extreme.
    #[serde(default)]
    pub exec: Vec<String>,
    #[serde(default)]
    pub pty: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetPermission {
    #[serde(default)]
    pub fetch: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SqlPermission {
    #[serde(default)]
    pub databases: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DbusPermission {
    #[serde(default)]
    pub session: bool,
    #[serde(default)]
    pub system: bool,
    /// Bus names this document may address. Empty means any, which `describe()` reports.
    #[serde(default)]
    pub destinations: Vec<String>,
    /// Bus names this document may own (`own_name` / `export_object`).
    #[serde(default)]
    pub own: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SocketPermission {
    #[serde(default)]
    pub connect: Vec<String>,
    #[serde(default)]
    pub listen: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardAccess {
    Read,
    Write,
}

/// The `xdg-desktop-portal` interfaces from the API catalog, Tier 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PortalCapability {
    Screenshot,
    ScreenCast,
    GlobalShortcuts,
    Inhibit,
    OpenUri,
    Background,
    Notification,
    Wallpaper,
    Location,
    FileChooser,
}

/// The threat model: a bare wildcard origin would make the `http` allow-list meaningless, so it is refused
/// at parse time rather than silently accepted.
fn validate_origin(origin: &str) -> Result<()> {
    let trimmed = origin.trim();
    if trimmed.is_empty()
        || trimmed == "*"
        || trimmed == "*://*"
        || trimmed == "*://*/*"
        || trimmed == "https://*"
        || trimmed == "http://*"
        || trimmed == "https://*/*"
        || trimmed == "http://*/*"
    {
        return Err(CapsError::WildcardOrigin(origin.to_string()));
    }
    Ok(())
}

/// Compile one manifest glob.
///
/// `literal_separator` is the security-relevant setting here: without it `*` would match across
/// `/`, so a grant of `/var/log/nginx/*.log` would silently also cover
/// `/var/log/nginx/../../../etc/x.log`-shaped paths at any depth. The PRD's own examples use `*`
/// and `**` to mean different things, so they have to actually mean different things.
fn compile_glob(pattern: &str) -> Result<Glob> {
    GlobBuilder::new(&expand_tilde_str(pattern))
        .literal_separator(true)
        .build()
        .map_err(|source| CapsError::InvalidGlob {
            glob: pattern.to_string(),
            source,
        })
}

/// Expand a leading `~` against the user's home directory.
pub fn expand_tilde_str(pattern: &str) -> String {
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    } else if pattern == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    pattern.to_string()
}

/// A compiled, enforceable view of one set of path globs.
///
/// The security model, rule 5: paths are resolved to canonical form *before* the glob check, so a symlink at
/// `~/logs/link-to-etc` cannot be used to escape the granted set.
#[derive(Clone)]
pub struct PathScope {
    globs: GlobSet,
    patterns: Vec<String>,
}

impl std::fmt::Debug for PathScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathScope")
            .field("patterns", &self.patterns)
            .finish()
    }
}

impl PathScope {
    /// Compile a set of glob patterns, expanding `~` as it goes.
    pub fn new<I, S>(patterns: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut builder = GlobSetBuilder::new();
        let mut kept = Vec::new();
        for pattern in patterns {
            let pattern = pattern.as_ref();
            builder.add(compile_glob(pattern)?);
            kept.push(pattern.to_string());
        }
        let globs = builder.build().map_err(|source| CapsError::InvalidGlob {
            glob: kept.join(", "),
            source,
        })?;
        Ok(Self { globs, patterns: kept })
    }

    /// An empty scope matches nothing. This is the deny-by-default case.
    pub fn empty() -> Self {
        Self {
            globs: GlobSetBuilder::new().build().expect("empty globset is valid"),
            patterns: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Whether `path` falls inside this scope, after symlink resolution.
    pub fn allows(&self, path: impl AsRef<Path>) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let resolved = resolve_for_check(path.as_ref());
        self.globs.is_match(&resolved)
    }

    /// Like [`allows`](Self::allows), but returns the canonical path that was checked, so callers
    /// can operate on exactly the path the decision was made about rather than re-resolving and
    /// opening a TOCTOU window.
    pub fn check(&self, path: impl AsRef<Path>) -> std::result::Result<PathBuf, DeniedPath> {
        let requested = path.as_ref().to_path_buf();
        let resolved = resolve_for_check(&requested);
        if !self.patterns.is_empty() && self.globs.is_match(&resolved) {
            Ok(resolved)
        } else {
            Err(DeniedPath {
                requested,
                resolved,
                patterns: self.patterns.clone(),
            })
        }
    }
}

/// A path that failed its scope check, carrying enough context for a useful error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeniedPath {
    pub requested: PathBuf,
    pub resolved: PathBuf,
    pub patterns: Vec<String>,
}

impl std::fmt::Display for DeniedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "access to {} (resolved to {}) is not granted by this document's manifest",
            self.requested.display(),
            self.resolved.display()
        )
    }
}

impl std::error::Error for DeniedPath {}

/// Resolve a path to the canonical form used for permission checks.
///
/// `canonicalize` only works on paths that exist, but a write to a not-yet-created file must still
/// be checked. So the deepest existing ancestor is canonicalized — which is what actually resolves
/// any symlinks in play — and the remaining components are appended with `.` and `..` folded away.
/// That closes the escape where `~/logs/link-to-etc/passwd` would otherwise be checked verbatim.
pub fn resolve_for_check(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };

    let mut existing = absolute.as_path();
    let mut trailing: Vec<Component<'_>> = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(canonical) => {
                let mut out = canonical;
                for component in trailing.iter().rev() {
                    match component {
                        Component::CurDir => {}
                        Component::ParentDir => {
                            out.pop();
                        }
                        other => out.push(other.as_os_str()),
                    }
                }
                return out;
            }
            Err(_) => match existing.parent() {
                Some(parent) => {
                    if let Some(name) = existing.file_name() {
                        trailing.push(Component::Normal(name));
                    } else if existing.ends_with("..") {
                        trailing.push(Component::ParentDir);
                    }
                    existing = parent;
                }
                // Reached the root without finding anything that exists.
                None => return normalize_lexically(&absolute),
            },
        }
    }
}

/// Fold `.` and `..` without touching the filesystem. Only used when nothing on the path exists.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
