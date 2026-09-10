//! `htmlapp types > htmlapp.d.ts` (docs/bridge.md).
//!
//! Generated from the same catalog the shim is built from, so completion in the author's editor
//! describes the runtime they actually get. Single-file authors have no build step and therefore
//! no other way to discover this surface.

use std::fmt::Write as _;

use crate::catalog::{self, ApiModule, MethodKind, Tier};

/// Hand-written types the generated module interfaces refer to.
///
/// These are structural, not generated, because they describe payload shapes rather than the call
/// surface — there is no second source for them to drift from.
const PRELUDE: &str = r#"
// --- shared payload types ---

export type Encoding = "utf8" | "binary";

export interface FileStat {
  path: string;
  size: number;
  isFile: boolean;
  isDirectory: boolean;
  isSymlink: boolean;
  /** Milliseconds since the Unix epoch. */
  modified: number | null;
  created: number | null;
  /** Unix permission bits. */
  mode: number;
  readonly: boolean;
}

export interface DirEntry {
  name: string;
  path: string;
  isFile: boolean;
  isDirectory: boolean;
  isSymlink: boolean;
}

export type WatchEvent =
  | { kind: "created"; paths: string[] }
  | { kind: "modified"; paths: string[] }
  | { kind: "removed"; paths: string[] }
  | { kind: "renamed"; paths: string[] };

export interface ProcessOutput {
  code: number | null;
  stdout: string;
  stderr: string;
}

export interface ProcessInfo {
  pid: number;
  program: string;
  args: string[];
  running: boolean;
}

export type ProcessEvent =
  | { kind: "started"; pid: number }
  | { kind: "stdout"; data: string }
  | { kind: "stderr"; data: string }
  | { kind: "exit"; code: number | null; signal: string | null };

export interface FileFilter {
  name: string;
  extensions: string[];
}

export interface HttpRequest {
  url: string;
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  timeoutMs?: number;
}

export interface HttpResponse {
  status: number;
  statusText: string;
  headers: Record<string, string>;
  body: string;
  url: string;
}

export type SqlValue = string | number | boolean | null;
export type SqlRow = Record<string, SqlValue>;
export interface SqlStatement { sql: string; params?: SqlValue[] }
export interface SqlExecuteResult { rowsAffected: number; lastInsertId: number | null }

export type DbusBus = "session" | "system";

export interface DbusCall {
  bus?: DbusBus;
  destination: string;
  path: string;
  iface: string;
  member: string;
  args?: unknown[];
  signature?: string;
}

export interface DbusSignal {
  sender: string | null;
  path: string;
  iface: string;
  member: string;
  args: unknown[];
}

export interface MenuItem {
  id?: string;
  label?: string;
  /** Renders a horizontal rule; `label` and `id` are ignored. */
  separator?: boolean;
  enabled?: boolean;
  checked?: boolean;
  accelerator?: string;
  submenu?: MenuItem[];
}

export interface Command {
  id: string;
  title: string;
  subtitle?: string;
  accelerator?: string;
  category?: string;
}

export interface NotificationSpec {
  title: string;
  body?: string;
  icon?: string;
  urgency?: "low" | "normal" | "critical";
  timeoutMs?: number;
  actions?: { id: string; label: string }[];
  /** Replaces an earlier notification in place. */
  replaceId?: number;
  progress?: number;
}

export interface Rect { x: number; y: number; width: number; height: number }
export interface Color { red: number; green: number; blue: number }
export interface Location { latitude: number; longitude: number; accuracy: number }
export interface ScreenCastFrame { nodeId: number; width: number; height: number }

export type Anchor = "top" | "bottom" | "left" | "right";
export type LayerName = "background" | "bottom" | "top" | "overlay";

export interface OutputInfo {
  name: string;
  description: string | null;
  width: number;
  height: number;
  scale: number;
  primary: boolean;
}

export interface ShortcutInfo { id: string; accelerator: string; description: string | null }

