//! `--sandbox` (docs/security.md, rule 7).
//!
//! "`--sandbox` re-execs the whole tree under `bubblewrap` with only the granted paths bind-mounted.
//! WebKit's own renderer sandbox is retained underneath."
//!
//! This is defence in depth, not the primary control. The manifest and the bridge are what actually
//! decide what a document may reach; this makes a bug in *that* enforcement much less useful to an
//! attacker, because the paths simply are not in the mount namespace to be reached.

use std::path::{Path, PathBuf};

use htmlapp_caps::Permissions;

/// Whether `bwrap` is available to re-exec under.
pub fn is_available() -> bool {
    which("bwrap").is_some()
}

fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// Build the `bwrap` argument vector for a document.
///
/// Read-only binds for the system directories the runtime itself needs, read-only or read-write
/// binds for exactly what the manifest granted, and nothing else. `/home` is *not* bound: that is
/// The entire point.
pub fn arguments(granted: Option<&Permissions>, document: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    /// bwrap takes bind flags as `--flag SOURCE DEST`; every one here binds a path onto itself.
    fn bind(args: &mut Vec<String>, flag: &str, path: &str) {
        args.push(flag.to_string());
        args.push(path.to_string());
        args.push(path.to_string());
    }

    // The runtime's own dependencies. Read-only, so a compromised document cannot rewrite them.
    for system in ["/usr", "/bin", "/lib", "/lib64", "/sbin", "/etc"] {
        if Path::new(system).exists() {
            bind(&mut args, "--ro-bind", system);
        }
    }

    args.push("--proc".into());
    args.push("/proc".into());
    args.push("--dev".into());
    args.push("/dev".into());
    args.push("--tmpfs".into());
    args.push("/tmp".into());

    // A new PID namespace, so the document cannot see or signal anything outside its own tree.
    args.push("--unshare-pid".into());
    args.push("--die-with-parent".into());

    // The display server, the session bus, and the runtime dir have to come through or nothing
    // renders. These are the sandbox's real limits, and they are worth being honest about: a
    // document that can talk to the X server can talk to other X clients.
    if let Some(runtime) = dirs::runtime_dir() {
        bind(&mut args, "--bind", &runtime.to_string_lossy());
    }
    if Path::new("/tmp/.X11-unix").exists() {
        bind(&mut args, "--ro-bind", "/tmp/.X11-unix");
    }

    // The runtime binary itself, read-only.
    //
    // Without this, `bwrap` cannot exec it: the system binds above only cover `/usr`, `/bin`, and
    // friends, so an installed-to-`~/.local/bin` binary — which is where the installer puts it —
    // a development build, or a stapled app would all fail with `execvp: No such file or directory`.
    if let Ok(exe) = std::env::current_exe()
        && let Ok(canonical) = exe.canonicalize()
    {
        bind(&mut args, "--ro-bind", &canonical.to_string_lossy());
    }

    // The document itself, read-only.
    if let Some(document) = document
        && let Ok(canonical) = document.canonicalize()
    {
        bind(&mut args, "--ro-bind", &canonical.to_string_lossy());
    }

    // The runtime's own state, so consent and the per-app store survive.
    for directory in [dirs::data_dir(), dirs::cache_dir(), dirs::config_dir()]
        .into_iter()
        .flatten()
    {
        let directory = directory.join("htmlapp");
        let _ = std::fs::create_dir_all(&directory);
        bind(&mut args, "--bind", &directory.to_string_lossy());
    }

    // Exactly what the manifest granted, and nothing more.
    if let Some(permissions) = granted
        && let Some(fs) = permissions.fs.as_ref()
    {
        for pattern in &fs.read {
            if let Some(root) = literal_root(pattern) {
                bind(&mut args, "--ro-bind-try", &root);
            }
        }
        for pattern in &fs.write {
            if let Some(root) = literal_root(pattern) {
                // Created first: bwrap can bind a directory that exists, and a write grant implies
                // The document expects to be able to create things there.
                let _ = std::fs::create_dir_all(&root);
                bind(&mut args, "--bind-try", &root);
            }
        }
    }

    args
}

/// The longest literal directory prefix of a glob — the deepest thing that can actually be bound.
///
/// `~/logs/**` binds `~/logs`, not `~`. A glob cannot be expressed as a mount, so binding the fixed
/// part is as tight as a bind can get; the `fs` scope check still enforces the rest of the pattern.
fn literal_root(pattern: &str) -> Option<String> {
    let expanded = htmlapp_caps::permissions::expand_tilde_str(pattern);
    let mut root = PathBuf::new();

    for component in Path::new(&expanded).components() {
        let text = component.as_os_str().to_string_lossy();
        if text.contains(['*', '?', '[', '{']) {
            break;
        }
        root.push(component.as_os_str());
    }

    if root.as_os_str().is_empty() || root == Path::new("/") {
        return None;
    }

    // A pattern naming a single file binds its containing directory, because a bind of a file
    // cannot be created inside the sandbox if the file is later replaced.
    //
    // A path that does not exist *at all* is left alone and bound with `--bind-try`, which
    // tolerates a missing source. Falling back to the parent here would be an over-bind: a grant
    // of `/tmp/myapp/**` would put the whole of `/tmp` in the namespace, which is exactly the
    // thing the sandbox is meant to prevent.
    if root.is_file()
        && let Some(parent) = root.parent()
    {
        root = parent.to_path_buf();
    }

    Some(root.to_string_lossy().into_owned())
}

/// Re-exec this process under `bwrap`.
///
/// Never returns on success: the sandboxed process replaces this one, so there is no window in
/// which an unsandboxed copy is still running.
pub fn reexec(granted: Option<&Permissions>, document: Option<&Path>) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt as _;

    let Some(bwrap) = which("bwrap") else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "bwrap is not installed; --sandbox needs bubblewrap",
        ));
    };

    // Canonicalised so the path inside the jail matches the one `arguments` bound.
    let exe = std::env::current_exe()?.canonicalize()?;
    let mut command = std::process::Command::new(bwrap);
    command.args(arguments(granted, document));
    command.arg("--");
    command.arg(&exe);

    // Every original argument except `--sandbox`, so the re-exec does not loop forever.
    for argument in std::env::args().skip(1).filter(|a| a != "--sandbox") {
        command.arg(argument);
    }
    // Marks the child so it knows it is already inside the jail.
    command.env("HTMLAPP_SANDBOXED", "1");

    Err(command.exec())
}

/// Whether this process is already running inside the jail.
pub fn is_sandboxed() -> bool {
    std::env::var_os("HTMLAPP_SANDBOXED").is_some()
}
