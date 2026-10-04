//! MCP servers for πDesk's private Pi, through Pi's built-in MCP support.
//! πDesk owns one file, `<agent dir>/mcp.json`, which Pi reads as strict
//! JSON (no comments or trailing commas). Shared MCP files in the user's
//! real home (`~/.config/mcp/mcp.json`, `~/.agents/…`, `~/.pi/agent/…`) are
//! never loaded: Pi runs with a private home. They can be copied in, per
//! server, only when the user asks, and the backend re-reads them itself, so
//! their values never pass through the UI.

use crate::error::{AppError, AppResult};
use crate::util;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_SERVERS: usize = 100;
const MAX_SERVER_BYTES: usize = 64 * 1024;
const MAX_NAME_CHARS: usize = 64;
const MAX_FIELD_CHARS: usize = 4096;
const MAX_ERROR_TAIL: usize = 1200;
const MAX_STATUS_ERROR: usize = 600;
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(330);
const MAX_CHECK_OUTPUT: usize = 1024 * 1024;
const MAX_CHECK_ERROR: usize = 64 * 1024;
const EXPOSURES: &[&str] = &[
    "codemode",
    "codemode-deferred",
    "deferred",
    "direct",
    "hidden",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    /// `stdio` or `http`.
    pub transport: String,
    /// Command line or URL, for display.
    pub target: String,
    pub enabled: bool,
    /// `codemode`, `deferred`, `direct`, or `hidden`.
    pub exposure: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Has env values, headers, or an OAuth client secret (likely credentials).
    pub has_secrets: bool,
    /// Import sources only: why the entry cannot be copied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// Full entry for editing; omitted for shared files.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportSource {
    pub id: String,
    pub path: String,
    pub servers: Vec<McpServer>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAdapter {
    /// The package source declared in the private Pi's settings.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Server names in the adapter's own config file.
    pub servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOverview {
    pub path: String,
    pub servers: Vec<McpServer>,
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub import_sources: Vec<McpImportSource>,
    /// Set only while the old `pi-mcp-adapter` package is installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<McpAdapter>,
    /// `settings.json` disables `builtin:mcp` via an extensions entry.
    pub builtin_disabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSkipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportResult {
    pub copied: Vec<String>,
    pub skipped: Vec<McpSkipped>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub name: String,
    pub state: String,
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub servers: Vec<McpServerStatus>,
    pub errors: Vec<String>,
}

fn config_path(root: &Path) -> PathBuf {
    root.join("agent/mcp.json")
}

/// The shared MCP files πDesk can copy servers from, under the user's real
/// home. `pi` is the terminal Pi's own built-in MCP file.
fn import_paths() -> Vec<(&'static str, PathBuf)> {
    let home = util::home_dir();
    vec![
        ("config-mcp", home.join(".config/mcp/mcp.json")),
        ("agents", home.join(".agents/mcp.json")),
        ("agents-mcp", home.join(".agents/mcp/mcp.json")),
        ("pi", home.join(".pi/agent/mcp.json")),
    ]
}

fn display_path(path: &Path) -> String {
    let home = util::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Strip `//` and `/* */` comments and trailing commas outside strings.
/// Only foreign files (import sources, the old adapter's config) are read
/// this way; Pi itself reads πDesk's `mcp.json` strictly.
fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_string) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match (c, chars.get(i + 1)) {
            ('"', _) => {
                in_string = true;
                out.push(c);
                i += 1;
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            (',', _) => {
                let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
                if !matches!(next, Some('}') | Some(']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Parse an MCP file; `Ok(None)` when it does not exist. `jsonc` tolerates
/// comments and trailing commas, as foreign clients accept them.
fn read_file(path: &Path, jsonc: bool) -> AppResult<Option<(String, Map<String, Value>)>> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Err(AppError::new(format!(
            "{} is not a regular file.",
            display_path(path)
        )));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(AppError::new(format!(
            "{} is too large to read.",
            display_path(path)
        )));
    }
    let raw = std::fs::read_to_string(path)?;
    let value = match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value,
        Err(error) => {
            if jsonc {
                serde_json::from_str::<Value>(&strip_jsonc(&raw)).map_err(|error| {
                    AppError::new(format!("{} is not valid JSON: {error}", display_path(path)))
                })?
            } else {
                return Err(AppError::new(format!(
                    "{} is not valid JSON: {error}",
                    display_path(path)
                )));
            }
        }
    };
    match value {
        Value::Object(map) => Ok(Some((raw, map))),
        _ => Err(AppError::new(format!(
            "{} must contain a JSON object.",
            display_path(path)
        ))),
    }
}

fn servers_of(map: &Map<String, Value>) -> Map<String, Value> {
    match map.get("mcpServers") {
        Some(Value::Object(servers)) => servers.clone(),
        _ => Map::new(),
    }
}

/// Pi's alias `codemode-deferred` means codemode; anything unrecognized in a
/// hand-edited file is reported as the default rather than dropping the row.
fn exposure_of(config: &Value) -> String {
    match config.get("exposure").and_then(Value::as_str) {
        Some("deferred") => "deferred".into(),
        Some("direct") => "direct".into(),
        Some("hidden") => "hidden".into(),
        _ => "codemode".into(),
    }
}

fn view(name: &str, config: &Value, include_config: bool) -> McpServer {
    let text = |key: &str| config.get(key).and_then(Value::as_str).map(String::from);
    let (transport, target) = if let Some(command) = text("command") {
        let args: Vec<String> = config
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        (
            "stdio",
            std::iter::once(command)
                .chain(args)
                .collect::<Vec<_>>()
                .join(" "),
        )
    } else {
        ("http", text("url").unwrap_or_default())
    };
    let non_empty = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_object)
            .is_some_and(|map| !map.is_empty())
    };
    McpServer {
        name: name.to_string(),
        transport: transport.into(),
        target: target.chars().take(400).collect(),
        enabled: config.get("enabled") != Some(&Value::Bool(false)),
        exposure: exposure_of(config),
        description: text("description"),
        has_secrets: non_empty("env")
            || non_empty("headers")
            || config
                .get("oauth")
                .and_then(|oauth| oauth.get("clientSecret"))
                .is_some_and(|secret| !secret.is_null()),
        issue: None,
        config: if include_config {
            config.clone()
        } else {
            Value::Null
        },
    }
}

/// The exact package source that installs the old adapter, when declared.
fn declared_adapter(root: &Path) -> Option<String> {
    crate::pi_settings::read_packages(root)
        .unwrap_or_default()
        .into_iter()
        .find(|source| source == "npm:pi-mcp-adapter" || source.starts_with("npm:pi-mcp-adapter@"))
}

fn adapter(root: &Path) -> Option<McpAdapter> {
    let source = declared_adapter(root)?;
    let version = std::fs::read(root.join("agent/npm/node_modules/pi-mcp-adapter/package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|json| {
            json.get("version")
                .and_then(Value::as_str)
                .map(String::from)
        });
    let servers: Vec<String> = read_file(&root.join("agent/mcp-adapter.json"), true)
        .ok()
        .flatten()
        .map(|(_, map)| servers_of(&map).keys().cloned().collect())
        .unwrap_or_default();
    Some(McpAdapter {
        source,
        version,
        servers,
    })
}

fn builtin_disabled(root: &Path) -> bool {
    crate::pi_settings::read_object(&root.join("agent/settings.json"))
        .map(|settings| {
            settings
                .get("extensions")
                .and_then(Value::as_array)
                .is_some_and(|entries| entries.iter().any(is_disabling_builtin))
        })
        .unwrap_or(false)
}

fn is_disabling_builtin(entry: &Value) -> bool {
    matches!(entry.as_str(), Some("-builtin:mcp") | Some("!builtin:mcp"))
}

fn import_source(id: &'static str, path: &Path) -> Option<McpImportSource> {
    let (_, map) = read_file(path, true).ok().flatten()?;
    let servers: Vec<McpServer> = servers_of(&map)
        .iter()
        .take(MAX_SERVERS)
        .map(|(name, config)| {
            let (shown, issue) = match to_builtin(config) {
                Ok(converted) => match validate_server(name, &converted) {
                    Ok(()) => (converted, None),
                    Err(error) => (converted, Some(error.to_string())),
                },
                Err(reason) => (config.clone(), Some(reason)),
            };
            let mut server = view(name, &shown, false);
            server.issue = issue;
            server
        })
        .collect();
    (!servers.is_empty()).then(|| McpImportSource {
        id: id.into(),
        path: display_path(path),
        servers,
    })
}

pub fn overview(root: &Path) -> McpOverview {
    let path = config_path(root);
    let (raw, map, error) = match read_file(&path, false) {
        Ok(Some((raw, map))) => (raw, map, None),
        Ok(None) => (String::new(), Map::new(), None),
        Err(error) => (
            std::fs::read_to_string(&path).unwrap_or_default(),
            Map::new(),
            Some(error.to_string()),
        ),
    };
    let import_sources = import_paths()
        .into_iter()
        .filter_map(|(id, path)| import_source(id, &path))
        .collect();
    McpOverview {
        path: display_path(&path),
        servers: servers_of(&map)
            .iter()
            .take(MAX_SERVERS)
            .map(|(name, config)| view(name, config, true))
            .collect(),
        raw,
        error,
        import_sources,
        adapter: adapter(root),
        builtin_disabled: builtin_disabled(root),
    }
}

// ---------------------------------------------------------------------
// Validation and writes
// ---------------------------------------------------------------------

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Pi turns `-` into `_` when naming tools (`mcp__<name>__…`), so two servers
/// whose namespaces collide would shadow each other.
fn namespace(name: &str) -> String {
    name.replace('-', "_")
}

fn clashing_name(servers: &Map<String, Value>, name: &str, except: Option<&str>) -> Option<String> {
    let target = namespace(name);
    servers
        .keys()
        .find(|existing| {
            existing.as_str() != name
                && Some(existing.as_str()) != except
                && namespace(existing) == target
        })
        .cloned()
}

fn string_map(value: Option<&Value>, what: &str, entries: usize) -> AppResult<()> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(map)) if map.len() <= entries => {
            for (key, value) in map {
                if key.is_empty() || key.len() > 200 || !value.is_string() {
                    return Err(AppError::new(format!(
                        "Each {what} entry needs a name and a text value."
                    )));
                }
            }
            Ok(())
        }
        _ => Err(AppError::new(format!("{what} must be name/value pairs."))),
    }
}

