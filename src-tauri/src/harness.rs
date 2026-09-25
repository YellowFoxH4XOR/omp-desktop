use crate::dto::{BackendEvent, HarnessInstallation, HarnessKind};
use crate::error::{AppError, AppResult};
use crate::store::Store;
use crate::util;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

const SETTING_OVERRIDE_OMP: &str = "executable_override.omp";
const SETTING_OVERRIDE_PI: &str = "executable_override.pi";

const MAX_PROBE_OUTPUT_BYTES: u64 = 1_048_576;

/// Fixed, allowlisted install commands. These are exactly what the onboarding
/// UI displays (via `install_commands`) and what `install_harness` executes
/// (via `install_command`); nothing else may be executed by install_harness.
/// The executed argv is intentionally unchanged (see E6 accepted risk): the
/// UI copy is derived from the same source so it cannot drift.
fn install_command(kind: HarnessKind) -> (&'static str, Vec<&'static str>) {
    match kind {
        HarnessKind::Omp => ("bun", vec!["install", "-g", "@oh-my-pi/pi-coding-agent"]),
        HarnessKind::Pi => (
            "npm",
            vec![
                "install",
                "-g",
                "--ignore-scripts",
                "@earendil-works/pi-coding-agent",
            ],
        ),
    }
}

fn install_command_display(kind: HarnessKind) -> String {
    let (prog, args) = install_command(kind);
    std::iter::once(prog)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

/// UI display copies of the fixed install commands, one per harness kind.
pub fn install_commands() -> Vec<crate::dto::HarnessInstallCommand> {
    [HarnessKind::Omp, HarnessKind::Pi]
        .iter()
        .map(|kind| crate::dto::HarnessInstallCommand {
            kind: *kind,
            command: install_command_display(*kind),
        })
        .collect()
}

fn override_setting_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Omp => SETTING_OVERRIDE_OMP,
        HarnessKind::Pi => SETTING_OVERRIDE_PI,
    }
}

fn override_digest_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Omp => "executable_override_digest.omp",
        HarnessKind::Pi => "executable_override_digest.pi",
    }
}

fn override_version_key(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::Omp => "executable_override_version.omp",
        HarnessKind::Pi => "executable_override_version.pi",
    }
}

