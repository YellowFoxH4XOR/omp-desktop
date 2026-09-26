//! MCP servers for πDesk's private Pi, through the bundled-by-default
//! `pi-mcp-adapter` extension. πDesk owns one file, `<agent dir>/mcp.json`
//! (the adapter's "Pi global" config). Shared MCP files in the user's real
//! home (`~/.config/mcp/mcp.json`, `~/.agents/…`) are never loaded: Pi runs
//! with a private home. They can be copied in, per server, only when the
//! user asks, and the backend re-reads them itself, so their values never
//! pass through the UI.

use crate::error::{AppError, AppResult};
use crate::util;
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_SERVERS: usize = 100;
const MAX_SERVER_BYTES: usize = 64 * 1024;
pub const ADAPTER_SOURCE: &str = "npm:pi-mcp-adapter";
const LIFECYCLES: &[&str] = &["lazy", "eager", "keep-alive", "lazy-keep-alive"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    /// `stdio`, `http`, `socket`, or `unknown`.
    pub transport: String,
    /// Command line or URL, for display.
    pub target: String,
    pub disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,
    /// Has env values, headers, or a bearer token (likely credentials).
    pub has_secrets: bool,
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
pub struct McpOverview {
    pub adapter_installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_version: Option<String>,
    pub path: String,
    pub servers: Vec<McpServer>,
    /// `off`, `all`, or `custom` (patterns set in the file).
    pub approve_tools: String,
    pub raw: String,
    /// Comments or trailing commas are lost if the form rewrites the file.
    pub has_comments: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub import_sources: Vec<McpImportSource>,
}

fn config_path(root: &Path) -> PathBuf {
    root.join("agent/mcp.json")
}

/// The shared MCP files the adapter would read from a normal home.
fn import_paths() -> Vec<(&'static str, PathBuf)> {
    let home = util::home_dir();
    vec![
        ("config-mcp", home.join(".config/mcp/mcp.json")),
        ("agents", home.join(".agents/mcp.json")),
        ("agents-mcp", home.join(".agents/mcp/mcp.json")),
    ]
}

fn display_path(path: &Path) -> String {
    let home = util::home_dir();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Strip `//` and `/* */` comments and trailing commas outside strings, as
/// the adapter's JSONC reader accepts them.
pub fn strip_jsonc(text: &str) -> String {
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

/// Parse an MCP file; `Ok(None)` when it does not exist.
fn read_file(path: &Path) -> AppResult<Option<(String, Map<String, Value>, bool)>> {
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
    let (value, commented) = match serde_json::from_str::<Value>(&raw) {
        Ok(value) => (value, false),
        Err(_) => (
            serde_json::from_str::<Value>(&strip_jsonc(&raw)).map_err(|error| {
                AppError::new(format!("{} is not valid JSON: {error}", display_path(path)))
            })?,
            true,
        ),
    };
    match value {
        Value::Object(map) => Ok(Some((raw, map, commented))),
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
    } else if let Some(url) = text("url") {
        ("http", url)
    } else if let Some(socket) = text("socket") {
        ("socket", socket)
    } else {
        ("unknown", String::new())
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
        disabled: config.get("disabled") == Some(&Value::Bool(true)),
        auth: text("auth"),
        lifecycle: text("lifecycle"),
        has_secrets: non_empty("env")
            || non_empty("headers")
            || config.get("bearerToken").is_some(),
        config: if include_config {
            config.clone()
        } else {
            Value::Null
        },
    }
}

fn adapter(root: &Path) -> (bool, Option<String>) {
    let declared = crate::pi_settings::read_packages(root)
        .unwrap_or_default()
        .iter()
        .any(|source| source == ADAPTER_SOURCE || source.starts_with("npm:pi-mcp-adapter@"));
    let version = std::fs::read(root.join("agent/npm/node_modules/pi-mcp-adapter/package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|json| {
            json.get("version")
                .and_then(Value::as_str)
                .map(String::from)
        });
    (declared && version.is_some(), version)
}

pub fn overview(root: &Path) -> McpOverview {
    let (adapter_installed, adapter_version) = adapter(root);
    let path = config_path(root);
    let (raw, map, has_comments, error) = match read_file(&path) {
        Ok(Some((raw, map, commented))) => (raw, map, commented, None),
        Ok(None) => (String::new(), Map::new(), false, None),
        Err(error) => (
            std::fs::read_to_string(&path).unwrap_or_default(),
            Map::new(),
            false,
            Some(error.to_string()),
        ),
    };
    let approve_tools = match map.get("settings").and_then(|s| s.get("approveTools")) {
        None | Some(Value::Bool(false)) => "off",
        Some(Value::Bool(true)) => "all",
        Some(_) => "custom",
    };
    let import_sources = import_paths()
        .into_iter()
        .filter_map(|(id, path)| {
            let (_, map, _) = read_file(&path).ok().flatten()?;
            let servers: Vec<McpServer> = servers_of(&map)
                .iter()
                .take(MAX_SERVERS)
                .map(|(name, config)| view(name, config, false))
                .collect();
            (!servers.is_empty()).then(|| McpImportSource {
                id: id.into(),
                path: display_path(&path),
                servers,
            })
        })
        .collect();
    McpOverview {
        adapter_installed,
        adapter_version,
        path: display_path(&path),
        servers: servers_of(&map)
            .iter()
            .take(MAX_SERVERS)
            .map(|(name, config)| view(name, config, true))
            .collect(),
        approve_tools: approve_tools.into(),
        raw,
        has_comments,
        error,
        import_sources,
    }
}

// ---------------------------------------------------------------------
// Validation and writes
// ---------------------------------------------------------------------

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

fn string_map(value: Option<&Value>, what: &str) -> AppResult<()> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(map)) if map.len() <= 100 => {
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

/// Check the fields πDesk's form edits; other adapter fields pass through.
pub fn validate_server(name: &str, config: &Value) -> AppResult<()> {
    if !valid_name(name) {
        return Err(AppError::new(
            "Server names use letters, numbers, dashes, dots, or underscores (up to 64).",
        ));
    }
    let Value::Object(map) = config else {
        return Err(AppError::new("A server entry must be a JSON object."));
    };
    if serde_json::to_vec(config)?.len() > MAX_SERVER_BYTES {
        return Err(AppError::new("This server entry is too large."));
    }
    let kinds = ["command", "url", "socket"]
        .iter()
        .filter(|key| map.get(**key).is_some_and(|v| !v.is_null()))
        .count();
    if kinds != 1 {
        return Err(AppError::new(
            "Give a server exactly one of a command or a URL.",
        ));
    }
    for key in ["command", "url", "socket", "cwd", "auth", "lifecycle"] {
        if let Some(value) = map.get(key) {
            if !value.is_null()
                && value
                    .as_str()
                    .is_none_or(|text| text.trim().is_empty() || text.len() > 4096)
            {
                return Err(AppError::new(format!("{key} must be non-empty text.")));
            }
        }
    }
    if let Some(url) = map.get("url").and_then(Value::as_str) {
        if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("${")) {
            return Err(AppError::new(
                "URLs start with https:// (or http:// for local servers).",
            ));
        }
    }
    if let Some(args) = map.get("args") {
        let ok = args.as_array().is_some_and(|args| {
            args.len() <= 100
                && args
                    .iter()
                    .all(|arg| arg.as_str().is_some_and(|a| a.len() <= 4096))
        });
        if !ok {
            return Err(AppError::new("Arguments must be a list of text values."));
        }
    }
    string_map(map.get("env"), "Environment")?;
    string_map(map.get("headers"), "Header")?;
    if let Some(auth) = map.get("auth").and_then(Value::as_str) {
        if !matches!(auth, "bearer" | "oauth") {
            return Err(AppError::new("Sign-in must be bearer or oauth."));
        }
    }
    if let Some(lifecycle) = map.get("lifecycle").and_then(Value::as_str) {
        if !LIFECYCLES.contains(&lifecycle) {
            return Err(AppError::new("Unknown lifecycle."));
        }
    }
    if map
        .get("disabled")
        .is_some_and(|v| !v.is_boolean() && !v.is_null())
    {
        return Err(AppError::new("disabled must be true or false."));
    }
    Ok(())
}

/// Read, change, and atomically rewrite the private mcp.json.
fn update(
    root: &Path,
    change: impl FnOnce(&mut Map<String, Value>) -> AppResult<()>,
) -> AppResult<()> {
    util::ensure_private_directory(root, &root.join("agent"))?;
    let path = config_path(root);
    let mut map = match read_file(&path)? {
        Some((_, map, _)) => map,
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
            server.remove("disabled");
        } else {
            server.insert("disabled".into(), Value::Bool(true));
        }
        Ok(())
    })
}