/// Mirror Pi's per-server validation so the form cannot save something Pi
/// would skip with an error.
pub fn validate_server(name: &str, config: &Value) -> AppResult<()> {
    if !valid_name(name) {
        return Err(AppError::new(
            "Server names use letters, numbers, dashes, or underscores (up to 64).",
        ));
    }
    let Value::Object(map) = config else {
        return Err(AppError::new("A server entry must be a JSON object."));
    };
    if serde_json::to_vec(config)?.len() > MAX_SERVER_BYTES {
        return Err(AppError::new("This server entry is too large."));
    }
    let kinds = ["command", "url"]
        .iter()
        .filter(|key| map.get(**key).is_some_and(|value| !value.is_null()))
        .count();
    if kinds != 1 {
        return Err(AppError::new(
            "Give a server exactly one of a command or a URL.",
        ));
    }
    if let Some(command) = map.get("command") {
        if command
            .as_str()
            .is_none_or(|text| text.trim().is_empty() || text.len() > MAX_FIELD_CHARS)
        {
            return Err(AppError::new("Command must be non-empty text."));
        }
    }
    if let Some(cwd) = map.get("cwd") {
        if cwd
            .as_str()
            .is_none_or(|text| text.trim().is_empty() || text.len() > MAX_FIELD_CHARS)
        {
            return Err(AppError::new("cwd must be non-empty text."));
        }
    }
    match map.get("url") {
        None | Some(Value::Null) => {}
        Some(Value::String(url)) if valid_url(url) => {}
        Some(Value::String(_)) => {
            return Err(AppError::new(
                "URLs start with https:// (or http:// for local servers) and need a host.",
            ))
        }
        Some(_) => return Err(AppError::new("url must be text.")),
    }
    if let Some(args) = map.get("args") {
        let ok = args.as_array().is_some_and(|args| {
            args.len() <= 100
                && args
                    .iter()
                    .all(|arg| arg.as_str().is_some_and(|a| a.len() <= MAX_FIELD_CHARS))
        });
        if !ok {
            return Err(AppError::new("Arguments must be a list of text values."));
        }
    }
    string_map(map.get("env"), "Environment", 100)?;
    string_map(map.get("headers"), "Header", 100)?;
    if let Some(kind) = map.get("type").filter(|value| !value.is_null()) {
        match kind.as_str() {
            Some("stdio") | Some("http") | Some("streamable-http") => {}
            Some("sse") => {
                return Err(AppError::new(
                    "Legacy SSE transport is not supported; use the streamable HTTP URL.",
                ))
            }
            _ => {
                return Err(AppError::new(
                    "Server type must be stdio, http, or streamable-http.",
                ))
            }
        }
    }
    if let Some(exposure) = map.get("exposure").filter(|value| !value.is_null()) {
        if exposure
            .as_str()
            .is_none_or(|value| !EXPOSURES.contains(&value))
        {
            return Err(AppError::new(
                "Tools reach the model through codemode, deferred, direct, or hidden.",
            ));
        }
    }
    if let Some(tool_exposure) = map.get("toolExposure").filter(|value| !value.is_null()) {
        match tool_exposure {
            Value::Object(entries) if entries.len() <= 200 => {
                for value in entries.values() {
                    if value
                        .as_str()
                        .is_none_or(|value| !EXPOSURES.contains(&value))
                    {
                        return Err(AppError::new(
                            "Each toolExposure value must be codemode, deferred, direct, or hidden.",
                        ));
                    }
                }
            }
            _ => {
                return Err(AppError::new(
                    "toolExposure must be a map of tool names to exposures.",
                ))
            }
        }
    }
    if let Some(enabled) = map.get("enabled").filter(|value| !value.is_null()) {
        if !enabled.is_boolean() {
            return Err(AppError::new("enabled must be true or false."));
        }
    }
    if let Some(description) = map.get("description").filter(|value| !value.is_null()) {
        if description
            .as_str()
            .is_none_or(|text| text.chars().count() > 1000)
        {
            return Err(AppError::new(
                "description must be text up to 1000 characters.",
            ));
        }
    }
    if let Some(timeout) = map.get("timeout").filter(|value| !value.is_null()) {
        if !timeout.as_f64().is_some_and(|value| value > 0.0) {
            return Err(AppError::new(
                "timeout must be a positive number of seconds.",
            ));
        }
    }
    match map.get("oauth") {
        None | Some(Value::Null) => {}
        Some(oauth) if oauth.is_object() => {}
        Some(_) => return Err(AppError::new("OAuth settings must be an object.")),
    }
    match map.get("auth").filter(|value| !value.is_null()) {
        None => {}
        Some(auth) => {
            let provider = auth
                .get("provider")
                .and_then(Value::as_str)
                .filter(|provider| !provider.is_empty());
            if provider.is_none() {
                return Err(AppError::new(
                    "Sign-in needs an auth object with a provider name.",
                ));
            }
            let url = map.get("url").and_then(Value::as_str).unwrap_or_default();
            if !(url.starts_with("https://") || is_loopback(url)) {
                return Err(AppError::new(
                    "Signed-in servers need an https:// URL (or a loopback address).",
                ));
            }
        }
    }
    Ok(())
}

