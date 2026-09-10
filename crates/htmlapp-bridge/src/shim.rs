//! Generating the injected shim (docs/bridge.md).
//!
//! The shim is built per-document, because which modules exist on the `htmlapp` global is a
//! function of what that document's manifest was granted. Building it here rather than shipping
//! one static file is what makes the API catalog's "the property does not exist" literally true.

use std::collections::BTreeMap;

use htmlapp_caps::Permissions;
use serde_json::{Value, json};

use crate::catalog::{self, MethodKind};

/// The shim source with its placeholders still in it.
const TEMPLATE: &str = include_str!("../js/shim.js");

/// Everything that varies between documents.
#[derive(Debug, Clone)]
pub struct ShimConfig {
    /// Reported to the page as `htmlapp.version`.
    pub version: String,
    /// Which modules to inject. Anything absent is genuinely absent.
    pub modules: Vec<String>,
    /// The frozen copy of the grant that the page can read back.
    pub permissions: Value,
    /// Headless mode: adds `stdin`, `stdout`, `stderr`, and `exit`.
    pub headless: bool,
    /// Headless mode's `--format`, surfaced to the page as `htmlapp.format`.
    pub format: Option<String>,
}

impl ShimConfig {
    /// Derive the shim configuration from a document's granted permissions.
    ///
    /// `granted` is `None` for a document with no manifest — the security model, rule 1 — which yields a shim
    /// with the transport present but no capability modules at all.
    pub fn for_permissions(
        version: impl Into<String>,
        granted: Option<&Permissions>,
        headless: bool,
    ) -> Self {
        let modules = granted
            .map(|p| {
                p.granted_modules()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let permissions = granted
            .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
            .unwrap_or(Value::Null);

        Self {
            version: version.into(),
            modules,
            permissions,
            headless,
            format: None,
        }
    }

    /// Set the `--format` hint (docs/bridge.md).
    pub fn with_format(mut self, format: Option<String>) -> Self {
        self.format = format;
        self
    }
}

/// Render the shim for one document.
pub fn render(config: &ShimConfig) -> String {
    let modules = module_table(&config.modules);

    TEMPLATE
        .replace("\"__HTMLAPP_VERSION__\"", &json_string(&config.version))
        .replace("__HTMLAPP_PERMISSIONS__", &config.permissions.to_string())
        .replace("__HTMLAPP_MODULES__", &modules.to_string())
        .replace("__HTMLAPP_HEADLESS__", if config.headless { "true" } else { "false" })
        .replace(
            "\"__HTMLAPP_FORMAT__\"",
            &json_string(config.format.as_deref().unwrap_or_default()),
        )
}

/// Build `{ "fs": { "read": "invoke", "watch": "stream", ... }, ... }` for the granted modules.
///
/// The shim needs to know each method's *shape* — a `stream` method returns an async iterable, an
/// `invoke` method returns a promise — and that comes from the same catalog that generates the
/// TypeScript, so the two can never disagree about which is which.
fn module_table(granted: &[String]) -> Value {
    let mut table = BTreeMap::new();

    for name in granted {
        let Some(module) = catalog::module(name) else {
            tracing::warn!(module = %name, "granted module is not in the API catalog");
            continue;
        };
        let methods: BTreeMap<&str, &str> = module
            .methods
            .iter()
            .map(|m| {
                (
                    m.name,
                    match m.kind {
                        MethodKind::Invoke => "invoke",
                        MethodKind::Stream => "stream",
                    },
                )
            })
            .collect();
        table.insert(module.name, methods);
    }

    json!(table)
}

/// Encode a Rust string as a JavaScript string literal that is safe in every context the runtime
/// puts one in.
///
/// `serde_json` alone is not enough. It escapes quotes and backslashes, so the literal cannot be
/// broken out of directly, but it passes `<` and `>` through verbatim — which means a payload
/// containing `</script>` would terminate an enclosing HTML script element early. It also passes
/// through U+2028 and U+2029, which JSON treats as ordinary characters but older JS parsers treat
/// as line terminators inside a string literal.
///
/// Escaping all five as `\uXXXX` keeps the value byte-identical after parsing while making the
/// literal inert whether it is handed to `evaluate_script` or embedded in a document.
fn json_string(value: &str) -> String {
    let encoded = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
    let mut out = String::with_capacity(encoded.len());
    for c in encoded.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other => out.push(other),
        }
    }
    out
}

/// Wrap a host→page message in the call that delivers it.
///
/// The host evaluates this string inside the page. The payload is embedded as a JSON *string
/// literal* and parsed on the other side rather than being interpolated as source, so a filename
/// containing a quote cannot break out into executable script.
pub fn dispatch_script(message_json: &str) -> String {
    format!(
        "window.__htmlapp_dispatch && window.__htmlapp_dispatch({});",
        json_string(message_json)
    )
}
