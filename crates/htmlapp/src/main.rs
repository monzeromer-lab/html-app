//! `htmlapp` — the command line entry point (docs/building.md).
//!
//! The launch modes is emphatic that "the CLI is one way in, not *the* way in", and that a bare invocation with
//! no document is a normal launch rather than a usage error. So the default action here is to open
//! The launcher, and every subcommand is something you asked for explicitly.

mod delegate;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use htmlapp_caps::{ConsentStore, Document, Recents};
use htmlapp_runtime::{Session, SessionOptions};

#[derive(Parser, Debug)]
#[command(
    name = "htmlapp",
    version,
    about = "Run a single .hta file as a Linux desktop application",
    long_about = None,
    disable_help_subcommand = true
)]
struct Cli {
    /// The `.hta` document to run. With no document, the launcher opens (docs/building.md).
    #[arg(value_name = "DOCUMENT")]
    document: Option<PathBuf>,

    /// Run with no window: stdin → stdout (docs/bridge.md).
    #[arg(long)]
    headless: bool,

    /// Open the file chooser immediately, as the desktop entry's "Open an .hta file…" action does.
    #[arg(long)]
    open: bool,

    /// Open the consent manager.
    #[arg(long = "permissions-ui")]
    permissions_ui: bool,

    /// Make the engine inspector reachable.
    #[arg(long)]
    devtools: bool,

    /// Do not record this document in the launcher's recents (the open questions, open question 8).
    #[arg(long)]
    private: bool,

    /// Run the document with no native APIs, whatever its manifest asks for.
    #[arg(long = "no-permissions")]
    no_permissions: bool,

    /// A hint passed to the document as `htmlapp.format` (docs/bridge.md).
    #[arg(long, value_name = "FORMAT")]
    format: Option<String>,

    /// Re-exec under bubblewrap with only the granted paths bind-mounted (docs/security.md, rule 7).
    #[arg(long)]
    sandbox: bool,

    /// Increase log verbosity. Repeat for more.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Inspect and revoke stored permission grants (docs/security.md, rule 6).
    Permissions {
        #[command(subcommand)]
        action: Option<PermissionsAction>,
    },

    /// Produce a standalone app from a `.hta` (docs/building.md).
    Build {
        document: PathBuf,
        /// Where to write the outputs.
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
        /// Skip the AppImage.
        #[arg(long)]
        no_appimage: bool,
        /// Skip the Flatpak manifest.
        #[arg(long)]
        no_flatpak: bool,
    },

    /// Emit TypeScript declarations for the bridge (docs/bridge.md).
    Types,

    /// Inspect a document's manifest without running it.
    Inspect { document: PathBuf },

    /// The cached remote modules from a document's import map (docs/document-format.md).
    Cache {
        #[command(subcommand)]
        action: Option<CacheAction>,
    },

    /// Register the `.desktop` entry, MIME type, and icons (docs/building.md).
    Install {
        /// Install system-wide instead of for this user.
        #[arg(long)]
        system: bool,
    },

    /// Undo `install`.
    Uninstall {
        #[arg(long)]
        system: bool,
    },
}

#[derive(Subcommand, Debug)]
enum PermissionsAction {
    /// List every stored decision.
    List,
    /// Grant a document everything its manifest asks for, without a prompt.
    ///
    /// This is the deliberate, explicit path for the cases where a sheet cannot be shown: a
    /// headless document in a pipeline, or a layer-shell bar with nothing on screen to attach a
    /// dialog to (the open questions, open question 6).
    Allow {
        document: PathBuf,
        /// Print what would be granted and stop.
        #[arg(long)]
        dry_run: bool,
    },
    /// Record a refusal for a document, so it runs as a plain page without asking.
    Deny { document: PathBuf },
    /// Forget the decisions for one document.
    Revoke { document: PathBuf },
    /// Forget everything.
    RevokeAll,
}

#[derive(Subcommand, Debug)]
enum CacheAction {
    /// Show what is cached.
    Info,
    /// Empty the cache.
    Purge,
}

