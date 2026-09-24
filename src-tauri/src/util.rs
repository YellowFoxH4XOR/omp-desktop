use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_iso() -> String {
    iso_from_unix(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
}

pub fn iso_from_unix(secs: u64) -> String {
    // Civil-from-days algorithm (Howard Hinnant), no external deps.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn home_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME") {
        let p = PathBuf::from(h);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Resolve a path to an absolute, canonical-ish form without requiring it to
/// exist: expand nothing, just normalize `.`/`..` lexically after absolutizing.
pub fn normalize_path(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(p)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize when the path exists, else lexical normalization.
pub fn resolve_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| normalize_path(p))
}

/// OMP session directory name for a cwd:
/// - under $HOME: `-` + relative path with separators replaced by `-`
/// - under tmpdir: `-tmp` + relative path encoded the same way
/// - anywhere else: `--` + absolute path (leading slash stripped) + `--`
pub fn omp_session_dir_name(cwd: &Path) -> String {
    let resolved = resolve_path(cwd);
    let home = resolve_path(&home_dir());
    if let Ok(rel) = resolved.strip_prefix(&home) {
        return encode_dashed("-", rel);
    }
    let tmp = resolve_path(&std::env::temp_dir());
    if let Ok(rel) = resolved.strip_prefix(&tmp) {
        return encode_dashed("-tmp", rel);
    }
    format!(
        "--{}--",
        dash_encode(
            &resolved
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
                .to_string()
        )
    )
}

fn encode_dashed(prefix: &str, rel: &Path) -> String {
    let s = rel.to_string_lossy().replace(['/', '\\', ':'], "-");
    if s.is_empty() {
        prefix.to_string()
    } else if prefix.ends_with('-') {
        format!("{prefix}{s}")
    } else {
        format!("{prefix}-{s}")
    }
}

fn dash_encode(s: &str) -> String {
    s.replace(['/', '\\', ':'], "-")
}

/// Pi session directory name: always `--<abs path encoded>--`.
pub fn pi_session_dir_name(cwd: &Path) -> String {
    let resolved = resolve_path(cwd);
    let s = resolved.to_string_lossy().replace('\\', "/");
    format!("--{}--", dash_encode(s.trim_start_matches('/')))
}

/// Use shell-profile overrides even when launched from Finder, whose
/// environment usually omits the user's terminal configuration.
fn harness_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| login_shell_env().and_then(|env| env.get(key).cloned()))
}

pub fn agent_dir(kind: crate::dto::HarnessKind) -> PathBuf {
    if let Some(dir) = harness_env("PI_CODING_AGENT_DIR") {
        return PathBuf::from(dir);
    }
    match kind {
        crate::dto::HarnessKind::Omp => home_dir().join(".omp").join("agent"),
        crate::dto::HarnessKind::Pi => home_dir().join(".pi").join("agent"),
    }
}

pub fn session_dir_for(kind: crate::dto::HarnessKind, cwd: &Path) -> PathBuf {
    if let Some(dir) = harness_env("PI_CODING_AGENT_SESSION_DIR") {
        return PathBuf::from(dir);
    }
    let name = match kind {
        crate::dto::HarnessKind::Omp => omp_session_dir_name(cwd),
        crate::dto::HarnessKind::Pi => pi_session_dir_name(cwd),
    };
    agent_dir(kind).join("sessions").join(name)
}

/// The login shell's exported environment is cached in memory, never logged
/// or persisted. Finder-launched apps otherwise miss both tool paths and
/// provider environment credentials configured in `.zshrc`/`.zprofile`.
static LOGIN_ENV: std::sync::LazyLock<Option<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(read_login_shell_env);

pub fn login_shell_env() -> Option<&'static std::collections::HashMap<String, String>> {
    LOGIN_ENV.as_ref()
}

pub fn login_shell_path() -> Option<String> {
    login_shell_env()?.get("PATH").cloned()
}

fn read_login_shell_env() -> Option<std::collections::HashMap<String, String>> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    shell_env(&shell, &["-i", "-l", "-c"]).or_else(|| shell_env(&shell, &["-l", "-c"]))
}

fn shell_env(shell: &str, flags: &[&str]) -> Option<std::collections::HashMap<String, String>> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(shell)
        .args(flags)
        .arg(
            "printf '\\0__OMP_DESKTOP_ENV_START__\\0'; env -0; printf '__OMP_DESKTOP_ENV_END__\\0'",
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return None;
    }
    parse_shell_env(&child.wait_with_output().ok()?.stdout)
}