export interface UnitStatus {
  unit: string;
  activeState: string;
  subState: string;
  loadState: string;
  description: string | null;
}

export interface JournalEntry {
  timestamp: number;
  message: string;
  priority: number;
  unit: string | null;
  fields: Record<string, string>;
}

export interface DeviceInfo {
  path: string;
  subsystem: string | null;
  devtype: string | null;
  properties: Record<string, string>;
}

export interface DeviceEvent { action: string; device: DeviceInfo }

export type ServerEvent =
  | { kind: "connection"; handle: number; peer: string }
  | { kind: "request"; requestId: number; method: string; path: string; headers: Record<string, string>; body: string }
  | { kind: "message"; handle: number; data: string }
  | { kind: "close"; handle: number };

export interface SerialPortInfo { port: string; kind: string; manufacturer: string | null; product: string | null }
export interface UsbDeviceInfo { vendorId: number; productId: number; manufacturer: string | null; product: string | null; serial: string | null }
export interface BluetoothAdapter { address: string; name: string; powered: boolean }
export interface BluetoothDevice { address: string; name: string | null; rssi: number | null; paired: boolean }

export interface OsInfo {
  platform: string;
  distro: string | null;
  kernel: string;
  arch: string;
  hostname: string;
  sessionType: "wayland" | "x11" | "unknown";
  compositor: string | null;
}

export interface XdgPaths {
  home: string;
  config: string;
  data: string;
  cache: string;
  runtime: string | null;
  documents: string | null;
  downloads: string | null;
}

export interface BatteryInfo { percentage: number; charging: boolean; secondsRemaining: number | null }
export interface ThemeInfo { colorScheme: "light" | "dark"; accent: string | null }

/** A native view placed inline in HTML layout (docs/bridge.md). */
export interface NativeView {
  readonly id: string;
  write(data: string): Promise<void>;
  call(method: string, params?: unknown): Promise<unknown>;
  set(props: Record<string, unknown>): Promise<void>;
  on(event: string, handler: (payload: any) => void): () => void;
}

/** An async iterable that can also be cancelled explicitly (docs/bridge.md). */
export interface HtmlAppStream<T> extends AsyncIterable<T> {
  cancel(): void;
}
"#;

/// Emit a complete `htmlapp.d.ts`.
pub fn emit() -> String {
    let mut out = String::new();

    out.push_str(
        "// Generated by `htmlapp types`. Do not edit.\n\
         //\n\
         // Type declarations for the HTML App bridge. Reference them from a single-file app with:\n\
         //   /// <reference path=\"./htmlapp.d.ts\" />\n\
         //\n\
         // A module only exists at runtime if the document's manifest was granted it, so every\n\
         // module is optional here. That is deliberate: it makes the compiler ask you to feature\n\
         // test exactly where the runtime would otherwise throw.\n\n",
    );

    out.push_str(PRELUDE.trim_start());
    out.push('\n');

    let mut current_tier: Option<Tier> = None;
    for module in catalog::MODULES {
        if current_tier != Some(module.tier) {
            let _ = write!(out, "\n// --- {} ---\n", module.tier.label());
            current_tier = Some(module.tier);
        }
        emit_module(&mut out, module);
    }

    emit_event_map(&mut out);
    emit_global(&mut out);

    out
}

fn emit_module(out: &mut String, module: &ApiModule) {
    let _ = write!(out, "\n/**\n * {}\n */\n", wrap_doc(module.summary, 76));
    let _ = writeln!(out, "export interface {} {{", interface_name(module.name));

    for method in module.methods {
        let params = if method.params == "void" {
            String::new()
        } else {
            format!("params: {}", method.params)
        };
        let returns = match method.kind {
            MethodKind::Invoke => format!("Promise<{}>", method.returns),
            MethodKind::Stream => format!("HtmlAppStream<{}>", method.returns),
        };
        let _ = writeln!(out, "  /** {} */", method.summary);
        let _ = writeln!(out, "  {}({}): {};", method.name, params, returns);
    }

    let _ = writeln!(out, "}}");
}

