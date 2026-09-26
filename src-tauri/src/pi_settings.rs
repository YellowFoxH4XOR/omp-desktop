//! Startup defaults in the private Pi's `settings.json`. Pi reads
//! `defaultProvider`, `defaultModel`, and `defaultThinkingLevel` when a new
//! session starts, so every new thread begins with them; resumed threads keep
//! their own model. RPC `set_model` never persists, so per-thread switches
//! cannot change the default.

use crate::dto::ModelDefaults;
use crate::error::{AppError, AppResult};
use crate::util;
use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

fn settings_path(root: &Path) -> PathBuf {
    root.join("agent/settings.json")
}

/// Provider and model ids are passed to Pi and written to its settings.
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/@+-".contains(c))
}

pub(crate) fn read_object(path: &Path) -> AppResult<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    util::check_owned_path(path, false)?;
    if std::fs::metadata(path)?.len() > MAX_SETTINGS_BYTES {
        return Err(AppError::new("Pi's settings file is too large to edit."));
    }
    match serde_json::from_slice::<Value>(&std::fs::read(path)?) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(AppError::new(
            "Pi's settings file is not valid JSON. Fix or remove ~/.pidesk/agent/settings.json.",
        )),
    }
}

/// Package sources declared in the private Pi's settings, in order. Object
/// entries (`{ "source": … }`) contribute their source.
pub fn read_packages(root: &Path) -> AppResult<Vec<String>> {
    let settings = read_object(&settings_path(root))?;
    let Some(Value::Array(entries)) = settings.get("packages") else {
        return Ok(vec![]);
    };
    Ok(entries
        .iter()
        .filter_map(|entry| match entry {
            Value::String(source) => Some(source.clone()),
            Value::Object(object) => object
                .get("source")
                .and_then(Value::as_str)
                .map(String::from),
            _ => None,
        })
        .take(200)
        .collect())
}

pub fn read_defaults(root: &Path) -> AppResult<ModelDefaults> {
    let settings = read_object(&settings_path(root))?;
    let text = |key: &str| {
        settings
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(String::from)
    };
    Ok(ModelDefaults {
        provider: text("defaultProvider"),
        model_id: text("defaultModel"),
        thinking_level: text("defaultThinkingLevel"),
    })
}

/// Update only the given keys, preserving everything else Pi stores there.
fn update(root: &Path, changes: &[(&str, &str)]) -> AppResult<ModelDefaults> {
    util::ensure_private_directory(root, &root.join("agent"))?;
    let path = settings_path(root);
    let mut settings = read_object(&path)?;
    for (key, value) in changes {
        settings.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    write_object(&path, settings)?;
    read_defaults(root)
}

/// Atomically replace a private JSON file (0600, never following a symlink).
pub(crate) fn write_object(path: &Path, object: Map<String, Value>) -> AppResult<()> {
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(object))?;
    bytes.push(b'\n');
    write_bytes(path, &bytes)
}

/// Atomically replace a private file with exactly these bytes.
pub(crate) fn write_bytes(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let written = (|| -> AppResult<()> {
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, &path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

pub fn set_default_model(root: &Path, provider: &str, model_id: &str) -> AppResult<ModelDefaults> {
    if !valid_id(provider) || !valid_id(model_id) {
        return Err(AppError::new("That model identifier is not valid."));
    }
    update(
        root,
        &[("defaultProvider", provider), ("defaultModel", model_id)],
    )
}

pub fn set_default_thinking_level(root: &Path, level: &str) -> AppResult<ModelDefaults> {
    if !THINKING_LEVELS.contains(&level) {
        return Err(AppError::new("That reasoning effort is not supported."));
    }
    update(root, &[("defaultThinkingLevel", level)])
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("pidesk-settings-{}", uuid::Uuid::new_v4()));
        util::ensure_private_directory(&root, &root.join("agent")).unwrap();
        root
    }

    #[test]
    fn defaults_update_only_their_keys() {
        let root = root();
        std::fs::write(
            settings_path(&root),
            r#"{"theme":"dark","defaultProvider":"old","packages":["x"]}"#,
        )
        .unwrap();
        let defaults = set_default_model(&root, "opencode-go", "kimi-k2.6").unwrap();
        assert_eq!(defaults.provider.as_deref(), Some("opencode-go"));
        assert_eq!(defaults.model_id.as_deref(), Some("kimi-k2.6"));
        let defaults = set_default_thinking_level(&root, "high").unwrap();
        assert_eq!(defaults.thinking_level.as_deref(), Some("high"));
        let saved: Value =
            serde_json::from_slice(&std::fs::read(settings_path(&root)).unwrap()).unwrap();
        assert_eq!(saved["theme"], "dark");
        assert_eq!(saved["packages"][0], "x");
        assert_eq!(saved["defaultModel"], "kimi-k2.6");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_bad_values_and_broken_files() {
        let root = root();
        assert!(read_defaults(&root).unwrap().model_id.is_none());
        assert!(set_default_model(&root, "", "x").is_err());
        assert!(set_default_model(&root, "p", "bad model\n").is_err());
        assert!(set_default_thinking_level(&root, "turbo").is_err());
        std::fs::write(settings_path(&root), "{not json").unwrap();
        assert!(set_default_model(&root, "p", "m").is_err());
        // A broken file is reported, never overwritten.
        assert_eq!(
            std::fs::read_to_string(settings_path(&root)).unwrap(),
            "{not json"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_a_symlinked_settings_file() {
        let root = root();
        let outside = root.join("outside.json");
        std::fs::write(&outside, "{}").unwrap();
        std::os::unix::fs::symlink(&outside, settings_path(&root)).unwrap();
        assert!(set_default_model(&root, "p", "m").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "{}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