/// http(s) only, with a non-empty host. Pi never expands `${VAR}` in URLs.
fn valid_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.contains('$')
}

fn is_loopback(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or("");
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    host == "localhost"
        || host.starts_with("localhost:")
        || host == "127.0.0.1"
        || host.starts_with("127.0.0.1:")
        || host == "[::1]"
        || host.starts_with("[::1]:")
}

/// Read, change, and atomically rewrite the private mcp.json (strict JSON).
fn update(
    root: &Path,
    change: impl FnOnce(&mut Map<String, Value>) -> AppResult<()>,
) -> AppResult<()> {
    util::ensure_private_directory(root, &root.join("agent"))?;
    let path = config_path(root);
    let mut map = match read_file(&path, false)? {
        Some((_, map)) => map,
        None => Map::new(),
    };
    change(&mut map)?;
    crate::pi_settings::write_object(&path, map)
}

fn servers_mut(map: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let entry = map
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("object")
}

pub fn save_server(
    root: &Path,
    original: Option<&str>,
    name: &str,
    config: Value,
) -> AppResult<()> {
    validate_server(name, &config)?;
    update(root, |map| {
        let servers = servers_mut(map);
        if original != Some(name) && servers.contains_key(name) {
            return Err(AppError::new(format!(
                "A server named {name} already exists."
            )));
        }
        if let Some(other) = clashing_name(servers, name, original) {
            return Err(AppError::new(format!(
                "{name} clashes with {other}: Pi turns - into _ in tool names, so those namespaces would collide."
            )));
        }
        if servers.len() >= MAX_SERVERS && !servers.contains_key(original.unwrap_or(name)) {
            return Err(AppError::new("πDesk supports up to 100 MCP servers."));
        }
        if let Some(original) = original.filter(|original| *original != name) {
            servers.remove(original);
        }
        servers.insert(name.to_string(), config);
        Ok(())
    })
}

