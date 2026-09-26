use crate::dto::HarnessKind;
use crate::error::{AppError, AppResult};
use crate::{harness, util};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

const MAX_TERMINALS: usize = 4;
const OUTPUT_CHUNK: usize = 16 * 1024;
const MAX_INPUT_BYTES: usize = 64 * 1024;
/// Input bytes queued for a terminal whose program is not reading before
/// `write` reports busy. Bounded by bytes, not writes, so a burst of single
/// keystrokes can never overflow it.
const MAX_QUEUED_INPUT: usize = 4 * 1024 * 1024;
/// Output flow control: stop reading the PTY once this much output is
/// unacknowledged by the renderer, and resume only after it drains below
/// `RESUME_UNACKED`. A flood (`yes`, a huge `cat`) then applies backpressure
/// to the shell instead of queueing unbounded data in the IPC layer.
const PAUSE_UNACKED: usize = 1024 * 1024;
const RESUME_UNACKED: usize = 256 * 1024;

#[derive(Default)]
struct FlowState {
    unacked: usize,
    paused: bool,
    closed: bool,
}

#[derive(Default)]
struct Flow {
    state: std::sync::Mutex<FlowState>,
    resume: std::sync::Condvar,
}

impl Flow {
    /// Block the reader while paused; returns false once the terminal closes.
    fn wait_until_writable(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.paused && !state.closed {
            state = self.resume.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        !state.closed
    }

    fn sent(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.unacked = state.unacked.saturating_add(bytes);
        if state.unacked >= PAUSE_UNACKED {
            state.paused = true;
        }
    }

    fn ack(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.unacked = state.unacked.saturating_sub(bytes);
        if state.paused && state.unacked <= RESUME_UNACKED {
            state.paused = false;
            self.resume.notify_all();
        }
    }

    fn close(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.resume.notify_all();
    }
}

struct Session {
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    /// Writes go through one ordered queue drained by a dedicated thread, so
    /// input keeps its order and a blocked PTY never holds the manager lock.
    input: std::sync::mpsc::Sender<Vec<u8>>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    flow: Arc<Flow>,
}

#[derive(Default)]
struct Sessions {
    open: HashMap<String, Session>,
    closing: bool,
}

#[derive(Default)]
pub struct TerminalManager {
    sessions: Mutex<Sessions>,
}

fn size(cols: u16, rows: u16) -> AppResult<PtySize> {
    if !(2..=500).contains(&cols) || !(2..=300).contains(&rows) {
        return Err(AppError::new("Invalid terminal dimensions."));
    }
    Ok(PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    })
}

