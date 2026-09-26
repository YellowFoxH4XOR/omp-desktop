use crate::dto::{
    BackendEvent, HarnessInstallCommand, HarnessInstallation, HarnessKind, InstallStage,
};
use crate::error::{AppError, AppResult};
use crate::util;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

// Pin the RPC contract this desktop build supports; updates are never implicit.
const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
const PI_PACKAGE_SPEC: &str = "@earendil-works/pi-coding-agent@0.87.1";
const INCOMPLETE_MARKER: &str = ".pidesk-install-incomplete";
const MAX_PROBE_BYTES: u64 = 1024 * 1024;
const MAX_LOG_LINE_BYTES: usize = 4096;
const MAX_LOG_LINES: usize = 5000;
const INSTALL_TIMEOUT_SECS: u64 = 600;
type Emit = Arc<dyn Fn(BackendEvent) + Send + Sync>;

fn executable_in(root: &Path) -> PathBuf {
    root.join("runtime/node_modules/.bin/pi")
}

pub(crate) fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@=".contains(c))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Fixed argv shared by the command display and execution. A private working
/// directory and explicit npm configs avoid project/user global-install flags.
fn install_args(root: &Path) -> Vec<String> {
    vec![
        "install".into(),
        "--prefix".into(),
        root.join("runtime").to_string_lossy().into_owned(),
        "--global=false".into(),
        "--ignore-scripts".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--progress=false".into(),
        "--color=false".into(),
        "--save-exact".into(),
        "--userconfig".into(),
        root.join("npm-user.conf").to_string_lossy().into_owned(),
        "--globalconfig".into(),
        root.join("npm-global.conf").to_string_lossy().into_owned(),
        "--cache".into(),
        root.join("npm-cache").to_string_lossy().into_owned(),
        PI_PACKAGE_SPEC.into(),
    ]
}

/// Shell-safe, credential-free invocation shared by Settings and the PTY's
/// `pi` function. Only πDesk's private executable and profile reach Pi.
pub(crate) fn private_pi_invocation(root: &Path) -> String {
    let agent = shell_quote(&root.join("agent").to_string_lossy());
    let sessions = shell_quote(&root.join("agent/sessions").to_string_lossy());
    let bin = shell_quote(&root.join("runtime/node_modules/.bin").to_string_lossy());
    let executable = shell_quote(&executable_in(root).to_string_lossy());
    let home = shell_quote(&util::home_dir().to_string_lossy());
    format!("/usr/bin/env -i HOME={home} USER=\"${{USER:-}}\" PATH={bin}:\"$PATH\" TERM=\"${{TERM:-xterm-256color}}\" PI_CODING_AGENT_DIR={agent} PI_CODING_AGENT_SESSION_DIR={sessions} PI_SKIP_VERSION_CHECK=1 PI_TELEMETRY=0 {executable} --no-approve --session-dir {sessions}")
}