pub fn remove_server(root: &Path, name: &str) -> AppResult<()> {
    update(root, |map| {
        servers_mut(map)
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| AppError::new(format!("No server named {name}.")))
    })
}

pub fn set_enabled(root: &Path, name: &str, enabled: bool) -> AppResult<()> {
    update(root, |map| {
        let server = servers_mut(map)
            .get_mut(name)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| AppError::new(format!("No server named {name}.")))?;
        if enabled {
            server.remove("enabled");
        } else {
            server.insert("enabled".into(), Value::Bool(false));
        }
        Ok(())
    })
}

/// Remove `-builtin:mcp` / `!builtin:mcp` from `settings.json`, keeping every
/// other key. Pi loads the built-in MCP unless one of those entries is present.
pub fn enable_builtin(root: &Path) -> AppResult<()> {
    let path = root.join("agent/settings.json");
    let mut settings = crate::pi_settings::read_object(&path)?;
    let Some(Value::Array(entries)) = settings.get("extensions").cloned() else {
        return Ok(());
    };
    let kept: Vec<Value> = entries
        .iter()
        .filter(|entry| !is_disabling_builtin(entry))
        .cloned()
        .collect();
    if kept.len() == entries.len() {
        return Ok(());
    }
    if kept.is_empty() {
        settings.remove("extensions");
    } else {
        settings.insert("extensions".into(), Value::Array(kept));
    }
    util::ensure_private_directory(root, &root.join("agent"))?;
    crate::pi_settings::write_object(&path, settings)
}

/// Save hand-edited text as-is once it parses as a strict JSON object whose
/// `mcpServers`, if present, is an object of valid, non-clashing servers.
pub fn save_raw(root: &Path, text: &str) -> AppResult<()> {
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(AppError::new("mcp.json is too large."));
    }
    let value: Value = serde_json::from_str(text).map_err(|error| {
        AppError::new(format!(
            "mcp.json is not valid JSON (Pi reads it strictly, without comments): {error}"
        ))
    })?;
    let Value::Object(map) = &value else {
        return Err(AppError::new("mcp.json must contain a JSON object."));
    };
    match map.get("mcpServers") {
        None => {}
        Some(Value::Object(servers)) => {
            if servers.len() > MAX_SERVERS {
                return Err(AppError::new("πDesk supports up to 100 MCP servers."));
            }
            let mut seen: HashMap<String, &str> = HashMap::new();
            for (name, config) in servers {
                validate_server(name, config)
                    .map_err(|error| AppError::new(format!("{name}: {error}")))?;
                if let Some(other) = seen.insert(namespace(name), name) {
                    return Err(AppError::new(format!(
                        "{name} clashes with {other}: Pi turns - into _ in tool names, so those namespaces would collide."
                    )));
                }
            }
        }
        Some(_) => return Err(AppError::new("mcpServers must be an object of servers.")),
    }
    if map
        .get("autoEnableCodemode")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(AppError::new("autoEnableCodemode must be true or false."));
    }
    util::ensure_private_directory(root, &root.join("agent"))?;
    let mut bytes = text.as_bytes().to_vec();
    if !text.ends_with('\n') {
        bytes.push(b'\n');
    }
    crate::pi_settings::write_bytes(&config_path(root), &bytes)
}