pub fn set_approve_tools(root: &Path, all: bool) -> AppResult<()> {
    update(root, |map| {
        let settings = map
            .entry("settings")
            .or_insert_with(|| Value::Object(Map::new()));
        if !settings.is_object() {
            *settings = Value::Object(Map::new());
        }
        let settings = settings.as_object_mut().expect("object");
        if all {
            settings.insert("approveTools".into(), Value::Bool(true));
        } else {
            settings.remove("approveTools");
        }
        Ok(())
    })
}

/// Save hand-edited text as-is (comments kept) once it parses as an object
/// whose `mcpServers`, if present, is an object of valid servers.
pub fn save_raw(root: &Path, text: &str) -> AppResult<()> {
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(AppError::new("mcp.json is too large."));
    }
    let value: Value = serde_json::from_str(text)
        .or_else(|_| serde_json::from_str(&strip_jsonc(text)))
        .map_err(|error| AppError::new(format!("Not valid JSON: {error}")))?;
    let Value::Object(map) = &value else {
        return Err(AppError::new("mcp.json must contain a JSON object."));
    };
    match map.get("mcpServers") {
        None => {}
        Some(Value::Object(servers)) => {
            if servers.len() > MAX_SERVERS {
                return Err(AppError::new("πDesk supports up to 100 MCP servers."));
            }
            for (name, config) in servers {
                validate_server(name, config)
                    .map_err(|error| AppError::new(format!("{name}: {error}")))?;
            }
        }
        Some(_) => return Err(AppError::new("mcpServers must be an object of servers.")),
    }
    util::ensure_private_directory(root, &root.join("agent"))?;
    let mut bytes = text.as_bytes().to_vec();
    if !text.ends_with('\n') {
        bytes.push(b'\n');
    }
    crate::pi_settings::write_bytes(&config_path(root), &bytes)
}