/// A typed map of every event name to its payload, so `htmlapp.on` can be overloaded precisely.
fn emit_event_map(out: &mut String) {
    out.push_str("\n// --- events (docs/bridge.md) ---\n\nexport interface HtmlAppEventMap {\n");
    for event in catalog::AMBIENT_EVENTS {
        let _ = writeln!(out, "  /** {} */", event.summary);
        let _ = writeln!(out, "  {:?}: {};", event.name, event.payload);
    }
    for module in catalog::MODULES {
        for event in module.events {
            let _ = writeln!(out, "  /** {} */", event.summary);
            let _ = writeln!(out, "  {:?}: {};", event.name, event.payload);
        }
    }
    out.push_str("}\n");
}

fn emit_global(out: &mut String) {
    out.push_str(
        "\n// --- the global (docs/bridge.md) ---\n\n\
         export interface HtmlApp {\n\
        \x20 /** Call a native method and await one result. */\n\
        \x20 invoke<T = unknown>(method: string, params?: unknown): Promise<T>;\n\
        \x20 /** Call a native method that yields many values over time. */\n\
        \x20 stream<T = unknown>(method: string, params?: unknown): HtmlAppStream<T>;\n\
        \x20 /** Subscribe to an event. Returns an unsubscribe function. */\n\
        \x20 on<K extends keyof HtmlAppEventMap>(event: K, handler: (payload: HtmlAppEventMap[K]) => void): () => void;\n\
        \x20 on(event: string, handler: (payload: any) => void): () => void;\n\
        \x20 /** Subscribe to the next occurrence of an event only. */\n\
        \x20 once<K extends keyof HtmlAppEventMap>(event: K, handler: (payload: HtmlAppEventMap[K]) => void): () => void;\n\
        \x20 /** The runtime version. */\n\
        \x20 readonly version: string;\n\
        \x20 /** A frozen copy of what this document was granted. */\n\
        \x20 readonly permissions: Readonly<Record<string, unknown>> | null;\n\
        \x20 /** Address a native view by the id of its <htmlapp-view> element (docs/bridge.md). */\n\
        \x20 view(id: string): NativeView;\n",
    );

    for module in catalog::MODULES {
        let _ = writeln!(
            out,
            "  /** {} Present only if the manifest granted it. */\n  readonly {}?: {};",
            first_sentence(module.summary),
            module.name,
            interface_name(module.name)
        );
    }

    out.push_str(
        "\n  // Headless mode only.\n\
        \x20 readonly stdin?: { read(): Promise<string>; lines(): HtmlAppStream<string> };\n\
        \x20 readonly stdout?: { write(data: string): Promise<void>; writeLine(data: string): Promise<void> };\n\
        \x20 readonly stderr?: { write(data: string): Promise<void> };\n\
        \x20 exit?(code?: number): Promise<void>;\n\
         }\n\n\
         declare global {\n\
        \x20 const htmlapp: HtmlApp;\n\
        \x20 interface Window { htmlapp: HtmlApp }\n\
         }\n\n\
         export {};\n",
    );
}

/// `fs` → `FsApi`, `dbus` → `DbusApi`.
fn interface_name(module: &str) -> String {
    let mut name = String::with_capacity(module.len() + 3);
    let mut capitalise = true;
    for c in module.chars() {
        if capitalise {
            name.extend(c.to_uppercase());
            capitalise = false;
        } else {
            name.push(c);
        }
    }
    name.push_str("Api");
    name
}

fn first_sentence(text: &str) -> String {
    let collapsed = collapse_whitespace(text);
    match collapsed.find(". ") {
        Some(i) => collapsed[..=i].to_string(),
        None => collapsed,
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Re-wrap a summary into a `*`-prefixed doc comment body.
fn wrap_doc(text: &str, width: usize) -> String {
    let words = collapse_whitespace(text);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in words.split(' ') {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.join("\n * ")
}