fn main() -> ExitCode {
    // The launch modes: "A stapled binary (./tool) runs its embedded document. The launcher never appears."
    // Checked before argument parsing, because a built app's own flags are its business, not ours.
    if let Some(document) = htmlapp_build::extract_from_current_exe() {
        return match run_stapled(document) {
            Ok(code) => ExitCode::from(code as u8),
            Err(error) => {
                eprintln!("htmlapp: {error:#}");
                ExitCode::FAILURE
            }
        };
    }

    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match dispatch(cli) {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("htmlapp: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(verbosity: u8) {
    let default = match verbosity {
        0 => "htmlapp=warn",
        1 => "htmlapp=info,htmlapp_runtime=info,htmlapp_engine=info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("HTMLAPP_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// The Tokio runtime the bridge dispatches on. GPUI owns the main thread, so this runs beside it.
fn tokio_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("htmlapp-bridge")
        .build()
        .context("could not start the async runtime")
}

fn run_stapled(document: Vec<u8>) -> Result<i32> {
    init_tracing(0);
    let document = Document::from_bytes(&document).context("the stapled document is not valid")?;
    let config = htmlapp_runtime::Config::load();
    let session = Session::from_document(
        document,
        SessionOptions {
            devtools: config.devtools,
            private: !config.recents,
            sandbox: config.sandbox,
            ..Default::default()
        },
    )?;
    let runtime = tokio_runtime()?;
    Ok(htmlapp_runtime::app::run(session, runtime)?)
}

fn dispatch(cli: Cli) -> Result<i32> {
    match cli.command {
        Some(Command::Types) => {
            print!("{}", htmlapp_bridge::emit_typescript());
            Ok(0)
        }
        Some(Command::Inspect { document }) => inspect(&document),
        Some(Command::Permissions { action }) => permissions(action),
        Some(Command::Cache { action }) => cache(action),
        Some(Command::Install { system }) => install(system),
        Some(Command::Uninstall { system }) => uninstall(system),
        Some(Command::Build {
            document,
            out,
            no_appimage,
            no_flatpak,
        }) => build(&document, out, !no_appimage, !no_flatpak),

        None => match cli.document.clone() {
            Some(path) => run_document(&path, &cli),
            // The launch modes: no document is a normal launch.
            None => {
                // The open questions Q9: `launcher = false` makes a bare invocation print help instead.
                let config = htmlapp_runtime::Config::load();
                if !config.launcher && !cli.open && !cli.permissions_ui {
                    use clap::CommandFactory as _;
                    Cli::command().print_help()?;
                    println!(
                        "\nThe launcher window is disabled by `launcher = false` in {}.",
                        htmlapp_runtime::Config::default_path().display()
                    );
                    return Ok(0);
                }
                delegate::run_launcher(cli.open, cli.permissions_ui)?;
                Ok(0)
            }
        },
    }
}

fn run_document(path: &PathBuf, cli: &Cli) -> Result<i32> {
    let config = htmlapp_runtime::Config::load();

    let options = SessionOptions {
        headless: cli.headless,
        devtools: cli.devtools || config.devtools,
        private: cli.private || !config.recents,
        force_powerless: cli.no_permissions,
        format: cli.format.clone(),
        sandbox: cli.sandbox || config.sandbox,
    };

    let session = Session::load(path, options)
        .with_context(|| format!("could not open {}", path.display()))?;

    // The security model, rule 7. Done *after* the manifest is read, so the jail is built from what this
    // document actually asked for — and before any of it runs.
    if session.options.sandbox && !htmlapp_runtime::sandbox::is_sandboxed() {
        if !htmlapp_runtime::sandbox::is_available() {
            anyhow::bail!("--sandbox needs bubblewrap; install `bubblewrap` and try again");
        }
        // Never returns on success.
        htmlapp_runtime::sandbox::reexec(session.granted.as_ref(), Some(path))
            .context("could not re-exec under bubblewrap")?;
    }
    let runtime = tokio_runtime()?;
    Ok(htmlapp_runtime::app::run(session, runtime)?)
}

/// `htmlapp inspect` — read a manifest without running anything.
fn inspect(path: &PathBuf) -> Result<i32> {
    let document =
        Document::load(path).with_context(|| format!("could not read {}", path.display()))?;
    let manifest = &document.manifest;

    println!("{}", manifest.display_name());
    println!("  path      {}", path.display());
    println!("  sha256    {}", document.hash);
    if let Some(id) = &manifest.id {
        println!("  id        {id}");
    }
    if let Some(version) = &manifest.version {
        println!("  version   {version}");
    }
    println!("  mode      {}", manifest.window.mode.as_str());
    println!(
        "  window    {}×{}",
        manifest.window.width, manifest.window.height
    );

    if !document.has_manifest {
        println!("\n  No manifest. This document runs as a plain page with no native APIs.");
        return Ok(0);
    }

    match &manifest.permissions {
        None => println!("\n  Requests no permissions."),
        Some(permissions) => {
            let modules: Vec<&str> = permissions.granted_modules().into_iter().collect();
            println!("\n  Requests: {}", modules.join(", "));
            for description in permissions.describe() {
                println!(
                    "    [{:?}] {} — {}",
                    description.risk, description.summary, description.detail
                );
            }
        }
    }

    if !manifest.imports.is_empty() {
        println!("\n  Imports:");
        for (name, spec) in &manifest.imports {
            println!("    {name}  {}  {}", spec.url, spec.integrity);
        }
    }

    // Say whether it would prompt, so this is usable as a pre-flight check.
    let store = ConsentStore::load_default()?;
    let decision = store.decide(
        &document.hash,
        document.source.as_deref(),
        manifest.permissions.as_ref(),
    );
    println!("\n  Consent:  {}", describe_decision(&decision));
    Ok(0)
}

fn describe_decision(decision: &htmlapp_caps::ConsentDecision) -> String {
    use htmlapp_caps::ConsentDecision as D;
    match decision {
        D::NotRequired => "not required — this document asks for nothing".into(),
        D::AlreadyGranted(_) => "already granted for this exact file".into(),
        D::AlreadyDenied(_) => "previously refused; it will run as a plain page".into(),
        D::NeedsPrompt { previous, diff } => {
            if previous.is_some() {
                format!(
                    "will re-prompt — the file changed (added: {}, changed: {}, removed: {})",
                    or_none(&diff.added),
                    or_none(&diff.changed),
                    or_none(&diff.removed)
                )
            } else {
                "will prompt on first run".into()
            }
        }
    }
}

fn or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items.join(", ")
    }
}

/// `htmlapp permissions` (docs/security.md, rule 6).
fn permissions(action: Option<PermissionsAction>) -> Result<i32> {
    let mut store = ConsentStore::load_default()?;

    match action.unwrap_or(PermissionsAction::List) {
        PermissionsAction::List => {
            if store.records.is_empty() {
                println!("No stored permission decisions.");
                println!("Store: {}", ConsentStore::default_path().display());
                return Ok(0);
            }
            for record in &store.records {
                let modules: Vec<&str> = record.permissions.granted_modules().into_iter().collect();
                println!(
                    "{} {}",
                    if record.granted { "allowed" } else { "refused" },
                    record.name.as_deref().unwrap_or("(unnamed)")
                );
                println!(
                    "  path      {}",
                    record
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(embedded)".into())
                );
                println!("  sha256    {}", record.hash);
                println!("  grants    {}", or_none_str(&modules));
                println!();
            }
            println!("Store: {}", ConsentStore::default_path().display());
            Ok(0)
        }

        PermissionsAction::Allow { document, dry_run } => {
            let loaded = Document::load(&document)
                .with_context(|| format!("could not read {}", document.display()))?;

            let Some(requested) = loaded.manifest.permissions.clone() else {
                println!("{} asks for nothing; there is nothing to grant.", document.display());
                return Ok(0);
            };

            // Show what is being granted even in the non-dry-run case: an unattended grant that
            // prints nothing would be exactly the kind of silent trust the failure to avoid warns about.
            println!("{}", loaded.manifest.display_name());
            println!("  path    {}", document.display());
            println!("  sha256  {}", loaded.hash);
            for description in requested.describe() {
                println!(
                    "    [{:?}] {} — {}",
                    description.risk, description.summary, description.detail
                );
            }

            if dry_run {
                println!("\nDry run: nothing was stored.");
                return Ok(0);
            }

            store.record(
                &loaded.hash,
                Some(document.as_path()),
                loaded.manifest.name.as_deref(),
                loaded.manifest.id.as_deref(),
                &requested,
                true,
            );
            store.save_default()?;
            println!("\nGranted. This is pinned to the file's current contents; editing it asks again.");
            Ok(0)
        }

        PermissionsAction::Deny { document } => {
            let loaded = Document::load(&document)
                .with_context(|| format!("could not read {}", document.display()))?;
            let requested = loaded.manifest.permissions.clone().unwrap_or_default();
            store.record(
                &loaded.hash,
                Some(document.as_path()),
                loaded.manifest.name.as_deref(),
                loaded.manifest.id.as_deref(),
                &requested,
                false,
            );
            store.save_default()?;
            println!("Refused. {} will run as a plain page.", document.display());
            Ok(0)
        }

        PermissionsAction::Revoke { document } => {
            // Revoke by path *and* by the current content hash, so both a moved file and an edited
            // one are covered.
            let canonical = document.canonicalize().unwrap_or(document.clone());
            let mut removed = store.revoke_path(&canonical);
            if let Ok(loaded) = Document::load(&canonical) {
                removed += store.revoke_hash(&loaded.hash);
            }
            store.save_default()?;
            println!(
                "Revoked {removed} decision(s) for {}.",
                canonical.display()
            );
            Ok(0)
        }

        PermissionsAction::RevokeAll => {
            let removed = store.revoke_all();
            store.save_default()?;
            println!("Revoked {removed} decision(s).");
            Ok(0)
        }
    }
}

fn or_none_str(items: &[&str]) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items.join(", ")
    }
}