/// Copy the named servers from a shared MCP file. Existing names are kept
/// and reported, never overwritten. Returns the names copied.
pub fn import(root: &Path, source_id: &str, names: &[String]) -> AppResult<Vec<String>> {
    let (_, path) = import_paths()
        .into_iter()
        .find(|(id, _)| *id == source_id)
        .ok_or_else(|| AppError::new("Unknown MCP file."))?;
    let (_, map, _) = read_file(&path)?
        .ok_or_else(|| AppError::new(format!("{} no longer exists.", display_path(&path))))?;
    let shared = servers_of(&map);
    let mut copied = Vec::new();
    update(root, |target| {
        let servers = servers_mut(target);
        for name in names.iter().take(MAX_SERVERS) {
            let Some(config) = shared.get(name) else {
                continue;
            };
            if servers.contains_key(name)
                || validate_server(name, config).is_err()
                || servers.len() >= MAX_SERVERS
            {
                continue;
            }
            servers.insert(name.clone(), config.clone());
            copied.push(name.clone());
        }
        Ok(())
    })?;
    Ok(copied)
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
    fn jsonc_comments_and_trailing_commas_are_stripped_outside_strings() {
        let text = "{\n // note\n \"a\": \"x // not a comment\", /* block */ \"b\": [1, 2,],\n}";
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value, json!({"a": "x // not a comment", "b": [1, 2]}));
    }

    #[test]
    fn servers_validate_transport_fields_and_names() {
        assert!(validate_server(
            "ctx7",
            &json!({"url": "https://mcp.context7.com/mcp", "lifecycle": "lazy"})
        )
        .is_ok());
        assert!(validate_server(
            "dev",
            &json!({"command": "npx", "args": ["-y", "x"], "env": {"K": "v"}, "futureField": 1})
        )
        .is_ok());
        for (name, config) in [
            ("bad name", json!({"url": "https://x"})),
            ("both", json!({"url": "https://x", "command": "npx"})),
            ("none", json!({"args": []})),
            ("scheme", json!({"url": "file:///etc"})),
            ("args", json!({"command": "x", "args": "one"})),
            ("env", json!({"command": "x", "env": {"K": 1}})),
            ("auth", json!({"url": "https://x", "auth": "basic"})),
            (
                "life",
                json!({"url": "https://x", "lifecycle": "sometimes"}),
            ),
        ] {
            assert!(validate_server(name, &config).is_err(), "{name}");
        }
    }

    #[test]
    fn edits_preserve_other_fields_and_rename_safely() {
        let root = root();
        std::fs::write(root.join("agent/mcp.json"), r#"{"settings":{"toolPrefix":"short"},"mcpServers":{"a":{"url":"https://a","directTools":true}}}"#).unwrap();
        save_server(
            &root,
            Some("a"),
            "alpha",
            json!({"url": "https://a2", "directTools": true}),
        )
        .unwrap();
        save_server(&root, None, "b", json!({"command": "npx", "args": ["b"]})).unwrap();
        assert!(save_server(&root, None, "b", json!({"command": "x"})).is_err());
        set_enabled(&root, "b", false).unwrap();
        set_approve_tools(&root, true).unwrap();
        let view = overview(&root);
        let names: Vec<_> = view
            .servers
            .iter()
            .map(|s| (s.name.as_str(), s.disabled))
            .collect();
        assert_eq!(names, vec![("alpha", false), ("b", true)]);
        assert_eq!(view.approve_tools, "all");
        let file: Value =
            serde_json::from_slice(&std::fs::read(root.join("agent/mcp.json")).unwrap()).unwrap();
        assert_eq!(file["settings"]["toolPrefix"], "short");
        assert_eq!(file["mcpServers"]["alpha"]["directTools"], true);
        set_enabled(&root, "b", true).unwrap();
        remove_server(&root, "alpha").unwrap();
        assert!(remove_server(&root, "alpha").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn raw_saves_keep_comments_but_must_be_valid() {
        let root = root();
        assert!(save_raw(&root, "{ not json").is_err());
        assert!(save_raw(&root, r#"{"mcpServers":{"x":{"url":"ftp://x"}}}"#).is_err());
        let text = "{\n  // docs lookups\n  \"mcpServers\": { \"ctx\": { \"url\": \"https://c/mcp\" } }\n}\n";
        save_raw(&root, text).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("agent/mcp.json")).unwrap(),
            text
        );
        let view = overview(&root);
        assert!(view.has_comments);
        assert_eq!(view.servers[0].transport, "http");
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
                server.has_secrets
            ),
            ("stdio", "npx -y srv", true)
        );
        assert!(server.config.is_null());
        let remote = view("r", &json!({"url": "https://r", "auth": "oauth"}), true);
        assert_eq!(
            (
                remote.transport.as_str(),
                remote.auth.as_deref(),
                remote.has_secrets
            ),
            ("http", Some("oauth"), false)
        );
    }
}