/// Convert a foreign MCP entry (the old adapter's format, or another client's
/// file) into Pi's built-in shape. Unknown keys are dropped; anything Pi
/// cannot represent becomes an error.
pub fn to_builtin(config: &Value) -> Result<Value, String> {
    let Some(map) = config.as_object() else {
        return Err("The server entry must be a JSON object.".into());
    };
    if map.get("socket").is_some_and(|value| !value.is_null()) {
        return Err("Socket servers aren't supported by Pi's built-in MCP.".into());
    }
    if map.get("type").and_then(Value::as_str) == Some("sse") {
        return Err("Legacy SSE transport is not supported; use the streamable HTTP URL.".into());
    }
    let mut out = Map::new();
    for key in [
        "type",
        "command",
        "args",
        "env",
        "cwd",
        "url",
        "headers",
        "description",
        "enabled",
        "exposure",
        "toolExposure",
    ] {
        if let Some(value) = map.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    if let Some(timeout) = map.get("timeout") {
        if timeout.is_number() {
            out.insert("timeout".into(), timeout.clone());
        }
    }
    match map.get("oauth") {
        None | Some(Value::Null) => {}
        Some(oauth) if oauth.is_object() => {
            out.insert("oauth".into(), oauth.clone());
        }
        Some(_) => return Err("OAuth settings must be an object.".into()),
    }
    // Pi signs in over OAuth automatically; the adapter's string forms
    // ("oauth", "bearer", false) have no equivalent and are dropped.
    if let Some(auth) = map.get("auth") {
        if auth.is_object() {
            out.insert("auth".into(), auth.clone());
        }
    }
    if map.get("disabled") == Some(&Value::Bool(true)) {
        out.insert("enabled".into(), Value::Bool(false));
    }
    let mut headers = out
        .get("headers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let has_authorization = headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"));
    if !has_authorization {
        let token = map
            .get("bearerToken")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty());
        let env = map
            .get("bearerTokenEnv")
            .and_then(Value::as_str)
            .filter(|env| !env.is_empty());
        if let Some(token) = token {
            headers.insert(
                "Authorization".into(),
                Value::String(format!("Bearer {token}")),
            );
        } else if let Some(env) = env {
            headers.insert(
                "Authorization".into(),
                Value::String(format!("Bearer ${{{env}}}")),
            );
        }
    }
    if !headers.is_empty() {
        out.insert("headers".into(), Value::Object(headers));
    }
    match map.get("directTools") {
        Some(Value::Bool(true)) if !out.contains_key("exposure") => {
            out.insert("exposure".into(), Value::String("direct".into()));
        }
        Some(Value::Array(names)) => {
            let mut tool_exposure = out
                .get("toolExposure")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for name in names.iter().filter_map(Value::as_str) {
                tool_exposure.insert(name.to_string(), Value::String("direct".into()));
            }
            if !tool_exposure.is_empty() {
                out.insert("toolExposure".into(), Value::Object(tool_exposure));
            }
        }
        _ => {}
    }
    Ok(Value::Object(out))
}

/// Copy converted entries into the private mcp.json; existing names, invalid
/// entries, and namespace clashes are reported instead of overwritten.
fn copy_into(root: &Path, entries: Vec<(String, Value)>) -> AppResult<McpImportResult> {
    let mut result = McpImportResult {
        copied: Vec::new(),
        skipped: Vec::new(),
    };
    update(root, |map| {
        let servers = servers_mut(map);
        for (name, config) in entries {
            let converted = match to_builtin(&config) {
                Ok(converted) => converted,
                Err(reason) => {
                    result.skipped.push(McpSkipped { name, reason });
                    continue;
                }
            };
            if let Err(error) = validate_server(&name, &converted) {
                result.skipped.push(McpSkipped {
                    name,
                    reason: error.to_string(),
                });
                continue;
            }
            if servers.contains_key(&name) {
                result.skipped.push(McpSkipped {
                    name,
                    reason: "already in mcp.json".into(),
                });
                continue;
            }
            if let Some(other) = clashing_name(servers, &name, None) {
                result.skipped.push(McpSkipped {
                    name,
                    reason: format!("namespace clashes with {other}"),
                });
                continue;
            }
            if servers.len() >= MAX_SERVERS {
                result.skipped.push(McpSkipped {
                    name,
                    reason: "πDesk supports up to 100 MCP servers.".into(),
                });
                continue;
            }
            servers.insert(name.clone(), converted);
            result.copied.push(name);
        }
        Ok(())
    })?;
    Ok(result)
}

/// Copy the named servers from a shared MCP file. Existing names, clashes,
/// and entries Pi cannot represent come back as skipped with a reason.
pub fn import(root: &Path, source_id: &str, names: &[String]) -> AppResult<McpImportResult> {
    let (_, path) = import_paths()
        .into_iter()
        .find(|(id, _)| *id == source_id)
        .ok_or_else(|| AppError::new("Unknown MCP file."))?;
    let (_, map) = read_file(&path, true)?
        .ok_or_else(|| AppError::new(format!("{} no longer exists.", display_path(&path))))?;
    let available = servers_of(&map);
    let mut entries = Vec::new();
    let mut missing = Vec::new();
    for name in names.iter().take(MAX_SERVERS) {
        match available.get(name) {
            Some(config) => entries.push((name.clone(), config.clone())),
            None => missing.push(name.clone()),
        }
    }
    let mut result = copy_into(root, entries)?;
    for name in missing {
        result.skipped.push(McpSkipped {
            name,
            reason: "not in that file".into(),
        });
    }
    Ok(result)
}

/// Copy every server from the old adapter's config through `to_builtin`.
fn copy_adapter_servers(root: &Path) -> AppResult<McpImportResult> {
    let (_, map) = read_file(&root.join("agent/mcp-adapter.json"), true)?.unwrap_or_default();
    let entries: Vec<(String, Value)> = servers_of(&map).into_iter().take(MAX_SERVERS).collect();
    copy_into(root, entries)
}

/// Replace the adapter with Pi's built-in MCP: copy its servers first (so a
/// failure leaves the adapter in place), then remove the package and turn the
/// built-in support on.
pub async fn switch_to_builtin(
    registry: &crate::harness::HarnessRegistry,
) -> AppResult<McpImportResult> {
    let root = util::pidesk_root();
    let copy_root = root.clone();
    let result = tauri::async_runtime::spawn_blocking(move || copy_adapter_servers(&copy_root))
        .await
        .map_err(|error| AppError::new(format!("Background task failed: {error}")))??;
    if let Some(source) = declared_adapter(&root) {
        crate::extensions::remove(registry, &source).await?;
    }
    enable_builtin(&root)?;
    Ok(result)
}