/// `htmlapp cache` (the import map, the open questions, open question 5).
fn cache(action: Option<CacheAction>) -> Result<i32> {
    let cache = htmlapp_engine::ModuleCache::default_cache();
    match action.unwrap_or(CacheAction::Info) {
        CacheAction::Info => {
            let bytes = cache.size();
            println!("Module cache: {}", cache.root().display());
            println!("Size:         {:.1} KiB", bytes as f64 / 1024.0);
            Ok(0)
        }
        CacheAction::Purge => {
            let removed = cache.purge()?;
            println!("Removed {removed} cached module(s).");
            Ok(0)
        }
    }
}

/// `htmlapp install` (docs/building.md).
fn install(system: bool) -> Result<i32> {
    let paths = if system {
        htmlapp_build::InstallPaths::system()
    } else {
        htmlapp_build::InstallPaths::user()
            .context("could not determine the user's data directory")?
    };

    let exe = std::env::current_exe().context("could not locate the running binary")?;
    let report = htmlapp_build::install(&paths, &exe)?;

    for path in &report.written {
        println!("wrote {}", path.display());
    }
    // Packaging and distribution: "An install that leaves double-click broken is a failed install."
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if report.warnings.is_empty() {
        println!("\nDouble-clicking a .hta file will now open it with HTML App.");
    }
    Ok(if report.warnings.is_empty() { 0 } else { 1 })
}