/// Ownership/type gate for a user-supplied executable override. The override
/// becomes the binary every later spawn runs, so it must be an absolute path
/// to a regular file the current user owns, with no group/world write bits
/// (on Unix) and no symlink anywhere in the resolution (a symlink could be
/// repointed after validation).
fn check_override_file(path: &Path) -> AppResult<PathBuf> {
    if !path.is_absolute() {
        return Err(AppError::new(
            "The override must be an absolute path, not a relative one.",
        ));
    }
    // Reject symlinks before resolving: `symlink_metadata` does not follow
    // the final component, and `canonicalize` afterwards catches symlinked
    // parents. Either way the stored path is the fully resolved one.
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| AppError::new(format!("No executable exists at {}.", path.display())))?;
    if meta.file_type().is_symlink() {
        return Err(AppError::new(
            "The override must be a real file, not a symlink.",
        ));
    }
    let resolved = std::fs::canonicalize(path)
        .map_err(|_| AppError::new(format!("No executable exists at {}.", path.display())))?;
    if std::fs::symlink_metadata(&resolved)
        .map(|m| !m.file_type().is_file())
        .unwrap_or(true)
    {
        return Err(AppError::new(format!(
            "No executable exists at {}.",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(&resolved)
            .map_err(|_| AppError::new(format!("No executable exists at {}.", path.display())))?;
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(AppError::new(
                "The override must be owned by the current user.",
            ));
        }
        if meta.mode() & 0o022 != 0 {
            return Err(AppError::new(
                "The override must not be writable by group or others.",
            ));
        }
    }
    Ok(resolved)
}

/// Re-verify a stored override immediately before use: the file must still
/// pass the ownership/type gate and its digest must match the value recorded
/// at validation time. Fails closed when the override changed or the stored
/// digest is missing (e.g. written by an older build).
fn verify_stored_override_file(
    store: &Store,
    kind: HarnessKind,
    path: &Path,
) -> AppResult<PathBuf> {
    let resolved = check_override_file(path)?;
    let expected = store
        .get_setting_checked(override_digest_key(kind))?
        .filter(|s| !s.is_empty());
    let actual = util::file_digest(&resolved)?;
    if expected.as_deref() != Some(actual.as_str()) {
        return Err(AppError::new(format!(
            "{} override at {} changed since it was validated. Re-select it in Settings.",
            kind.display_name(),
            resolved.display()
        )));
    }
    Ok(resolved)
}

/// Well-known install locations checked after PATH lookups.
fn known_locations(kind: HarnessKind) -> Vec<PathBuf> {
    let home = util::home_dir();
    let name = kind.binary_name();
    let mut v = vec![
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
        home.join(".bun/bin").join(name),
        home.join(".local/bin").join(name),
        home.join(".npm-global/bin").join(name),
        home.join(".volta/bin").join(name),
        home.join(".local/share/pnpm").join(name),
        PathBuf::from("/usr/bin").join(name),
    ];
    if let Ok(nvm) = std::env::var("NVM_DIR") {
        v.push(PathBuf::from(nvm).join("current/bin").join(name));
    }
    v
}

async fn run_probe(
    path: &Path,
    args: &[&str],
    timeout: std::time::Duration,
    timeout_error: AppError,
) -> AppResult<std::process::Output> {
    let mut child = Command::new(path)
        .args(args)
        .env("PATH", util::merged_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::new(format!("Could not run {}: {e}", path.display())))?;
    let stdout = child.stdout.take().expect("probe stdout piped");
    let stderr = child.stderr.take().expect("probe stderr piped");
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stdout
            .take(MAX_PROBE_OUTPUT_BYTES)
            .read_to_end(&mut bytes)
            .await;
        bytes
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let _ = stderr
            .take(MAX_PROBE_OUTPUT_BYTES)
            .read_to_end(&mut bytes)
            .await;
        bytes
    });
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
            return Err(AppError::new(format!(
                "Could not inspect {}: {error}",
                path.display()
            )));
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
            return Err(timeout_error);
        }
    };
    Ok(std::process::Output {
        status,
        stdout: stdout_task.await.unwrap_or_default(),
        stderr: stderr_task.await.unwrap_or_default(),
    })
}

/// Validate a candidate executable by running `<path> --version`.
/// Returns the parsed version string on success.
async fn validate_executable(path: &Path, kind: HarnessKind) -> AppResult<String> {
    let out = run_probe(
        path,
        &["--version"],
        std::time::Duration::from_secs(15),
        AppError::new(format!(
            "{} at {} did not answer --version in time.",
            kind.display_name(),
            path.display()
        )),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(AppError::new(format!(
            "{} is not a working {} executable (exit {}).",
            path.display(),
            kind.display_name(),
            out.status.code().unwrap_or(-1)
        )));
    }
    let text = format!("{stdout}\n{stderr}");
    let version = util::parse_version(&text).ok_or_else(|| {
        AppError::new(format!(
            "{} did not report a version; is it really {}?",
            path.display(),
            kind.display_name()
        ))
    })?;
    match kind {
        HarnessKind::Omp if !stdout.trim_start().to_ascii_lowercase().starts_with("omp") => {
            return Err(AppError::new(
                "The selected executable reports a version, but is not OMP.",
            ));
        }
        HarnessKind::Pi => {
            // Pi's --version prints a bare semver; confirm its identity from
            // its CLI help without launching an agent or reading credentials.
            let help = run_probe(
                path,
                &["--help"],
                std::time::Duration::from_secs(15),
                AppError::new("Pi --help did not respond in time."),
            )
            .await?;
            let help_text = String::from_utf8_lossy(&help.stdout).to_ascii_lowercase();
            if !help.status.success()
                || !help_text.contains("pi - ai coding assistant")
                || !help_text.contains("--mode")
            {
                return Err(AppError::new(
                    "The selected executable is not a supported Pi CLI.",
                ));
            }
        }
        _ => {}
    }
    Ok(version)
}

