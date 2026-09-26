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

/// Pi session directory name: always `--<abs path encoded>--`.
pub fn pi_session_dir_name(cwd: &Path) -> String {
    let resolved = resolve_path(cwd);
    let s = resolved.to_string_lossy().replace('\\', "/");
    format!(
        "--{}--",
        s.trim_start_matches('/').replace(['/', '\\', ':'], "-")
    )
}

pub fn pidesk_root() -> PathBuf {
    home_dir().join(".pidesk")
}

pub fn agent_dir() -> PathBuf {
    pidesk_root().join("agent")
}

pub fn session_dir_for(cwd: &Path) -> PathBuf {
    agent_dir().join("sessions").join(pi_session_dir_name(cwd))
}

/// Keep OS/tool essentials, but never inherit provider credentials, Pi resource
/// bindings, NODE_OPTIONS, or npm configuration from the launching Pi/shell.
fn safe_process_env(key: &str) -> bool {
    matches!(
        key,
        "HOME"
            | "USER"
            | "LOGNAME"
            | "SHELL"
            | "TMPDIR"
            | "TMP"
            | "TEMP"
            | "LANG"
            | "LC_ALL"
            | "LC_CTYPE"
            | "TERM"
            | "SSH_AUTH_SOCK"
            | "HTTP_PROXY"
            | "HTTPS_PROXY"
            | "ALL_PROXY"
            | "NO_PROXY"
            | "http_proxy"
            | "https_proxy"
            | "all_proxy"
            | "no_proxy"
    )
}

pub fn configure_private_command(command: &mut tokio::process::Command, root: &Path) {
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        if key.to_str().is_some_and(safe_process_env) {
            command.env(key, value);
        }
    }
    let path = format!(
        "{}:{}",
        root.join("runtime/node_modules/.bin").display(),
        merged_path()
    );
    command
        .env("HOME", home_dir())
        .env("PATH", path)
        .env("PI_CODING_AGENT_DIR", root.join("agent"))
        .env("PI_CODING_AGENT_SESSION_DIR", root.join("agent/sessions"))
        .env("PI_SKIP_VERSION_CHECK", "1")
        .env("PI_TELEMETRY", "0");
}

/// Create private directories one component at a time, never following a
/// symlink out of πDesk's root. Called only by explicit install/thread actions.
pub fn ensure_private_directory(root: &Path, path: &Path) -> crate::error::AppResult<()> {
    use crate::error::AppError;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AppError::new("Invalid private directory."))?;
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            let std::path::Component::Normal(name) = component else {
                return Err(AppError::new("Invalid private directory."));
            };
            current.push(name);
        }
        if !current
            .try_exists()
            .map_err(|e| AppError::new(format!("Cannot inspect private directory: {e}")))?
        {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&current).map_err(|e| {
                AppError::new(format!(
                    "Cannot create private directory {}: {e}",
                    current.display()
                ))
            })?;
        }
        check_owned_path(&current, true)?;
    }
    Ok(())
}