fn install_plan(root: &Path) -> HarnessInstallCommand {
    let command = std::iter::once("npm".to_string())
        .chain(install_args(root).iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    HarnessInstallCommand {
        kind: HarnessKind::Pi,
        command,
        install_path: root.join("runtime").to_string_lossy().into_owned(),
        agent_dir: root.join("agent").to_string_lossy().into_owned(),
        // Copyable, credential-free display; don't embed proxy values or keys.
        login_command: private_pi_invocation(root),
    }
}

pub fn install_commands() -> Vec<HarnessInstallCommand> {
    vec![install_plan(&util::pidesk_root())]
}

/// No PATH lookup for Pi, no user override, and no fallback outside the private
/// package. The npm-created .bin symlink is allowed only within that package.
fn managed_executable(root: &Path) -> AppResult<PathBuf> {
    util::check_owned_path(root, true)?;
    for dir in [
        "runtime",
        "runtime/node_modules",
        "runtime/node_modules/.bin",
        "runtime/node_modules/@earendil-works",
        "runtime/node_modules/@earendil-works/pi-coding-agent",
    ] {
        util::check_owned_path(&root.join(dir), true)?;
    }
    let package = root.join("runtime/node_modules").join(PI_PACKAGE);
    let canonical_package = std::fs::canonicalize(&package)?;
    let manifest = package.join("package.json");
    util::check_owned_path(&manifest, false)?;
    if std::fs::metadata(&manifest)?.len() > MAX_PROBE_BYTES {
        return Err(AppError::new(
            "The private Pi package manifest is too large.",
        ));
    }
    let manifest: Value = serde_json::from_slice(&std::fs::read(manifest)?)?;
    if manifest.get("name").and_then(Value::as_str) != Some(PI_PACKAGE) {
        return Err(AppError::new(
            "The private package is not Pi. Retry installation.",
        ));
    }
    let resolved = std::fs::canonicalize(executable_in(root))
        .map_err(|_| AppError::new("Private Pi is missing. Choose Install Pi in πDesk."))?;
    if !resolved.starts_with(&canonical_package) {
        return Err(AppError::new(
            "The private Pi executable points outside its own package.",
        ));
    }
    util::check_owned_path(&resolved, false)?;
    if !util::is_executable(&resolved) {
        return Err(AppError::new(
            "The private Pi executable is not runnable. Retry installation.",
        ));
    }
    Ok(resolved)
}

async fn stop_child(child: &mut Child) {
    // Only signal the original process group while its leader is unreaped.
    if matches!(child.try_wait(), Ok(None)) {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = child.start_kill();
    }
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await;
}

fn command_for(root: &Path, executable: &Path) -> Command {
    let mut command = Command::new(executable);
    util::configure_private_command(&mut command, root);
    #[cfg(unix)]
    command.process_group(0);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn probe(root: &Path, executable: &Path, args: &[&str]) -> AppResult<String> {
    let mut child = command_for(root, executable)
        .args(args)
        .env("PI_OFFLINE", "1")
        .spawn()
        .map_err(|error| {
            AppError::new(format!("Could not run {}: {error}", executable.display()))
        })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_PROBE_BYTES)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let err_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(MAX_PROBE_BYTES)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let status = tokio::time::timeout(std::time::Duration::from_secs(15), child.wait()).await;
    if !matches!(status, Ok(Ok(_))) {
        stop_child(&mut child).await;
        out_task.abort();
        err_task.abort();
        return Err(AppError::new(format!(
            "{} did not respond in time.",
            executable.display()
        )));
    }
    let status = status.unwrap()?;
    let out_abort = out_task.abort_handle();
    let err_abort = err_task.abort_handle();
    let output = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        (out_task.await, err_task.await)
    })
    .await;
    let Ok((Ok(Ok(stdout)), Ok(Ok(stderr)))) = output else {
        out_abort.abort();
        err_abort.abort();
        return Err(AppError::new("Could not read the executable's response."));
    };
    if !status.success() {
        return Err(AppError::new(format!(
            "{} failed its readiness check (exit {}).",
            executable.display(),
            status.code().unwrap_or(-1)
        )));
    }
    Ok(format!(
        "{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    ))
}

/// Identity of a validated install. Launching Pi to re-check `--version` and
/// `--help` costs two Node startups (~500 ms), so the result is reused until
/// the executable or package manifest changes on disk.
#[derive(Debug, Clone, PartialEq)]
struct InstallFingerprint {
    executable: PathBuf,
    executable_len: u64,
    executable_modified: Option<std::time::SystemTime>,
    manifest_len: u64,
    manifest_modified: Option<std::time::SystemTime>,
}

fn fingerprint(root: &Path, executable: &Path) -> AppResult<InstallFingerprint> {
    let exe = std::fs::metadata(executable)?;
    let manifest = std::fs::metadata(
        root.join("runtime/node_modules")
            .join(PI_PACKAGE)
            .join("package.json"),
    )?;
    Ok(InstallFingerprint {
        executable: executable.to_path_buf(),
        executable_len: exe.len(),
        executable_modified: exe.modified().ok(),
        manifest_len: manifest.len(),
        manifest_modified: manifest.modified().ok(),
    })
}

async fn validate_pi(root: &Path) -> AppResult<HarnessInstallation> {
    let executable = managed_executable(root)?;
    let version_text = probe(root, &executable, &["--version"]).await?;
    let version = util::parse_version(&version_text)
        .ok_or_else(|| AppError::new("Private Pi did not report its version."))?;
    let help = probe(
        root,
        &executable,
        &[
            "--help",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--no-approve",
        ],
    )
    .await?
    .to_ascii_lowercase();
    if !help.contains("pi - ai coding assistant")
        || !help.contains("--mode")
        || !help.contains("--no-approve")
    {
        return Err(AppError::new(
            "The private executable is not a supported Pi CLI. Retry installation.",
        ));
    }
    Ok(HarnessInstallation {
        kind: HarnessKind::Pi,
        path: executable.to_string_lossy().into_owned(),
        version,
        source: "managed".into(),
    })
}

fn emit_line(emit: &Emit, count: &AtomicUsize, bytes: &[u8], truncated: bool) {
    let index = count.fetch_add(1, Ordering::Relaxed);
    if index > MAX_LOG_LINES {
        return;
    }
    let line = if index == MAX_LOG_LINES {
        "[Further installer output omitted; installation is still running.]".to_string()
    } else {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_end_matches('\r');
        if text.is_empty() {
            return;
        }
        let mut line = util::redact_secrets(text);
        if truncated {
            line.push_str(" [truncated]");
        }
        line
    };
    emit(BackendEvent::InstallProgress {
        kind: HarnessKind::Pi,
        line,
    });
}

/// Drain both streams even after limits are reached so a noisy subprocess
/// cannot fill a pipe, allocate unbounded lines, or flood the event queue.
async fn stream_output(
    mut stream: impl AsyncRead + Unpin,
    emit: Emit,
    count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut chunk = [0u8; 4096];
    let mut line = Vec::new();
    let mut truncated = false;
    loop {
        let length = stream.read(&mut chunk).await?;
        if length == 0 {
            break;
        }
        for byte in &chunk[..length] {
            if *byte == b'\n' || *byte == b'\r' {
                emit_line(&emit, &count, &line, truncated);
                line.clear();
                truncated = false;
            } else if line.len() < MAX_LOG_LINE_BYTES {
                line.push(*byte);
            } else {
                truncated = true;
            }
        }
    }
    if !line.is_empty() {
        emit_line(&emit, &count, &line, truncated);
    }
    Ok(())
}

pub struct HarnessRegistry {
    root: PathBuf,
    validated: parking_lot::Mutex<Option<(InstallFingerprint, HarnessInstallation)>>,
    closing: AtomicBool,
    installing: AtomicBool,
    stop_install: tokio::sync::Notify,
    install_done: tokio::sync::Notify,
}

struct InstallActivity<'a>(&'a HarnessRegistry);
impl Drop for InstallActivity<'_> {
    fn drop(&mut self) {
        self.0.installing.store(false, Ordering::SeqCst);
        self.0.install_done.notify_one();
    }
}