fn uninstall(system: bool) -> Result<i32> {
    let paths = if system {
        htmlapp_build::InstallPaths::system()
    } else {
        htmlapp_build::InstallPaths::user()
            .context("could not determine the user's data directory")?
    };
    for path in htmlapp_build::uninstall(&paths) {
        println!("removed {}", path.display());
    }
    Ok(0)
}

/// `htmlapp build` (docs/building.md).
fn build(document: &PathBuf, out: PathBuf, appimage: bool, flatpak: bool) -> Result<i32> {
    let options = htmlapp_build::BuildOptions {
        output_dir: out,
        appimage,
        flatpak,
    };
    let output = htmlapp_build::build(document, &options)?;

    for (label, path) in [
        ("binary  ", &output.binary),
        ("desktop ", &output.desktop_entry),
        ("icon    ", &output.icon),
        ("appdir  ", &output.appdir),
        ("appimage", &output.appimage),
        ("flatpak ", &output.flatpak_manifest),
    ] {
        if let Some(path) = path {
            println!("{label}  {}", path.display());
        }
    }
    for skipped in &output.skipped {
        eprintln!("note: {skipped}");
    }
    Ok(0)
}

/// Used by the launcher's recents list; kept here so the CLI and the UI agree.
pub fn recents() -> Recents {
    Recents::load_default().unwrap_or_default()
}