pub fn check_owned_path(path: &Path, directory: bool) -> crate::error::AppResult<()> {
    use crate::error::AppError;
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| AppError::new(format!("Cannot inspect {}: {e}", path.display())))?;
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err(AppError::new(format!(
            "{} must be a real {} owned by πDesk's user, not a symlink.",
            path.display(),
            if directory { "directory" } else { "file" }
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err(AppError::new(format!(
                "{} must be owned by this user and not writable by others.",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Existing session files must remain within private storage after resolving
/// symlinks. Never resume or register a path supplied by external Pi.
pub fn private_session_file(path: &Path) -> crate::error::AppResult<PathBuf> {
    private_session_path_in(&pidesk_root(), path, true)
}

/// Pi may report its future journal path before the first turn is persisted.
pub fn private_session_target(path: &Path) -> crate::error::AppResult<PathBuf> {
    private_session_path_in(&pidesk_root(), path, false)
}

pub(crate) fn private_session_path_in(
    root: &Path,
    path: &Path,
    require_file: bool,
) -> crate::error::AppResult<PathBuf> {
    use crate::error::AppError;
    let sessions = root.join("agent/sessions");
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(AppError::new("Invalid private session path."));
    }
    check_owned_path(root, true)?;
    check_owned_path(&root.join("agent"), true)?;
    check_owned_path(&sessions, true)?;
    let base = std::fs::canonicalize(sessions)?;
    if !path.is_absolute() {
        return Err(AppError::new(
            "This session does not belong to πDesk's private Pi.",
        ));
    }
    let resolved = match std::fs::canonicalize(path) {
        Ok(resolved) => {
            check_owned_path(path, false)?; // reject even an in-tree symlink
            check_owned_path(&resolved, false)?;
            resolved
        }
        Err(error) if !require_file && error.kind() == std::io::ErrorKind::NotFound => {
            // A dangling symlink is not a future journal file.
            if std::fs::symlink_metadata(path).is_ok() {
                return Err(AppError::new("Invalid private session path."));
            }
            // A retry after deleting an isolated thread's last journal may
            // find its now-empty session directory already removed. Resolve
            // the nearest existing ancestor so missing parents cannot turn
            // an outside path (or symlink) into a permitted target.
            let mut ancestor = path;
            let mut suffix = Vec::new();
            while !ancestor.exists() {
                suffix.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| AppError::new("Invalid private session path."))?
                        .to_os_string(),
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| AppError::new("Invalid private session path."))?;
                if std::fs::symlink_metadata(ancestor)
                    .is_ok_and(|meta| meta.file_type().is_symlink())
                {
                    return Err(AppError::new("Invalid private session path."));
                }
            }
            let mut resolved = std::fs::canonicalize(ancestor)?;
            for name in suffix.into_iter().rev() {
                resolved.push(name);
            }
            resolved
        }
        Err(_) => {
            return Err(AppError::new(
                "The private session file is missing. Restore it before reopening this thread.",
            ))
        }
    };
    if !resolved.starts_with(base) {
        return Err(AppError::new(
            "This session does not belong to πDesk's private Pi.",
        ));
    }
    Ok(resolved)
}

/// The login shell's exported environment is cached in memory, never logged
/// or persisted. Finder-launched apps need the tool PATH configured in shell
/// profiles; private Pi does not inherit their credentials or Pi overrides.
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

/// Cap for one login-shell capture. Output beyond this is truncated for
/// parsing, but the shell is still drained so it can never block on a full
/// pipe buffer.
const SHELL_ENV_CAP_BYTES: usize = 1_048_576;
/// Whole-operation deadline: shell exit *and* stdout EOF share it, so a
/// backgrounded descendant holding stdout open cannot hang startup.
const SHELL_ENV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

#[cfg(unix)]
fn kill_shell_group(pgid: i32) {
    // The child is spawned as a process-group leader, so a negative pid
    // targets the whole group. SIGKILL cannot be caught or ignored. A
    // non-positive pgid must never reach kill: 0 would signal our own group.
    if pgid <= 0 {
        return;
    }
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

fn shell_env(shell: &str, flags: &[&str]) -> Option<std::collections::HashMap<String, String>> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + SHELL_ENV_TIMEOUT;
    let mut child = {
        let mut cmd = Command::new(shell);
        cmd.args(flags)
            .arg("printf '\\0__PIDESK_ENV_START__\\0'; env -0; printf '__PIDESK_ENV_END__\\0'")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd.spawn().ok()?
    };
    // `process_group(0)` makes the child its own group leader, so its pid is
    // the pgid. Captured on all platforms to keep the timeout paths uniform;
    // only the Unix kill path uses it.
    #[allow(unused_variables)]
    let pgid: i32 = child.id() as i32;
    let stdout = child.stdout.take()?;
    // Drain stdout on a helper thread so a large `env -0` can never fill the
    // pipe buffer and deadlock the shell. Only the first CAP bytes are kept
    // for parsing; the rest is still drained so the writer never blocks.
    let drain = std::thread::spawn(move || {
        use std::io::Read;
        let mut kept = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        let mut reader = stdout;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if !truncated {
                        let room = SHELL_ENV_CAP_BYTES.saturating_sub(kept.len());
                        if n <= room {
                            kept.extend_from_slice(&buf[..n]);
                        } else {
                            kept.extend_from_slice(&buf[..room]);
                            truncated = true;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        kept
    });
    // Wait for the shell itself within the deadline.
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            kill_shell_group(pgid);
            #[cfg(not(unix))]
            let _ = child.kill();
            // Reaps the direct child only; never waits for pipe EOF.
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        // The shell is gone but a backgrounded descendant may still hold
        // stdout open, which would leave the drain thread blocked on read
        // forever. Kill the group so the pipe reaches EOF, then return
        // without trusting anything for parsing.
        #[cfg(unix)]
        kill_shell_group(pgid);
        return None;
    }
    // The shell exited, but a backgrounded descendant may still hold stdout
    // open. EOF shares the same deadline: never wait for it unboundedly.
    while !drain.is_finished() {
        if Instant::now() >= deadline {
            #[cfg(unix)]
            kill_shell_group(pgid);
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    parse_shell_env(&drain.join().ok()?)
}

fn parse_shell_env(output: &[u8]) -> Option<std::collections::HashMap<String, String>> {
    const START: &[u8] = b"\0__PIDESK_ENV_START__\0";
    const END: &[u8] = b"__PIDESK_ENV_END__\0";
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
/// Private Pi receives only an allowlisted environment; diagnostics are still
/// scrubbed because tools and package managers can echo credentials.
pub fn redact_secrets(text: &str) -> String {
    let mut result = text.to_string();
    let secret_key = |key: &str| {
        let upper = key.to_ascii_uppercase();
        [
            "TOKEN",
            "API_KEY",
            "PASSWORD",
            "SECRET",
            "CREDENTIAL",
            "PROXY",
        ]
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
    // Proxy/package-manager errors can include URL userinfo even without a
    // matching environment value. Keep the host but remove login details.
    let mut from = 0;
    while let Some(offset) = result[from..].find("://") {
        let start = from + offset + 3;
        let end = result[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '\"' | '\''))
            .map(|offset| start + offset)
            .unwrap_or(result.len());
        if let Some(at) = result[start..end].rfind('@') {
            result.replace_range(start..start + at, "[REDACTED]");
            from = start + "[REDACTED]@".len();
        } else {
            from = end;
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

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_environment_ignores_startup_banner() {
        let bytes = b"welcome\n\0__PIDESK_ENV_START__\0PATH=/custom/bin:/usr/bin\0OPENAI_API_KEY=test-token\0__PIDESK_ENV_END__\0";
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
    fn private_commands_drop_inherited_pi_and_provider_configuration() {
        let root = Path::new("/tmp/private-desk");
        let mut command = tokio::process::Command::new("test");
        command
            .env("PI_CODING_AGENT_DIR", "/external")
            .env("PI_SESSION_FILE", "/external/session.jsonl")
            .env("OPENAI_API_KEY", "must-not-inherit")
            .env("NODE_OPTIONS", "--require=/external/hook.js");
        configure_private_command(&mut command, root);
        let env: std::collections::HashMap<_, _> = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert_eq!(env["PI_CODING_AGENT_DIR"], "/tmp/private-desk/agent");
        assert_eq!(
            env["PI_CODING_AGENT_SESSION_DIR"],
            "/tmp/private-desk/agent/sessions"
        );
        assert_eq!(env["PI_TELEMETRY"], "0");
        for key in [
            "PI_SESSION_FILE",
            "OPENAI_API_KEY",
            "NODE_OPTIONS",
            "NODE_PATH",
            "PI_PACKAGE_DIR",
            "PI_SUBAGENT_PI_BINARY",
        ] {
            assert!(!env.contains_key(key));
            assert!(!safe_process_env(key));
        }
        assert!(env["PATH"].starts_with("/tmp/private-desk/runtime/node_modules/.bin:"));
    }

    #[test]
    fn private_session_paths_reject_external_and_symlinked_targets() {
        let fixture = std::env::temp_dir().join(format!("pidesk-paths-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&fixture).unwrap();
        let root = fixture.join("private");
        let sessions = root.join("agent/sessions/project");
        ensure_private_directory(&root, &sessions).unwrap();
        let journal = sessions.join("session.jsonl");
        assert!(private_session_path_in(&root, &journal, false).is_ok());
        assert!(private_session_path_in(&root, &journal, true).is_err());
        std::fs::write(&journal, "private").unwrap();
        assert!(private_session_path_in(&root, &journal, true).is_ok());
        let external = fixture.join("external.jsonl");
        std::fs::write(&external, "untouched").unwrap();
        assert!(private_session_path_in(&root, &external, false).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&external, sessions.join("escape.jsonl")).unwrap();
            assert!(private_session_path_in(&root, &sessions.join("escape.jsonl"), true).is_err());
            std::os::unix::fs::symlink(&fixture, root.join("linked")).unwrap();
            assert!(ensure_private_directory(&root, &root.join("linked/new-dir")).is_err());
            assert!(!fixture.join("new-dir").exists());
        }
        assert_eq!(std::fs::read_to_string(external).unwrap(), "untouched");
        std::fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn parses_pi_version_format() {
        assert_eq!(parse_version("0.86.1\n"), Some("0.86.1".into()));
        assert_eq!(parse_version("unrelated output"), None);
    }

    #[test]
    fn redacts_bearer_and_key_values_from_diagnostics() {
        let text = "Authorization: Bearer example123 token=another456";
        let cleaned = redact_secrets(text);
        assert_eq!(cleaned, "Authorization: Bearer [REDACTED] token=[REDACTED]");
        assert_eq!(
            redact_secrets("Failed https://user:password@registry.example/path"),
            "Failed https://[REDACTED]@registry.example/path"
        );
    }
}

/// Physical memory footprint (what Activity Monitor calls "Memory") of one
/// process, or `None` when it has exited or cannot be inspected.
#[cfg(target_os = "macos")]
pub fn process_footprint(pid: i32) -> Option<u64> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast::<libc::rusage_info_t>(),
        )
    };
    (result == 0).then_some(info.ri_phys_footprint)
}

#[cfg(not(target_os = "macos"))]
pub fn process_footprint(_pid: i32) -> Option<u64> {
    None
}

/// Total footprint and member count of a process group. Each Pi runs as its
/// own group leader, so this covers Pi plus the tools it spawned (bash, node).
#[cfg(target_os = "macos")]
pub fn process_group_footprint(pgid: i32) -> Option<(u64, u32)> {
    // `PROC_PGRP_ONLY` from <libproc.h>; libc does not export it.
    const PROC_PGRP_ONLY: u32 = 2;
    const MAX_GROUP_MEMBERS: usize = 1024;
    let mut pids = vec![0 as libc::pid_t; MAX_GROUP_MEMBERS];
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_PGRP_ONLY,
            pgid as u32,
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int,
        )
    };
    if bytes <= 0 {
        return None;
    }
    let count = (bytes as usize / std::mem::size_of::<libc::pid_t>()).min(pids.len());
    let (total, members) = pids[..count]
        .iter()
        .filter(|pid| **pid > 0)
        .filter_map(|pid| process_footprint(*pid))
        .fold((0u64, 0u32), |(total, members), bytes| {
            (total.saturating_add(bytes), members + 1)
        });
    (members > 0).then_some((total, members))
}

#[cfg(not(target_os = "macos"))]
pub fn process_group_footprint(_pgid: i32) -> Option<(u64, u32)> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod memory_tests {
    use super::*;

    #[test]
    fn measures_own_process_and_group() {
        let pid = std::process::id() as i32;
        assert!(process_footprint(pid).is_some_and(|bytes| bytes > 0));
        let pgid = unsafe { libc::getpgid(0) };
        let (total, members) = process_group_footprint(pgid).expect("own group");
        assert!(members >= 1 && total > 0);
        assert!(process_footprint(i32::MAX).is_none());
    }
}