// ---------------------------------------------------------------------
// Pi's built-in MCP through the private CLI
// ---------------------------------------------------------------------

/// One sign-in at a time; holds the running child's process group id (0 while
/// the process is still starting).
static LOGIN: parking_lot::Mutex<Option<i32>> = parking_lot::Mutex::new(None);

struct CappedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn read_capped(stream: impl tokio::io::AsyncRead + Unpin, cap: usize) -> Vec<u8> {
    let mut buffer = Vec::new();
    let _ = stream.take(cap as u64).read_to_end(&mut buffer).await;
    buffer
}

/// Run a private-Pi command with capped pipe reads; the timeout covers spawn,
/// reads, and exit, and dropping the future kills the child (kill_on_drop).
async fn run_capped(
    mut command: tokio::process::Command,
    timeout: std::time::Duration,
    stdout_cap: usize,
    stderr_cap: usize,
    too_long: &str,
) -> AppResult<CappedOutput> {
    tokio::time::timeout(timeout, async {
        let mut child = command
            .spawn()
            .map_err(|_| AppError::new("Could not start the private Pi."))?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let (stdout, stderr) = tokio::join!(
            read_capped(stdout, stdout_cap),
            read_capped(stderr, stderr_cap)
        );
        let _ = child.wait().await;
        Ok::<_, AppError>(CappedOutput { stdout, stderr })
    })
    .await
    .map_err(|_| AppError::new(too_long))?
}

/// Redact credentials and cap a diagnostic line before it reaches the UI.
fn cap_redacted(text: &str, limit: usize) -> String {
    util::redact_secrets(text.trim())
        .chars()
        .take(limit)
        .collect()
}

/// Redacted tail of a command's output for an error message.
fn output_tail(stderr: &[u8], stdout: &[u8]) -> String {
    let bytes = if stderr.iter().any(|byte| !byte.is_ascii_whitespace()) {
        stderr
    } else {
        stdout
    };
    let text = String::from_utf8_lossy(bytes);
    let tail: String = text
        .chars()
        .rev()
        .take(MAX_ERROR_TAIL)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let tail = tail.trim();
    if tail.is_empty() {
        "No output from Pi.".into()
    } else {
        util::redact_secrets(tail)
    }
}