impl TerminalManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn open(
        self: &Arc<Self>,
        cols: u16,
        rows: u16,
        cwd: &Path,
        output: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
        registry: &harness::HarnessRegistry,
    ) -> AppResult<String> {
        // Validate before writing startup files or launching the shell.
        registry.executable_path(HarnessKind::Pi).await?;
        let cwd = cwd
            .canonicalize()
            .map_err(|_| AppError::new("Terminal working directory does not exist."))?;
        let root = util::pidesk_root();
        let (shell, args, zdotdir) = shell_command(&root)?;
        let mut command = CommandBuilder::new(shell);
        command.args(args);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        if let Some(zdotdir) = zdotdir {
            command.env("ZDOTDIR", zdotdir);
        }
        self.open_command(size(cols, rows)?, command, move |bytes| {
            output
                .send(tauri::ipc::InvokeResponseBody::Raw(bytes))
                .is_ok()
        })
    }

    fn open_command(
        self: &Arc<Self>,
        size: PtySize,
        command: CommandBuilder,
        send: impl Fn(Vec<u8>) -> bool + Send + 'static,
    ) -> AppResult<String> {
        let mut sessions = self.sessions.lock();
        if sessions.closing {
            return Err(AppError::new("The app is shutting down."));
        }
        if sessions.open.len() >= MAX_TERMINALS {
            return Err(AppError::new("At most four terminals can be open."));
        }
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|_| AppError::new("Could not open a terminal."))?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|_| AppError::new("Could not start the shell."))?;
        let reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(_) => {
                terminate(child.as_mut(), pair.master.as_ref());
                return Err(AppError::new("Could not read the terminal."));
            }
        };
        let mut writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(_) => {
                terminate(child.as_mut(), pair.master.as_ref());
                return Err(AppError::new("Could not write to the terminal."));
            }
        };
        let (input, pending) = std::sync::mpsc::channel::<Vec<u8>>();
        let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let flow = Arc::new(Flow::default());
        let id = uuid::Uuid::new_v4().to_string();
        sessions.open.insert(
            id.clone(),
            Session {
                child,
                master: pair.master,
                input,
                queued: queued.clone(),
                flow: flow.clone(),
            },
        );
        drop(sessions);
        // Exits when the session (and its sender) is dropped, or when a write
        // fails because close() killed the shell and the PTY went away.
        std::thread::spawn(move || {
            for chunk in pending {
                let ok = writer
                    .write_all(&chunk)
                    .and_then(|_| writer.flush())
                    .is_ok();
                queued.fetch_sub(chunk.len(), std::sync::atomic::Ordering::SeqCst);
                if !ok {
                    break;
                }
            }
        });
        let manager = self.clone();
        let read_id = id.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; OUTPUT_CHUNK];
            while flow.wait_until_writable() {
                match reader.read(&mut buf) {
                    Ok(len) if len > 0 => {
                        flow.sent(len);
                        if !send(buf[..len].to_vec()) {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            manager.close(&read_id);
        });
        Ok(id)
    }

    /// Queue input without blocking. Callers must submit writes for one
    /// terminal in order (the frontend chains them); the queue preserves it.
    pub fn write(&self, id: &str, data: &str) -> AppResult<()> {
        if data.len() > MAX_INPUT_BYTES {
            return Err(AppError::new("Terminal input is too large."));
        }
        let (input, queued) = self
            .sessions
            .lock()
            .open
            .get(id)
            .map(|session| (session.input.clone(), session.queued.clone()))
            .ok_or_else(|| AppError::new("Terminal is closed."))?;
        let bytes = data.len();
        let reserved = queued.fetch_add(bytes, std::sync::atomic::Ordering::SeqCst);
        if reserved + bytes > MAX_QUEUED_INPUT {
            queued.fetch_sub(bytes, std::sync::atomic::Ordering::SeqCst);
            return Err(AppError::new(
                "The terminal is busy. Try again in a moment.",
            ));
        }
        input.send(data.as_bytes().to_vec()).map_err(|_| {
            queued.fetch_sub(bytes, std::sync::atomic::Ordering::SeqCst);
            AppError::new("Terminal is closed.")
        })
    }

    /// The renderer finished drawing `bytes` of output.
    pub fn ack(&self, id: &str, bytes: usize) {
        let flow = self
            .sessions
            .lock()
            .open
            .get(id)
            .map(|session| session.flow.clone());
        if let Some(flow) = flow {
            flow.ack(bytes);
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let dimensions = size(cols, rows)?;
        let sessions = self.sessions.lock();
        let session = sessions
            .open
            .get(id)
            .ok_or_else(|| AppError::new("Terminal is closed."))?;
        session
            .master
            .resize(dimensions)
            .map_err(|_| AppError::new("Could not resize the terminal."))
    }

    pub fn close(&self, id: &str) {
        let removed = self.sessions.lock().open.remove(id);
        if let Some(mut session) = removed {
            // Wake a reader paused on flow control so it can exit.
            session.flow.close();
            terminate(session.child.as_mut(), session.master.as_ref());
        }
    }

    pub fn shutdown(&self) {
        let ids = {
            let mut sessions = self.sessions.lock();
            sessions.closing = true;
            sessions.open.keys().cloned().collect::<Vec<_>>()
        };
        for id in ids {
            self.close(&id);
        }
    }
}

#[cfg(target_os = "macos")]
fn kill_session_groups(session_id: i32) {
    // zsh/bash job control gives background jobs their own process groups.
    // Keep the shell leader unreaped during enumeration: its PID (also the
    // session ID) cannot be reused by an unrelated process in this window.
    const PROC_ALL_PIDS: u32 = 1;
    const MAX_PIDS: usize = 65_536;
    let mut pids = vec![0 as libc::pid_t; MAX_PIDS];
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_ALL_PIDS,
            0,
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int,
        )
    };
    if bytes <= 0 {
        return;
    }
    let count = (bytes as usize / std::mem::size_of::<libc::pid_t>()).min(pids.len());
    let own_group = unsafe { libc::getpgrp() };
    let mut groups = std::collections::HashSet::new();
    for &pid in &pids[..count] {
        if pid <= 0 || unsafe { libc::getsid(pid) } != session_id {
            continue;
        }
        let group = unsafe { libc::getpgid(pid) };
        if group > 0 && group != own_group {
            groups.insert(group);
        }
    }
    for group in groups {
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

fn terminate(child: &mut dyn Child, master: &dyn MasterPty) {
    // portable-pty calls setsid() at spawn. Interactive shells can move Pi
    // into a separate foreground job group: kill that group first, but only
    // if it still belongs to this shell's session. Never target our own group.
    #[cfg(unix)]
    if let Some(pid) = child.process_id().filter(|pid| *pid > 0) {
        #[cfg(target_os = "macos")]
        kill_session_groups(pid as i32);
        if let Some(fd) = master.as_raw_fd() {
            let foreground = unsafe { libc::tcgetpgrp(fd) };
            if foreground > 0
                && foreground != pid as i32
                && unsafe { libc::getsid(foreground) } == pid as i32
            {
                unsafe {
                    libc::kill(-foreground, libc::SIGKILL);
                }
            }
        }
    }
    // Only signal the original leader's group while it is still unreaped;
    // after reaping, its PID may be reused by an unrelated process.
    if !matches!(child.try_wait(), Ok(Some(_))) {
        #[cfg(unix)]
        if let Some(pid) = child.process_id() {
            if pid > 0 && unsafe { libc::getpgid(pid as i32) } == pid as i32 {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(pid) = child.process_id() {
            kill_session_groups(pid as i32);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn write_wrapper(path: &Path, content: &str) -> AppResult<()> {
    if path.exists() {
        util::check_owned_path(path, false)?;
    }
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let result = (|| -> AppResult<()> {
        options.open(&temp)?.write_all(content.as_bytes())?;
        std::fs::rename(&temp, path)?;
        util::check_owned_path(path, false)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

fn wrapper_files(root: &Path) -> AppResult<()> {
    wrapper_files_in(root, &util::home_dir())
}

fn wrapper_files_in(root: &Path, home: &Path) -> AppResult<()> {
    let shell_dir = root.join("shell");
    let zsh_dir = shell_dir.join("zsh");
    util::ensure_private_directory(root, &zsh_dir)?;
    let quote = |path: &Path| harness::shell_quote(&path.to_string_lossy());
    let zsh_path = quote(&zsh_dir);
    let invocation = harness::private_pi_invocation(root);
    let zsh_pi = format!("unalias pi 2>/dev/null || :\nif functions pi >/dev/null 2>&1; then unfunction pi || exit 1; fi\npi() {{ {invocation} \"$@\"; }}\n");
    let bash_pi = format!("unalias pi 2>/dev/null || :\nunset -f pi 2>/dev/null || exit 1\npi() {{ {invocation} \"$@\"; }}\n");
    let home_path = quote(home);
    for filename in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        // zsh reads .zshenv from $HOME; that file may point ZDOTDIR elsewhere
        // (e.g. ~/.config/zsh), and the remaining startup files live there.
        let mut script = if filename == ".zshenv" {
            let real = quote(&home.join(filename));
            format!(
                "export ZDOTDIR={home_path}\nif [ -f {real} ]; then . {real}; fi\nexport PIDESK_USER_ZDOTDIR=\"${{ZDOTDIR:-{home_path}}}\"\nbuiltin setopt rcs\nexport ZDOTDIR={zsh_path} || exit 1\n{zsh_pi}"
            )
        } else {
            format!(
                "export ZDOTDIR=\"${{PIDESK_USER_ZDOTDIR:-{home_path}}}\"\nif [ -f \"$ZDOTDIR/{filename}\" ]; then . \"$ZDOTDIR/{filename}\"; fi\nbuiltin setopt rcs\nexport ZDOTDIR={zsh_path} || exit 1\n{zsh_pi}"
            )
        };
        if filename == ".zlogin" {
            script.push_str("printf 'πDesk shell — pi runs πDesk private Pi (~/.pidesk)\\n'\n");
            // Startup is done: give the session the user's real ZDOTDIR back
            // so runtime tools never see πDesk's wrapper directory.
            script.push_str(&format!(
                "if [ \"${{PIDESK_USER_ZDOTDIR:-{home_path}}}\" = {home_path} ]; then unset ZDOTDIR; else export ZDOTDIR=\"$PIDESK_USER_ZDOTDIR\"; fi\nunset PIDESK_USER_ZDOTDIR\n"
            ));
        }
        write_wrapper(&zsh_dir.join(filename), &script)?;
    }
    let profile = [".bash_profile", ".bash_login", ".profile"].map(|name| quote(&home.join(name)));
    let bashrc = quote(&home.join(".bashrc"));
    let bash = format!("if [ -f {} ]; then . {}; elif [ -f {} ]; then . {}; elif [ -f {} ]; then . {}; fi\nif [ -f {bashrc} ]; then . {bashrc}; fi\n{bash_pi}printf 'πDesk shell — pi runs πDesk private Pi (~/.pidesk)\\n'\n", profile[0], profile[0], profile[1], profile[1], profile[2], profile[2]);
    write_wrapper(&shell_dir.join("bashrc"), &bash)
}

fn shell_command(root: &Path) -> AppResult<(String, Vec<String>, Option<String>)> {
    wrapper_files(root)?;
    let selected = std::env::var("SHELL").unwrap_or_default();
    let shell = Path::new(&selected);
    if shell.file_name().is_some_and(|name| name == "bash")
        && shell.is_absolute()
        && shell.is_file()
    {
        return Ok((
            selected,
            vec![
                "--rcfile".into(),
                root.join("shell/bashrc").to_string_lossy().into_owned(),
                "-i".into(),
            ],
            None,
        ));
    }
    let shell = if shell.file_name().is_some_and(|name| name == "zsh")
        && shell.is_absolute()
        && shell.is_file()
    {
        selected
    } else {
        "/bin/zsh".into()
    };
    Ok((
        shell,
        vec!["-i".into(), "-l".into()],
        Some(root.join("shell/zsh").to_string_lossy().into_owned()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn pty_echo_and_terminal_limit() {
        let manager = TerminalManager::new();
        let (sender, receiver) = mpsc::channel();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "read line; echo tagged:$line"]);
        let id = manager
            .open_command(PtySize::default(), command, move |bytes| {
                sender.send(bytes).is_ok()
            })
            .unwrap();
        manager.write(&id, "hello\n").unwrap();
        let mut output = String::new();
        while !output.contains("tagged:hello") {
            output.push_str(&String::from_utf8_lossy(
                &receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            ));
        }
        manager.close(&id);
        let mut ids = Vec::new();
        for _ in 0..MAX_TERMINALS {
            let mut command = CommandBuilder::new("/bin/sh");
            command.args(["-c", "read line"]);
            ids.push(
                manager
                    .open_command(PtySize::default(), command, |_| true)
                    .unwrap(),
            );
        }
        assert!(manager
            .open_command(PtySize::default(), CommandBuilder::new("/bin/sh"), |_| true)
            .is_err());
        manager.shutdown();
        assert!(manager.sessions.lock().open.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn close_and_shutdown_kill_background_job_groups() {
        let manager = TerminalManager::new();
        let launch = |manager: &Arc<TerminalManager>| {
            let (sender, receiver) = mpsc::channel();
            let mut command = CommandBuilder::new("/bin/zsh");
            command.args([
                "-f",
                "-i",
                "-c",
                "nohup /bin/sleep 30 >/dev/null 2>&1 & echo BG:$!; read line",
            ]);
            let id = manager
                .open_command(PtySize::default(), command, move |bytes| {
                    sender.send(bytes).is_ok()
                })
                .unwrap();
            let mut text = String::new();
            let pid = loop {
                text.push_str(&String::from_utf8_lossy(
                    &receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
                ));
                if let Some(pid) = text
                    .split("BG:")
                    .skip(1)
                    .filter_map(|part| {
                        part.chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect::<String>()
                            .parse::<i32>()
                            .ok()
                    })
                    .next()
                {
                    break pid;
                }
            };
            assert_eq!(
                unsafe { libc::getsid(pid) },
                manager
                    .sessions
                    .lock()
                    .open
                    .get(&id)
                    .unwrap()
                    .child
                    .process_id()
                    .unwrap() as i32
            );
            (id, pid)
        };
        let (id, pid) = launch(&manager);
        manager.close(&id);
        let (id, shutdown_pid) = launch(&manager);
        manager.shutdown();
        assert!(!manager.sessions.lock().open.contains_key(&id));
        for child in [pid, shutdown_pid] {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while unsafe { libc::kill(child, 0) } == 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                unsafe { libc::kill(child, 0) },
                -1,
                "background job {child} survived terminal close"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_shells_override_pi_aliases_and_zshenv_rcs() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("pidesk-shell-live-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let executable = root.join("runtime/node_modules/.bin/pi");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "#!/bin/sh\nprintf 'PRIVATE:%s:%s\\n' \"${OPENAI_API_KEY-unset}\" \"$PI_CODING_AGENT_DIR\"\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            home.join(".zshenv"),
            "alias pi='echo SYSTEM_PI'\nunsetopt RCS\n",
        )
        .unwrap();
        std::fs::write(home.join(".zshrc"), "alias pi='echo SYSTEM_PI'\n").unwrap();
        std::fs::write(home.join(".bashrc"), "alias pi='echo SYSTEM_PI'\n").unwrap();
        wrapper_files_in(&root, &home).unwrap();
        for (shell, args) in [
            ("/bin/zsh", vec!["-i", "-l", "-c", "type pi; pi --version"]),
            (
                "/bin/bash",
                vec![
                    "--rcfile",
                    root.join("shell/bashrc").to_str().unwrap(),
                    "-i",
                    "-c",
                    "type pi; pi --version",
                ],
            ),
        ] {
            let output = std::process::Command::new(shell)
                .args(args)
                .env_clear()
                .env("HOME", &home)
                .env("ZDOTDIR", root.join("shell/zsh"))
                .env("PATH", "/usr/bin:/bin")
                .env("TERM", "xterm-256color")
                .env("OPENAI_API_KEY", "must-not-reach-pi")
                .output()
                .unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.status.success(), "{shell}: {text}");
            assert!(text.contains("PRIVATE:unset:"), "{shell}: {text}");
            assert!(
                text.contains(&root.join("agent").to_string_lossy().to_string()),
                "{shell}: {text}"
            );
            assert!(!text.contains("SYSTEM_PI"), "{shell}: {text}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    fn collect_until(receiver: &mpsc::Receiver<Vec<u8>>, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let mut output = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done(&output) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!left.is_zero(), "timed out; got {} bytes", output.len());
            if let Ok(chunk) = receiver.recv_timeout(left) {
                output.extend(chunk);
            }
        }
        output
    }

    #[test]
    fn many_small_writes_arrive_in_order() {
        let manager = TerminalManager::new();
        let (sender, receiver) = mpsc::channel();
        let mut command = CommandBuilder::new("/bin/sh");
        // No echo: the output is exactly what `cat` read, in order.
        // Raw mode: no echo and no line buffering, so `cat` sees every byte
        // immediately and the output is exactly the input, in order.
        command.args(["-c", "stty raw -echo; printf READY; exec cat"]);
        let id = manager
            .open_command(PtySize::default(), command, move |bytes| {
                sender.send(bytes).is_ok()
            })
            .unwrap();
        collect_until(&receiver, |out| out.ends_with(b"READY"));
        let expected: String = (0..1000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        for ch in expected.chars() {
            manager.write(&id, &ch.to_string()).unwrap();
        }
        manager.ack(&id, usize::MAX);
        let output = collect_until(&receiver, |out| out.len() >= expected.len());
        assert_eq!(String::from_utf8_lossy(&output[..expected.len()]), expected);
        manager.close(&id);
    }

    #[test]
    fn writes_never_block_on_a_stalled_program_and_close_is_prompt() {
        let manager = TerminalManager::new();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let id = manager
            .open_command(PtySize::default(), command, |_| true)
            .unwrap();
        let chunk = "x".repeat(MAX_INPUT_BYTES);
        let started = std::time::Instant::now();
        // Far more than the PTY buffer plus the queue: must report busy, not hang.
        let mut busy = false;
        for _ in 0..(MAX_QUEUED_INPUT / MAX_INPUT_BYTES) * 4 {
            if manager.write(&id, &chunk).is_err() {
                busy = true;
                break;
            }
        }
        assert!(
            busy,
            "a stalled program must eventually make the queue report busy"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        // Other operations on the manager are not stuck behind the stalled write.
        manager.resize(&id, 100, 30).unwrap();
        let closing = std::time::Instant::now();
        manager.close(&id);
        assert!(closing.elapsed() < Duration::from_secs(3));
        assert!(manager.sessions.lock().open.is_empty());
    }

    #[test]
    fn output_pauses_without_acks_and_resumes_after_them() {
        let manager = TerminalManager::new();
        let (sender, receiver) = mpsc::channel();
        let mut command = CommandBuilder::new("/usr/bin/yes");
        command.arg("flow-control");
        let id = manager
            .open_command(PtySize::default(), command, move |bytes| {
                sender.send(bytes).is_ok()
            })
            .unwrap();
        let first = collect_until(&receiver, |out| out.len() >= PAUSE_UNACKED);
        std::thread::sleep(Duration::from_millis(300));
        let mut total = first.len();
        while let Ok(chunk) = receiver.try_recv() {
            total += chunk.len();
        }
        assert!(
            total < PAUSE_UNACKED + 2 * OUTPUT_CHUNK,
            "reader kept going without acks: {total} bytes"
        );
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            receiver.try_recv().is_err(),
            "output continued while paused"
        );

        manager.ack(&id, total);
        let resumed = collect_until(&receiver, |out| out.len() >= OUTPUT_CHUNK);
        assert!(!resumed.is_empty());

        // Without further acks it pauses again PAUSE_UNACKED bytes after the
        // resume (`resumed` already counts toward that), then still closes
        // promptly.
        let rest = PAUSE_UNACKED - resumed.len();
        let _ = collect_until(&receiver, |out| out.len() >= rest);
        let closing = std::time::Instant::now();
        manager.close(&id);
        assert!(closing.elapsed() < Duration::from_secs(3));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_wrapper_honours_a_custom_zdotdir() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("pidesk-zdotdir-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let custom = home.join(".config/zsh");
        let executable = root.join("runtime/node_modules/.bin/pi");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "#!/bin/sh\necho PRIVATE_PI\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            home.join(".zshenv"),
            format!("export ZDOTDIR={}\n", custom.display()),
        )
        .unwrap();
        std::fs::write(
            custom.join(".zshrc"),
            "export CUSTOM_RC=loaded\nalias pi='echo SYSTEM_PI'\n",
        )
        .unwrap();
        wrapper_files_in(&root, &home).unwrap();
        let output = std::process::Command::new("/bin/zsh")
            .args(["-i", "-l", "-c", "echo rc:$CUSTOM_RC zd:$ZDOTDIR; pi"])
            .env_clear()
            .env("HOME", &home)
            .env("ZDOTDIR", root.join("shell/zsh"))
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "xterm-256color")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("rc:loaded"), "{text}");
        assert!(text.contains(&format!("zd:{}", custom.display())), "{text}");
        assert!(
            text.contains("PRIVATE_PI") && !text.contains("SYSTEM_PI"),
            "{text}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shell_wrappers_source_originals_and_isolate_pi() {
        let root = std::env::temp_dir().join(format!("pidesk-shell-{}", uuid::Uuid::new_v4()));
        wrapper_files(&root).unwrap();
        for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            let script = std::fs::read_to_string(root.join("shell/zsh").join(name)).unwrap();
            assert!(script.contains(&format!("/{name}")) && script.contains("then . "));
        }
        for script in [root.join("shell/zsh/.zlogin"), root.join("shell/bashrc")] {
            let script = std::fs::read_to_string(script).unwrap();
            assert!(script.contains("pi() { /usr/bin/env -i"));
            assert!(script.contains("PI_CODING_AGENT_DIR="));
            assert!(script.contains(
                &root
                    .join("runtime/node_modules/.bin/pi")
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(script.contains("--no-approve --session-dir"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