impl HarnessRegistry {
    pub fn new() -> Self {
        Self::at_root(util::pidesk_root())
    }

    fn at_root(root: PathBuf) -> Self {
        Self {
            root,
            validated: parking_lot::Mutex::new(None),
            closing: AtomicBool::new(false),
            installing: AtomicBool::new(false),
            stop_install: tokio::sync::Notify::new(),
            install_done: tokio::sync::Notify::new(),
        }
    }

    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.stop_install.notify_one();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while self.installing.load(Ordering::SeqCst) {
                self.install_done.notified().await;
            }
        })
        .await;
    }

    fn check_open(&self) -> AppResult<()> {
        if self.closing.load(Ordering::SeqCst) {
            Err(AppError::new(
                "Installation stopped because πDesk is closing. Retry next time you open the app.",
            ))
        } else {
            Ok(())
        }
    }

    pub async fn executable_path(&self, _kind: HarnessKind) -> AppResult<PathBuf> {
        if self
            .root
            .join("runtime")
            .join(INCOMPLETE_MARKER)
            .try_exists()?
        {
            return Err(AppError::new(
                "Private Pi installation is incomplete. Retry installation in πDesk.",
            ));
        }
        self.validated_installation()
            .await
            .map(|installation| PathBuf::from(installation.path))
    }

    /// Ownership and symlink checks always run (they are filesystem-only);
    /// the Node probes run only when the install changed since last success.
    async fn validated_installation(&self) -> AppResult<HarnessInstallation> {
        let executable = managed_executable(&self.root)?;
        let current = fingerprint(&self.root, &executable)?;
        if let Some((cached, installation)) = self.validated.lock().as_ref() {
            if *cached == current {
                return Ok(installation.clone());
            }
        }
        let installation = validate_pi(&self.root).await?;
        let verified = fingerprint(&self.root, &managed_executable(&self.root)?)?;
        *self.validated.lock() = Some((verified, installation.clone()));
        Ok(installation)
    }

    /// Detection never creates directories, installs packages, or runs system Pi.
    pub async fn detect(&self) -> Vec<HarnessInstallation> {
        if self
            .root
            .join("runtime")
            .join(INCOMPLETE_MARKER)
            .try_exists()
            .unwrap_or(true)
        {
            return Vec::new();
        }
        match self.validated_installation().await {
            Ok(installation) => vec![installation],
            Err(_) => {
                *self.validated.lock() = None;
                Vec::new()
            }
        }
    }

    pub async fn install(&self, _kind: HarnessKind, app: AppHandle) -> AppResult<()> {
        let emit: Emit = Arc::new(move |event| {
            let _ = app.emit("desktop-event", event);
        });
        self.install_with_events(emit).await
    }

    async fn install_with_events(&self, emit: Emit) -> AppResult<()> {
        // IPC also guards concurrent calls; keep shutdown ownership here.
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(AppError::new("Pi installation is already running."));
        }
        let _activity = InstallActivity(self);
        let result = self.perform_install(&emit).await;
        let error = result
            .as_ref()
            .err()
            .map(|error| util::redact_secrets(&error.to_string()));
        emit(BackendEvent::InstallFinished {
            kind: HarnessKind::Pi,
            success: result.is_ok(),
            error,
        });
        result
    }

    async fn perform_install(&self, emit: &Emit) -> AppResult<()> {
        emit(BackendEvent::InstallStage {
            kind: HarnessKind::Pi,
            stage: InstallStage::Preparing,
        });
        self.check_open()?;
        if self.root.try_exists()? {
            util::check_owned_path(&self.root, true)?;
        }
        // This endpoint is setup, not a live-runtime upgrade. Never replace a
        // working installation while it may have active sessions.
        if !self.detect().await.is_empty() {
            emit(BackendEvent::InstallProgress {
                kind: HarnessKind::Pi,
                line: "Private Pi is already installed and verified.".into(),
            });
            return Ok(());
        }
        let path = util::merged_path();
        let node = util::which_in_path("node", &path).ok_or_else(|| AppError::new("Node.js 22.19+ is required. Install Node.js, then retry here; your system Pi is not used."))?;
        let npm = util::which_in_path("npm", &path).ok_or_else(|| {
            AppError::new("npm is required. Install Node.js with npm, then retry.")
        })?;
        self.install_package(emit, &node, &npm).await
    }

    async fn install_package(&self, emit: &Emit, node: &Path, npm: &Path) -> AppResult<()> {
        self.check_open()?;
        let node_version = probe(&self.root, node, &["--version"]).await?;
        self.check_open()?;
        let version = util::parse_version(&node_version)
            .ok_or_else(|| AppError::new("Could not determine the Node.js version."))?;
        let parts: Vec<u64> = version
            .split('.')
            .take(2)
            .filter_map(|part| part.parse().ok())
            .collect();
        if parts.len() != 2 || parts[0] < 22 || (parts[0] == 22 && parts[1] < 19) {
            return Err(AppError::new(
                "Node.js 22.19 or newer is required. Update Node.js, then retry.",
            ));
        }
        emit(BackendEvent::InstallProgress {
            kind: HarnessKind::Pi,
            line: format!("Node.js {version} found. Preparing πDesk's private directories…"),
        });
        for dir in ["", "runtime", "agent", "agent/sessions", "npm-cache"] {
            util::ensure_private_directory(&self.root, &self.root.join(dir))?;
        }
        // Serialize across application instances as well as the IPC guard.
        let lock_path = self.root.join("install.lock");
        let lock_file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                util::check_owned_path(&lock_path, false)?;
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&lock_path)?
            }
            Err(error) => return Err(error.into()),
        };
        lock_file.try_lock().map_err(|_| {
            AppError::new(
                "Another πDesk instance is installing Pi. Wait for it to finish, then check again.",
            )
        })?;
        // Empty app-owned configs prevent inherited npm prefix/scripts/registry
        // settings from changing this fixed local install.
        for name in ["npm-user.conf", "npm-global.conf"] {
            let config = self.root.join(name);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&config)
            {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    util::check_owned_path(&config, false)?;
                    if std::fs::metadata(&config)?.len() != 0 {
                        return Err(AppError::new("πDesk's installer npm config must be empty. Remove its custom contents before retrying."));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        let marker = self.root.join("runtime").join(INCOMPLETE_MARKER);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
        {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                util::check_owned_path(&marker, false)?;
            }
            Err(error) => return Err(error.into()),
        }
        emit(BackendEvent::InstallProgress {
            kind: HarnessKind::Pi,
            line: format!("$ {}", install_plan(&self.root).command),
        });
        emit(BackendEvent::InstallStage {
            kind: HarnessKind::Pi,
            stage: InstallStage::Installing,
        });
        self.check_open()?;
        let mut child = command_for(&self.root, npm)
            .args(install_args(&self.root))
            .current_dir(self.root.join("runtime"))
            .env("PATH", util::merged_path())
            .env("NO_COLOR", "1")
            .env("CI", "true")
            .spawn()
            .map_err(|error| AppError::new(format!("Could not start npm: {error}")))?;
        let count = Arc::new(AtomicUsize::new(0));
        let out_task = tokio::spawn(stream_output(
            child.stdout.take().expect("piped stdout"),
            emit.clone(),
            count.clone(),
        ));
        let err_task = tokio::spawn(stream_output(
            child.stderr.take().expect("piped stderr"),
            emit.clone(),
            count,
        ));
        let status = tokio::select! {
            status = tokio::time::timeout(std::time::Duration::from_secs(INSTALL_TIMEOUT_SECS), child.wait()) => status,
            _ = self.stop_install.notified() => {
                stop_child(&mut child).await;
                out_task.abort(); err_task.abort();
                return Err(AppError::new("Installation stopped because πDesk is closing. Retry next time you open the app."));
            }
        };
        let Ok(status) = status else {
            stop_child(&mut child).await;
            out_task.abort();
            err_task.abort();
            return Err(AppError::new(
                "Pi installation timed out after 10 minutes. Check your connection and retry.",
            ));
        };
        let out_abort = out_task.abort_handle();
        let err_abort = err_task.abort_handle();
        let drained = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            (out_task.await, err_task.await)
        })
        .await;
        if !matches!(drained, Ok((Ok(Ok(())), Ok(Ok(()))))) {
            out_abort.abort();
            err_abort.abort();
            return Err(AppError::new(
                "Installer output did not close cleanly. Retry installation.",
            ));
        }
        let status = status?;
        if !status.success() {
            return Err(AppError::new(format!("npm exited with code {}. Review the terminal output, fix the reported issue, and retry.", status.code().unwrap_or(-1))));
        }
        emit(BackendEvent::InstallStage {
            kind: HarnessKind::Pi,
            stage: InstallStage::Verifying,
        });
        emit(BackendEvent::InstallProgress {
            kind: HarnessKind::Pi,
            line: "Packages installed. Verifying the private Pi executable…".into(),
        });
        self.check_open()?;
        *self.validated.lock() = None;
        let installation = self.validated_installation().await?;
        self.check_open()?;
        std::fs::remove_file(marker)?;
        emit(BackendEvent::InstallProgress {
            kind: HarnessKind::Pi,
            line: format!(
                "Pi {} is ready at {}. Your existing Pi was not changed.",
                installation.version, installation.path
            ),
        });
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("pidesk-runtime-{}", uuid::Uuid::new_v4()))
    }

    fn fake_private_pi(root: &Path) {
        let package = root.join("runtime/node_modules").join(PI_PACKAGE);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(root.join("runtime/node_modules/.bin")).unwrap();
        std::fs::write(
            package.join("package.json"),
            format!("{{\"name\":\"{PI_PACKAGE}\"}}"),
        )
        .unwrap();
        let executable = package.join("cli.js");
        std::fs::write(&executable, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 0.87.1; else echo 'Pi - AI coding assistant --mode --no-approve'; fi\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&executable, executable_in(root)).unwrap();
    }

    #[tokio::test]
    async fn missing_runtime_never_falls_back_or_creates_files() {
        let root = root();
        let registry = HarnessRegistry::at_root(root.clone());
        assert!(registry.detect().await.is_empty());
        assert!(registry.executable_path(HarnessKind::Pi).await.is_err());
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn validation_probes_run_once_until_the_install_changes() {
        let root = root();
        fake_private_pi(&root);
        let executable = root
            .join("runtime/node_modules")
            .join(PI_PACKAGE)
            .join("cli.js");
        let log = root.join("probes.log");
        let script = |extra: &str| {
            format!(
                "#!/bin/sh\necho run >> '{}'\nif [ \"$1\" = \"--version\" ]; then echo 0.87.1; else echo 'Pi - AI coding assistant --mode --no-approve'; fi\n{extra}",
                log.display()
            )
        };
        std::fs::write(&executable, script("")).unwrap();
        let probes = || {
            std::fs::read_to_string(&log)
                .map(|s| s.lines().count())
                .unwrap_or(0)
        };
        let registry = HarnessRegistry::at_root(root.clone());

        registry.executable_path(HarnessKind::Pi).await.unwrap();
        assert_eq!(probes(), 2, "first use runs --version and --help");
        registry.executable_path(HarnessKind::Pi).await.unwrap();
        assert_eq!(registry.detect().await.len(), 1);
        assert_eq!(probes(), 2, "unchanged install is not re-probed");

        // A changed executable (new size) must be validated again.
        std::fs::write(&executable, script("# updated\n")).unwrap();
        registry.executable_path(HarnessKind::Pi).await.unwrap();
        assert_eq!(probes(), 4);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn only_complete_private_package_is_detected() {
        let root = root();
        fake_private_pi(&root);
        let registry = HarnessRegistry::at_root(root.clone());
        let installs = registry.detect().await;
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].source, "managed");
        std::fs::write(root.join("runtime").join(INCOMPLETE_MARKER), "").unwrap();
        assert!(registry.detect().await.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn preparation_failure_emits_one_finished_event_without_running_installer() {
        let root = root();
        std::fs::write(&root, "not a directory").unwrap();
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let captured = events.clone();
        let emit: Emit = Arc::new(move |event| captured.lock().push(event));
        let registry = HarnessRegistry::at_root(root.clone());
        assert!(registry.install_with_events(emit).await.is_err());
        let events = events.lock();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            BackendEvent::InstallStage {
                stage: InstallStage::Preparing,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            BackendEvent::InstallFinished { success: false, .. }
        ));
        assert_eq!(std::fs::read_to_string(&root).unwrap(), "not a directory");
        std::fs::remove_file(root).unwrap();
    }

    #[tokio::test]
    async fn fake_installer_streams_failure_and_verifies_success_on_retry() {
        let root = root();
        fake_private_pi(&root);
        let marker = root.join("runtime").join(INCOMPLETE_MARKER);
        std::fs::write(&marker, "").unwrap();
        let node = root.join("fake-node");
        let npm = root.join("fake-npm");
        std::fs::write(&node, "#!/bin/sh\necho v22.19.0\n").unwrap();
        std::fs::write(&npm, "#!/bin/sh\necho 'Registry unavailable' >&2\nexit 7\n").unwrap();
        for bin in [&node, &npm] {
            std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let captured = events.clone();
        let emit: Emit = Arc::new(move |event| captured.lock().push(event));
        let registry = HarnessRegistry::at_root(root.clone());
        let failure = registry
            .install_package(&emit, &node, &npm)
            .await
            .unwrap_err();
        assert!(failure.to_string().contains("code 7"));
        assert!(marker.exists());
        assert!(registry.detect().await.is_empty());
        assert!(events.lock().iter().any(|event| matches!(event, BackendEvent::InstallProgress { line, .. } if line == "Registry unavailable")));
        std::fs::write(&npm, "#!/bin/sh\nprintf '%s\\n' \"$@\" > received-args.txt\necho 'Downloaded private packages'\n").unwrap();
        registry.install_package(&emit, &node, &npm).await.unwrap();
        assert!(!marker.exists());
        assert_eq!(registry.detect().await.len(), 1);
        let args = std::fs::read_to_string(root.join("runtime/received-args.txt")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            install_args(&root)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert!(events.lock().iter().any(|event| matches!(
            event,
            BackendEvent::InstallStage {
                stage: InstallStage::Verifying,
                ..
            }
        )));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn shutdown_stops_fake_installer_and_keeps_incomplete_marker() {
        let root = root();
        fake_private_pi(&root);
        let node = root.join("fake-node");
        let npm = root.join("fake-npm");
        std::fs::write(&node, "#!/bin/sh\necho v22.19.0\n").unwrap();
        std::fs::write(&npm, "#!/bin/sh\necho started\nexec /bin/sleep 60\n").unwrap();
        for bin in [&node, &npm] {
            std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let started = parking_lot::Mutex::new(Some(tx));
        let emit: Emit = Arc::new(move |event| {
            if matches!(event, BackendEvent::InstallProgress { ref line, .. } if line == "started")
            {
                if let Some(tx) = started.lock().take() {
                    let _ = tx.send(());
                }
            }
        });
        let registry = Arc::new(HarnessRegistry::at_root(root.clone()));
        let running = registry.clone();
        let job = tokio::spawn(async move { running.install_package(&emit, &node, &npm).await });
        tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .unwrap()
            .unwrap();
        registry.shutdown().await;
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), job)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("closing"));
        assert!(root.join("runtime").join(INCOMPLETE_MARKER).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_private_bin_pointing_at_system_executable() {
        let root = root();
        fake_private_pi(&root);
        std::fs::remove_file(executable_in(&root)).unwrap();
        std::os::unix::fs::symlink("/bin/sh", executable_in(&root)).unwrap();
        assert!(managed_executable(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installer_is_local_fixed_and_safely_displayed() {
        let root = Path::new("/tmp/desk user's home/.pidesk");
        let args = install_args(root);
        assert!(!args.iter().any(|arg| arg == "-g" || arg == "--global"));
        assert!(args.contains(&"--global=false".to_string()));
        assert!(args.contains(&"--ignore-scripts".to_string()));
        assert!(args.contains(&root.join("runtime").to_string_lossy().into_owned()));
        let plan = install_plan(root);
        assert!(plan.command.contains("'\\''"));
        assert!(plan.login_command.starts_with("/usr/bin/env -i "));
        assert!(plan.login_command.contains("--session-dir"));
    }

    #[tokio::test]
    async fn installer_logs_are_bounded_redacted_and_drained() {
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let captured = events.clone();
        let emit: Emit = Arc::new(move |event| {
            captured.lock().push(event);
        });
        let input = format!(
            "Authorization: Bearer top-secret\n{}\nlast line\n",
            "é".repeat(MAX_LOG_LINE_BYTES)
        );
        stream_output(input.as_bytes(), emit, Arc::new(AtomicUsize::new(0)))
            .await
            .unwrap();
        let events = events.lock();
        assert_eq!(events.len(), 3);
        let lines: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                BackendEvent::InstallProgress { line, .. } => Some(line.as_str()),
                _ => None,
            })
            .collect();
        assert!(!lines[0].contains("top-secret"));
        assert!(lines[1].len() < MAX_LOG_LINE_BYTES + 30);
        assert!(lines[1].ends_with("[truncated]"));
        assert_eq!(lines[2], "last line");
    }
}