pub struct HarnessRegistry {
    store: Arc<Store>,
    /// Resolved executable per kind, refreshed by detect/override/install.
    resolved: Mutex<std::collections::HashMap<HarnessKind, HarnessInstallation>>,
}

impl HarnessRegistry {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            resolved: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Currently resolved executable for a kind, if detection has run.
    pub fn executable(&self, kind: HarnessKind) -> Option<HarnessInstallation> {
        self.resolved.lock().get(&kind).cloned()
    }

    /// Executable path for spawning; errors with guidance when undetected.
    /// A stored override is re-verified by digest immediately before use
    /// (never executed just to detect it); non-override candidates keep the
    /// existing `--version` identity gate.
    pub async fn executable_path(&self, kind: HarnessKind) -> AppResult<PathBuf> {
        // A configured override always takes precedence over any cached
        // install and fails closed, so check the store first: a missing,
        // changed, or unreadable override must error rather than silently
        // run a stale cached binary (or skip to a different one).
        let configured = self.store.get_setting_checked(override_setting_key(kind))?;
        if let Some(ov) = configured {
            if !ov.trim().is_empty() {
                return verify_stored_override_file(&self.store, kind, Path::new(ov.trim()))
                    .map_err(|e| {
                        self.resolved.lock().remove(&kind);
                        e
                    });
            }
        }
        if let Some(inst) = self.executable(kind) {
            return Ok(PathBuf::from(inst.path));
        }
        // Lazy detection so commands work even if detect_harnesses was skipped.
        self.detect_kind(kind).await
    }

    /// Detect one kind, verifying a stored override by digest and validating
    /// non-override candidates by execution as before. A configured override
    /// takes precedence and fails closed: when the user's chosen executable
    /// is missing, changed, or unreadable, spawning must error rather than
    /// silently run a different binary.
    async fn detect_kind(&self, kind: HarnessKind) -> AppResult<PathBuf> {
        let configured = self.store.get_setting_checked(override_setting_key(kind))?;
        if let Some(ov) = configured {
            if !ov.trim().is_empty() {
                let resolved = verify_stored_override_file(&self.store, kind, Path::new(ov.trim()))
                    .map_err(|e| {
                        // Fail closed: never serve a stale cached install
                        // after the override broke.
                        self.resolved.lock().remove(&kind);
                        e
                    })?;
                let version = self
                    .store
                    .get_setting_checked(override_version_key(kind))?
                    .filter(|s| !s.is_empty())
                    .unwrap_or_default();
                let inst = HarnessInstallation {
                    kind,
                    path: resolved.to_string_lossy().to_string(),
                    version,
                    source: "override".to_string(),
                };
                self.resolved.lock().insert(kind, inst.clone());
                return Ok(PathBuf::from(inst.path));
            }
        }
        for (path, source) in self.candidates(kind) {
            if source == "override" {
                continue;
            }
            if let Ok(version) = validate_executable(&path, kind).await {
                let inst = HarnessInstallation {
                    kind,
                    path: path.to_string_lossy().to_string(),
                    version,
                    source,
                };
                self.resolved.lock().insert(kind, inst.clone());
                return Ok(PathBuf::from(inst.path));
            }
        }
        Err(AppError::new(format!(
            "{} is not installed or could not be found. Install it or choose the executable in Settings.",
            kind.display_name()
        )))
    }