fn parse_shell_env(output: &[u8]) -> Option<std::collections::HashMap<String, String>> {
    const START: &[u8] = b"\0__OMP_DESKTOP_ENV_START__\0";
    const END: &[u8] = b"__OMP_DESKTOP_ENV_END__\0";
    let start = output
        .windows(START.len())
        .rposition(|part| part == START)?
        + START.len();
    let tail = &output[start..];
    let end = tail.windows(END.len()).position(|part| part == END)?;
    let mut env = std::collections::HashMap::new();
    for entry in tail[..end].split(|byte| *byte == 0) {
        let Ok(text) = std::str::from_utf8(entry) else {
            continue;
        };
        if let Some((key, value)) = text.split_once('=') {
            if !key.is_empty() {
                env.insert(key.to_string(), value.to_string());
            }
        }
    }
    env.contains_key("PATH").then_some(env)
}

/// Merge the login-shell PATH with the process PATH (login entries first,
/// deduplicated, preserving order).
pub fn merged_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    match login_shell_path() {
        Some(login) => {
            let mut seen = std::collections::HashSet::new();
            let mut parts: Vec<String> = Vec::new();
            for p in login.split(':').chain(current.split(':')) {
                if !p.is_empty() && seen.insert(p.to_string()) {
                    parts.push(p.to_string());
                }
            }
            parts.join(":")
        }
        None => current,
    }
}

/// Look up `name` in a PATH string.
pub fn which_in_path(name: &str, path: &str) -> Option<PathBuf> {
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(p) {
            return meta.is_file() && meta.permissions().mode() & 0o111 != 0;
        }
        false
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// Extract a semver-looking token from `--version` output.
pub fn parse_version(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let token = token
            .rsplit_once('/')
            .map(|(_, version)| version)
            .unwrap_or(token);
        let t = token.trim_matches(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
        });
        let t = t.strip_prefix('v').unwrap_or(t);
        let mut parts = t.split('.');
        if let (Some(major), Some(minor)) = (parts.next(), parts.next()) {
            if !major.is_empty()
                && major.chars().all(|c| c.is_ascii_digit())
                && minor.chars().all(|c| c.is_ascii_digit())
            {
                return Some(t.to_string());
            }
        }
    }
    None
}
/// Redact environment credentials before an error crosses into UI/log output.
/// The harness still receives the original environment; this only scrubs
/// diagnostic strings.
pub fn redact_secrets(text: &str) -> String {
    let mut result = text.to_string();
    let secret_key = |key: &str| {
        let upper = key.to_ascii_uppercase();
        ["TOKEN", "API_KEY", "PASSWORD", "SECRET", "CREDENTIAL"]
            .iter()
            .any(|part| upper.contains(part))
    };
    for (key, value) in std::env::vars_os() {
        let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
            continue;
        };
        if secret_key(key) && value.len() >= 8 && result.contains(value) {
            result = result.replace(value, "[REDACTED]");
        }
    }
    if let Some(shell_env) = login_shell_env() {
        for (key, value) in shell_env {
            if secret_key(key) && value.len() >= 8 && result.contains(value) {
                result = result.replace(value, "[REDACTED]");
            }
        }
    }
    for prefix in ["Bearer ", "api_key=", "apiKey=", "token="] {
        let mut from = 0;
        while let Some(relative) = result[from..].find(prefix) {
            let start = from + relative + prefix.len();
            let end = result[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '}'))
                .map(|offset| start + offset)
                .unwrap_or(result.len());
            if end == start {
                from = start;
                continue;
            }
            result.replace_range(start..end, "[REDACTED]");
            from = start + "[REDACTED]".len();
        }
    }
    result
}

/// True when bytes look like binary content (NUL in the first chunk).
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_environment_ignores_startup_banner() {
        let bytes = b"welcome\n\0__OMP_DESKTOP_ENV_START__\0PATH=/custom/bin:/usr/bin\0OPENAI_API_KEY=test-token\0__OMP_DESKTOP_ENV_END__\0";
        let env = parse_shell_env(bytes).unwrap();
        assert_eq!(
            env.get("PATH").map(String::as_str),
            Some("/custom/bin:/usr/bin")
        );
        assert_eq!(
            env.get("OPENAI_API_KEY").map(String::as_str),
            Some("test-token")
        );
        assert!(parse_shell_env(b"banner only").is_none());
    }

    #[test]
    fn parses_omp_and_pi_version_formats() {
        assert_eq!(parse_version("omp/18.2.11\n"), Some("18.2.11".into()));
        assert_eq!(parse_version("0.86.1\n"), Some("0.86.1".into()));
        assert_eq!(parse_version("unrelated output"), None);
    }

    #[test]
    fn redacts_bearer_and_key_values_from_diagnostics() {
        let text = "Authorization: Bearer example123 token=another456";
        let cleaned = redact_secrets(text);
        assert_eq!(cleaned, "Authorization: Bearer [REDACTED] token=[REDACTED]");
    }
}