/// `<pi> mcp list --json`: connect to every enabled server and report state.
pub async fn check(registry: &crate::harness::HarnessRegistry) -> AppResult<McpStatus> {
    let root = util::pidesk_root();
    let exe = registry
        .executable_path(crate::dto::HarnessKind::Pi)
        .await?;
    util::prepare_private_home(&root)?;
    let mut command = tokio::process::Command::new(exe);
    util::configure_private_command(&mut command, &root);
    command
        .args(["mcp", "list", "--json"])
        .current_dir(root.join("home"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let output = run_capped(
        command,
        CHECK_TIMEOUT,
        MAX_CHECK_OUTPUT,
        MAX_CHECK_ERROR,
        "Checking MCP servers took too long.",
    )
    .await?;
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(json) = serde_json::from_str::<Value>(text.trim())
        .ok()
        .and_then(|value| value.as_object().cloned())
    else {
        return Err(AppError::new(format!(
            "Pi could not list MCP servers: {}",
            output_tail(&output.stderr, &output.stdout)
        )));
    };
    let servers: Vec<McpServerStatus> = json
        .get("servers")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .take(200)
                .filter_map(|row| {
                    let name = row.get("name").and_then(Value::as_str)?.to_string();
                    let state = row
                        .get("state")
                        .and_then(Value::as_str)
                        .unwrap_or("disconnected")
                        .to_string();
                    let tools: Vec<String> = row
                        .get("tools")
                        .and_then(Value::as_array)
                        .map(|tools| {
                            tools
                                .iter()
                                .filter_map(Value::as_str)
                                .take(200)
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default();
                    let error = row
                        .get("error")
                        .and_then(Value::as_str)
                        .filter(|error| !error.trim().is_empty())
                        .map(|error| cap_redacted(error, MAX_STATUS_ERROR));
                    Some(McpServerStatus {
                        name,
                        state,
                        tools,
                        error,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let errors: Vec<String> = json
        .get("errors")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(|error| cap_redacted(error, MAX_STATUS_ERROR))
                .take(50)
                .collect()
        })
        .unwrap_or_default();
    Ok(McpStatus { servers, errors })
}

/// `pi mcp login <name>`: opens the browser and waits for the loopback
/// callback. Only one sign-in runs at a time; `cancel_login` kills it.
pub async fn login(registry: &crate::harness::HarnessRegistry, name: &str) -> AppResult<String> {
    if !valid_name(name) {
        return Err(AppError::new(
            "Server names use letters, numbers, dashes, or underscores (up to 64).",
        ));
    }
    {
        let mut slot = LOGIN.lock();
        if slot.is_some() {
            return Err(AppError::new("Another MCP sign-in is already waiting."));
        }
        // Reserve the slot before spawning so two clicks can never race.
        *slot = Some(0);
    }
    let result = run_login(registry, name).await;
    let mut slot = LOGIN.lock();
    if *slot == Some(0) {
        *slot = None;
    }
    result
}

async fn run_login(registry: &crate::harness::HarnessRegistry, name: &str) -> AppResult<String> {
    let root = util::pidesk_root();
    let exe = registry
        .executable_path(crate::dto::HarnessKind::Pi)
        .await?;
    util::prepare_private_home(&root)?;
    let mut command = tokio::process::Command::new(exe);
    util::configure_private_command(&mut command, &root);
    command
        .args(["mcp", "login", name, "--timeout", "300"])
        .current_dir(root.join("home"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| AppError::new("Could not start the private Pi."))?;
    #[cfg(unix)]
    let pgid = child.id().map(|id| id as i32).unwrap_or(0);
    #[cfg(not(unix))]
    let pgid = 0;
    {
        let mut slot = LOGIN.lock();
        if slot.is_some() {
            *slot = Some(pgid);
        } else {
            drop(slot);
            let _ = child.start_kill();
            return Err(AppError::new("Sign-in cancelled."));
        }
    }
    let waited = tokio::time::timeout(LOGIN_TIMEOUT, async {
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let (stdout, stderr) = tokio::join!(
            read_capped(stdout, MAX_CHECK_OUTPUT),
            read_capped(stderr, MAX_CHECK_ERROR)
        );
        let status = child
            .wait()
            .await
            .map_err(|_| AppError::new("Could not run the private Pi."))?;
        Ok::<_, AppError>((status, stdout, stderr))
    })
    .await;
    // Claim our process group in one critical section: `cancel_login` clears
    // the slot when it kills, and a cleared slot means this run was cancelled.
    let cancelled = {
        let mut slot = LOGIN.lock();
        if *slot == Some(pgid) {
            *slot = None;
            false
        } else {
            true
        }
    };
    if cancelled {
        let _ = child.start_kill();
        return Err(AppError::new("Sign-in cancelled."));
    }
    let (status, stdout, stderr) = match waited {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            let _ = child.start_kill();
            return Err(AppError::new("Sign-in took too long."));
        }
    };
    if status.success() {
        let text = String::from_utf8_lossy(&stdout);
        let line = text
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string());
        Ok(line.unwrap_or_else(|| format!("Signed in to {name}.")))
    } else {
        Err(AppError::new(format!(
            "Sign-in did not complete: {}",
            output_tail(&stderr, &stdout)
        )))
    }
}

/// Kill the waiting sign-in's process group, if any. The waiting `login`
/// then reports a cancellation instead of a terminated-process error.
pub fn cancel_login() {
    #[cfg(unix)]
    {
        let mut slot = LOGIN.lock();
        if let Some(pgid) = *slot {
            if pgid > 0 {
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("pidesk-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("agent")).unwrap();
        root
    }

    #[test]
    fn validation_mirrors_pis_rules() {
        assert!(validate_server("ctx7", &json!({"url": "https://mcp.context7.com/mcp"})).is_ok());
        assert!(validate_server(
            "dev",
            &json!({"command": "npx", "args": ["-y", "x"], "env": {"K": "v"}, "futureField": 1})
        )
        .is_ok());
        assert!(validate_server(
            "signed",
            &json!({"url": "https://x.example/mcp", "auth": {"provider": "oauth"}})
        )
        .is_ok());
        assert!(validate_server(
            "local",
            &json!({"url": "http://localhost:3000/mcp", "auth": {"provider": "oauth"}})
        )
        .is_ok());
        assert!(validate_server(
            "tools",
            &json!({"command": "x", "exposure": "hidden", "toolExposure": {"read": "direct"}, "timeout": 30, "description": "ok"})
        )
        .is_ok());
        for (name, config) in [
            ("bad name", json!({"url": "https://x.example"})),
            ("dot.name", json!({"url": "https://x.example"})),
            (
                "both",
                json!({"url": "https://x.example", "command": "npx"}),
            ),
            ("none", json!({"args": []})),
            ("scheme", json!({"url": "file:///etc"})),
            ("env", json!({"url": "${TOKEN}/mcp"})),
            (
                "string-auth",
                json!({"url": "https://x.example", "auth": "oauth"}),
            ),
            ("sse", json!({"url": "https://x.example", "type": "sse"})),
            (
                "exposure",
                json!({"url": "https://x.example", "exposure": "sometimes"}),
            ),
            (
                "enabled",
                json!({"url": "https://x.example", "enabled": "yes"}),
            ),
            ("timeout", json!({"url": "https://x.example", "timeout": 0})),
        ] {
            assert!(validate_server(name, &config).is_err(), "{name}");
        }
    }

    #[test]
    fn adapter_entries_convert_to_builtin_shape() {
        let converted = to_builtin(&json!({
            "url": "https://eureka.example/mcp",
            "auth": "oauth",
            "directTools": true,
            "protocolVersion": "auto",
        }))
        .unwrap();
        assert_eq!(
            converted,
            json!({"url": "https://eureka.example/mcp", "exposure": "direct"})
        );
        let bearer = to_builtin(&json!({
            "url": "https://x.example/mcp",
            "bearerTokenEnv": "X_TOKEN",
            "requestTimeoutMs": 30000,
        }))
        .unwrap();
        assert_eq!(
            bearer,
            json!({"url": "https://x.example/mcp", "headers": {"Authorization": "Bearer ${X_TOKEN}"}})
        );
        assert!(to_builtin(&json!({"socket": "/tmp/sock"})).is_err());
        assert!(to_builtin(&json!({"url": "https://x.example", "type": "sse"})).is_err());
    }

    #[test]
    fn toggling_writes_enabled_false_only_when_off() {
        let root = root();
        std::fs::write(
            root.join("agent/mcp.json"),
            r#"{"mcpServers":{"ctx":{"url":"https://x.example/mcp"}}}"#,
        )
        .unwrap();
        set_enabled(&root, "ctx", false).unwrap();
        let raw = std::fs::read_to_string(root.join("agent/mcp.json")).unwrap();
        assert!(raw.contains("\"enabled\": false"));
        assert!(!overview(&root).servers[0].enabled);
        set_enabled(&root, "ctx", true).unwrap();
        let raw = std::fs::read_to_string(root.join("agent/mcp.json")).unwrap();
        assert!(!raw.contains("enabled"));
        assert!(overview(&root).servers[0].enabled);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn raw_saves_are_strict_but_keep_the_exact_text() {
        let root = root();
        assert!(save_raw(&root, "{ not json").is_err());
        assert!(save_raw(&root, "{\n  // comment\n  \"mcpServers\": {}\n}").is_err());
        assert!(save_raw(&root, r#"{"mcpServers":{"x":{"url":"ftp://x"}}}"#).is_err());
        assert!(save_raw(&root, r#"{"autoEnableCodemode":"yes"}"#).is_err());
        let text = "{\n  \"mcpServers\": { \"ctx\": { \"url\": \"https://c/mcp\" } },\n  \"autoEnableCodemode\": true\n}\n";
        save_raw(&root, text).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("agent/mcp.json")).unwrap(),
            text
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn namespaces_that_collide_are_rejected() {
        let root = root();
        save_server(
            &root,
            None,
            "my-server",
            json!({"url": "https://a.example/mcp"}),
        )
        .unwrap();
        assert!(save_server(
            &root,
            None,
            "my_server",
            json!({"url": "https://b.example/mcp"})
        )
        .is_err());
        save_server(
            &root,
            None,
            "other",
            json!({"url": "https://c.example/mcp"}),
        )
        .unwrap();
        // A rename that only changes case-free spelling keeps the old entry.
        save_server(
            &root,
            Some("other"),
            "other",
            json!({"url": "https://d.example/mcp"}),
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copied_entries_convert_and_report_skips() {
        let root = root();
        let result = copy_into(
            &root,
            vec![
                (
                    "good".into(),
                    json!({"url": "https://x.example/mcp", "auth": "oauth", "directTools": true}),
                ),
                (
                    "legacy".into(),
                    json!({"url": "https://x.example/sse", "type": "sse"}),
                ),
                ("bad".into(), json!({"url": "file:///etc"})),
            ],
        )
        .unwrap();
        assert_eq!(result.copied, vec!["good"]);
        assert_eq!(result.skipped.len(), 2);
        assert!(result.skipped[0].reason.contains("SSE"));
        assert!(result.skipped[1].reason.contains("https"));
        assert_eq!(overview(&root).servers[0].exposure, "direct");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enabling_builtin_removes_only_the_disabling_entries() {
        let root = root();
        std::fs::write(
            root.join("agent/settings.json"),
            r#"{"defaultModel":"x","extensions":["npm:pi-web-access","-builtin:mcp"],"packages":["npm:pi-mcp-adapter"]}"#,
        )
        .unwrap();
        assert!(builtin_disabled(&root));
        enable_builtin(&root).unwrap();
        assert!(!builtin_disabled(&root));
        let settings: Value =
            serde_json::from_slice(&std::fs::read(root.join("agent/settings.json")).unwrap())
                .unwrap();
        assert_eq!(settings["extensions"], json!(["npm:pi-web-access"]));
        assert_eq!(settings["defaultModel"], json!("x"));
        assert_eq!(settings["packages"], json!(["npm:pi-mcp-adapter"]));
        // An empty list drops the key so Pi's own defaults keep working.
        std::fs::write(
            root.join("agent/settings.json"),
            r#"{"extensions":["!builtin:mcp"],"defaultModel":"y"}"#,
        )
        .unwrap();
        enable_builtin(&root).unwrap();
        let settings: Value =
            serde_json::from_slice(&std::fs::read(root.join("agent/settings.json")).unwrap())
                .unwrap();
        assert!(settings.get("extensions").is_none());
        assert_eq!(settings["defaultModel"], json!("y"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adapter_is_reported_only_while_its_package_is_declared() {
        let root = root();
        assert!(overview(&root).adapter.is_none());
        std::fs::write(
            root.join("agent/settings.json"),
            r#"{"packages":["npm:pi-mcp-adapter@2.37.0"]}"#,
        )
        .unwrap();
        // The adapter's own file is JSONC-tolerant, unlike πDesk's mcp.json.
        std::fs::write(
            root.join("agent/mcp-adapter.json"),
            "{\n  // legacy\n  \"mcpServers\": { \"a\": {}, }\n}",
        )
        .unwrap();
        let adapter = overview(&root).adapter.unwrap();
        assert_eq!(adapter.source, "npm:pi-mcp-adapter@2.37.0");
        assert_eq!(adapter.servers, vec!["a"]);
        assert_eq!(adapter.version, None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn views_flag_likely_credentials_and_describe_targets() {
        let server = view(
            "s",
            &json!({"command": "npx", "args": ["-y", "srv"], "env": {"API_KEY": "k"}}),
            false,
        );
        assert_eq!(
            (
                server.transport.as_str(),
                server.target.as_str(),
                server.has_secrets,
                server.exposure.as_str(),
            ),
            ("stdio", "npx -y srv", true, "codemode")
        );
        assert!(server.config.is_null());
        let remote = view(
            "r",
            &json!({"url": "https://r", "exposure": "codemode-deferred", "oauth": {"clientSecret": "s"}}),
            true,
        );
        assert_eq!(
            (
                remote.transport.as_str(),
                remote.exposure.as_str(),
                remote.has_secrets,
            ),
            ("http", "codemode", true)
        );
    }
}