    /// Ordered candidate list: override → PATH → login-shell PATH → known locations.
    /// `detect`/`detect_kind` check the override first via the checked API
    /// and fail closed, so this list only supplies it as a fallback entry;
    /// a database failure here skips the entry rather than inventing one.
    fn candidates(&self, kind: HarnessKind) -> Vec<(PathBuf, String)> {
        let mut out: Vec<(PathBuf, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |p: PathBuf, source: &str| {
            let key = p.to_string_lossy().to_string();
            if seen.insert(key) {
                out.push((p, source.to_string()));
            }
        };
        match self.store.get_setting_checked(override_setting_key(kind)) {
            Ok(Some(ov)) if !ov.trim().is_empty() => {
                push(PathBuf::from(ov), "override");
            }
            Ok(_) => {}
            Err(_) => {}
        }
        let name = kind.binary_name();
        if let Ok(p) = which::which(name) {
            push(p, "PATH");
        }
        if let Some(login_path) = util::login_shell_path() {
            if let Some(p) = util::which_in_path(name, &login_path) {
                push(p, "login shell PATH");
            }
        }
        for p in known_locations(kind) {
            if util::is_executable(&p) {
                push(p, "known location");
            }
        }
        out
    }

    /// Full detection pass; returns every validated installation. A stored
    /// override is verified by digest (never executed just to detect it);
    /// non-override candidates keep the existing `--version` identity gate.
    /// A configured override takes precedence and fails closed: a bad
    /// override reports nothing for that kind instead of silently returning
    /// a different binary as the user's choice.
    pub async fn detect(&self) -> Vec<HarnessInstallation> {
        let mut found = Vec::new();
        for kind in [HarnessKind::Omp, HarnessKind::Pi] {
            let configured = self
                .store
                .get_setting_checked(override_setting_key(kind))
                .ok()
                .flatten()
                .filter(|s| !s.trim().is_empty());
            if let Some(ov) = configured {
                match verify_stored_override_file(&self.store, kind, Path::new(ov.trim())) {
                    Ok(resolved) => {
                        let version = self
                            .store
                            .get_setting_checked(override_version_key(kind))
                            .ok()
                            .flatten()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_default();
                        let inst = HarnessInstallation {
                            kind,
                            path: resolved.to_string_lossy().to_string(),
                            version,
                            source: "override".to_string(),
                        };
                        self.resolved.lock().insert(kind, inst.clone());
                        found.push(inst);
                    }
                    Err(_) => {
                        // Fail closed: drop any previously cached install for
                        // this kind so a broken override is never served stale.
                        self.resolved.lock().remove(&kind);
                    }
                }
                continue;
            }
            for (path, source) in self.candidates(kind) {
                if source == "override" {
                    continue;
                }
                match validate_executable(&path, kind).await {
                    Ok(version) => {
                        let inst = HarnessInstallation {
                            kind,
                            path: path.to_string_lossy().to_string(),
                            version,
                            source,
                        };
                        self.resolved.lock().insert(kind, inst.clone());
                        found.push(inst);
                        break;
                    }
                    Err(_) => continue,
                }
            }
        }
        found
    }

    /// Pin a user-selected executable after validating it. The file must pass
    /// the ownership/type gate first; validation still executes the chosen
    /// binary once (the user explicitly picked it), and the resulting digest
    /// plus version are stored so later detection/spawn only re-verifies the
    /// digest without re-executing anything.
    pub async fn set_override(
        &self,
        kind: HarnessKind,
        path: &str,
    ) -> AppResult<HarnessInstallation> {
        let resolved = check_override_file(Path::new(path))?;
        let version = validate_executable(&resolved, kind).await?;
        let digest = util::file_digest(&resolved)?;
        self.store
            .set_setting(override_setting_key(kind), &resolved.to_string_lossy())?;
        self.store.set_setting(override_digest_key(kind), &digest)?;
        self.store
            .set_setting(override_version_key(kind), &version)?;
        let inst = HarnessInstallation {
            kind,
            path: resolved.to_string_lossy().to_string(),
            version,
            source: "override".to_string(),
        };
        self.resolved.lock().insert(kind, inst.clone());
        Ok(inst)
    }

    /// Run the fixed install command, streaming output lines as
    /// `install_progress` events and finishing with `install_finished`.
    pub async fn install(&self, kind: HarnessKind, app: AppHandle) -> AppResult<()> {
        let (prog, args) = install_command(kind);
        let display = install_command_display(kind);
        let emit = |ev: BackendEvent| {
            let _ = app.emit("desktop-event", ev);
        };
        emit(BackendEvent::InstallProgress {
            kind,
            line: format!("$ {display}"),
        });
        let path_env = util::merged_path();
        let resolved_prog =
            util::which_in_path(prog, &path_env).unwrap_or_else(|| PathBuf::from(prog));
        let mut child = match Command::new(&resolved_prog)
            .args(&args)
            .env("PATH", &path_env)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("Could not start `{display}`. Is {prog} installed? ({e})");
                emit(BackendEvent::InstallFinished {
                    kind,
                    success: false,
                    error: Some(msg.clone()),
                });
                return Err(AppError::new(msg));
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut tasks = Vec::new();
        if let Some(stream) = stdout {
            let app3 = app.clone();
            tasks.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = util::redact_secrets(line.trim_end());
                    if line.is_empty() {
                        continue;
                    }
                    let _ = app3.emit(
                        "desktop-event",
                        BackendEvent::InstallProgress { kind, line },
                    );
                }
            }));
        }
        if let Some(stream) = stderr {
            let app3 = app.clone();
            tasks.push(tokio::spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = util::redact_secrets(line.trim_end());
                    if line.is_empty() {
                        continue;
                    }
                    let _ = app3.emit(
                        "desktop-event",
                        BackendEvent::InstallProgress { kind, line },
                    );
                }
            }));
        }
        let status = child.wait().await;
        for t in tasks {
            let _ = t.await;
        }
        match status {
            Ok(s) if s.success() => {
                emit(BackendEvent::InstallProgress {
                    kind,
                    line: "Install finished. Verifying…".to_string(),
                });
                // Re-detect so the new installation is registered.
                let found = self.detect().await;
                if let Some(inst) = found.iter().find(|i| i.kind == kind) {
                    emit(BackendEvent::InstallProgress {
                        kind,
                        line: format!(
                            "{} {} detected at {}",
                            kind.display_name(),
                            inst.version,
                            inst.path
                        ),
                    });
                    emit(BackendEvent::InstallFinished {
                        kind,
                        success: true,
                        error: None,
                    });
                    Ok(())
                } else {
                    let msg = format!(
                        "Install completed but {} still could not be found. You may need to restart the app or select the executable manually.",
                        kind.display_name()
                    );
                    emit(BackendEvent::InstallFinished {
                        kind,
                        success: false,
                        error: Some(msg.clone()),
                    });
                    Err(AppError::new(msg))
                }
            }
            Ok(s) => {
                let msg = format!(
                    "`{display}` failed with exit code {}.",
                    s.code().unwrap_or(-1)
                );
                emit(BackendEvent::InstallFinished {
                    kind,
                    success: false,
                    error: Some(msg.clone()),
                });
                Err(AppError::new(msg))
            }
            Err(e) => {
                let msg = format!("Install failed: {e}");
                emit(BackendEvent::InstallFinished {
                    kind,
                    success: false,
                    error: Some(msg.clone()),
                });
                Err(AppError::new(msg))
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn rejects_unrelated_executable_that_prints_semver() {
        let dir = std::env::temp_dir().join(format!("omp-harness-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake-agent");
        std::fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 1.2.3; else echo 'not a coding agent'; fi\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_executable(&fake, HarnessKind::Omp).await.is_err());
        assert!(validate_executable(&fake, HarnessKind::Pi).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires both OMP and Pi installed on this machine"]
    async fn detects_system_harnesses_without_changing_credentials() {
        let dir = std::env::temp_dir().join(format!("omp-detect-test-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::open(&crate::store::db_path(&dir)).unwrap());
        let registry = HarnessRegistry::new(store);
        let found = registry.detect().await;
        assert!(found
            .iter()
            .any(|install| install.kind == HarnessKind::Omp && !install.version.is_empty()));
        assert!(found
            .iter()
            .any(|install| install.kind == HarnessKind::Pi && !install.version.is_empty()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
