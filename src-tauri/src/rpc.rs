use crate::error::{AppError, AppResult};
use crate::util;
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, watch, Mutex};

pub const MAX_FRAME_BYTES: usize = 1_048_576; // 1 MiB per OMP wire frame
pub const MAX_PI_FRAME_BYTES: usize = 8 * 1024 * 1024; // bounded Pi monolithic history frame
const MAX_REASSEMBLED_BYTES: usize = 67_108_864; // 64 MiB reassembled
const CHUNK_PAYLOAD_BYTES: usize = 262_144; // 256 KiB per chunk payload
const STDERR_TAIL_BYTES: usize = 16 * 1024;
const DEFAULT_CMD_TIMEOUT_SECS: u64 = 60;
const STDIN_IO_TIMEOUT_SECS: u64 = 10;
const MAX_PENDING_REQUESTS: usize = 256;
const LEADER_DRAIN_GRACE_MS: u64 = 250;
/// Up to 16 live processes each poll their leader; 25ms polling cost ~640
/// wakeups/s for an exit signal that tolerates tenths of a second of latency.
const LEADER_POLL_INTERVAL_MS: u64 = 100;

/// Read one `\n`-terminated line capped at `max` bytes (excluding the newline).
/// Returns raw bytes so callers choose strict (stdout frames) or lossy (stderr
/// tail) decoding. An oversized line fails with `ErrorKind::InvalidData`
/// WITHOUT consuming its terminator, so the caller can fail closed (stdout)
/// or discard the remainder and keep draining (stderr).
async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
            if buf.len() + pos > max {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "RPC line exceeded size limit",
                ));
            }
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            break;
        }
        if buf.len() + available.len() > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "RPC line exceeded size limit",
            ));
        }
        let len = available.len();
        buf.extend_from_slice(available);
        reader.consume(len);
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(Some(buf))
}

/// Consume the remainder of one line (through `\n` or EOF) in bounded
/// `fill_buf` chunks without buffering it. Used to skip an oversized stderr
/// line while keeping the pipe open.
async fn discard_line_rest<R: AsyncBufRead + Unpin>(reader: &mut R) -> std::io::Result<()> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(());
        }
        if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
            reader.consume(pos + 1);
            return Ok(());
        }
        let len = available.len();
        reader.consume(len);
    }
}

/// Append one stderr line to the capped tail, keeping the last
/// `STDERR_TAIL_BYTES` bytes on a char boundary.
async fn append_stderr_tail(inner: &Arc<RpcInner>, line: &str) {
    let mut tail = inner.stderr_tail.lock().await;
    tail.push_str(line);
    tail.push('\n');
    if tail.len() > STDERR_TAIL_BYTES {
        let mut cut = tail.len() - STDERR_TAIL_BYTES;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
}

/// ASCII-whitespace check for raw stdout lines (already `\n`-stripped).
fn trim_ascii_whitespace(line: &[u8]) -> &[u8] {
    let is_space = |byte: &u8| matches!(*byte, b' ' | b'\t' | b'\r');
    let start = line
        .iter()
        .position(|byte| !is_space(byte))
        .unwrap_or(line.len());
    let end = line
        .iter()
        .rposition(|byte| !is_space(byte))
        .map(|pos| pos + 1)
        .unwrap_or(0);
    &line[start.min(end)..end]
}

/// Report the leader exit the poll monitor recorded. Called exactly once by
/// the stdout reader after draining buffered frames (or on the bounded
/// deadline), preserving the existing `mark_exited` "exactly once" guarantee.
async fn finish_leader_exit(inner: &Arc<RpcInner>, exit: Option<LeaderExit>) {
    let Some(exit) = exit else {
        let tail = inner.stderr_tail.lock().await.clone();
        mark_exited(inner, inner.on_exit.clone(), None, tail).await;
        return;
    };
    let tail = inner.stderr_tail.lock().await.clone();
    if let Some(error) = exit.wait_error {
        mark_exited(
            inner,
            inner.on_exit.clone(),
            None,
            format!("{tail}\nCould not wait for harness process: {error}"),
        )
        .await;
        return;
    }
    mark_exited(inner, inner.on_exit.clone(), exit.code, tail).await;
}

/// One pending RPC request awaiting its `response` frame.
struct Pending {
    tx: oneshot::Sender<Result<Value, String>>,
}

/// Reassembly state for one `rpc_chunk` stream (OMP protocol v2).
struct ChunkState {
    chunk_id: String,
    count: usize,
    byte_length: usize,
    next_index: usize,
    received: usize,
    chunks: Vec<Vec<u8>>,
}

/// Shared handle to a running harness RPC process.
/// Owns the child; a reader task forwards events and resolves responses.
pub struct RpcClient {
    inner: Arc<RpcInner>,
}

struct RpcInner {
    stdin: Mutex<ChildStdin>,
    pending: Mutex<HashMap<u64, Pending>>,
    next_id: AtomicU64,
    child: Mutex<Child>,
    exited: AtomicBool,
    expected_exit: AtomicBool,
    stderr_tail: Mutex<String>,
    /// Process-group id captured before the leader can be reaped. Kept so a
    /// descendant cannot inherit stdout and keep a dead leader alive.
    process_group_id: Option<i32>,
    /// Watch sender for the leader status observed by the poll monitor. The
    /// monitor records the reaped status here; the stdout reader drains
    /// buffered frames, then reports it. Never used for group signalling.
    leader_exit_tx: watch::Sender<Option<LeaderExit>>,
    /// Exit callback, stored so stdin-failure paths can fail the client.
    on_exit: Arc<Box<dyn Fn(Option<i32>, String, bool) + Send + Sync>>,
}

/// Leader status observed by the poll monitor.
#[derive(Clone)]
struct LeaderExit {
    code: Option<i32>,
    wait_error: Option<String>,
}

/// Callbacks the process supervisor supplies.
pub struct RpcHandlers {
    /// Every non-response frame, already normalized (chunked frames are
    /// reassembled; oversized/redundant fields stripped by the caller).
    pub on_event: Box<dyn Fn(Value) + Send + Sync>,
    /// Fired once when the process exits or the stdout stream ends.
    /// Args: exit code, stderr tail, whether the exit was expected.
    pub on_exit: Box<dyn Fn(Option<i32>, String, bool) + Send + Sync>,
    /// OMP only: resolves when the `ready` frame arrives.
    pub on_ready: Option<Box<dyn Fn(Value) + Send + Sync>>,
}

impl RpcClient {
    /// Spawn the reader/stderr/monitor tasks around an already-spawned child.
    pub fn attach(
        child: Child,
        stdout: ChildStdout,
        stderr: ChildStderr,
        handlers: RpcHandlers,
    ) -> Self {
        Self::attach_with_frame_limit(child, stdout, stderr, handlers, MAX_FRAME_BYTES)
    }

    /// Attach with a harness-specific unchunked line ceiling. Pi returns one
    /// monolithic history frame, while OMP uses chunked v2 frames.
    pub fn attach_with_frame_limit(
        mut child: Child,
        stdout: ChildStdout,
        stderr: ChildStderr,
        handlers: RpcHandlers,
        max_frame_bytes: usize,
    ) -> Self {
        let stdin = child.stdin.take().expect("child stdin piped");
        #[cfg(unix)]
        let process_group_id = child.id().map(|pid| pid as i32);
        #[cfg(not(unix))]
        let process_group_id: Option<i32> = None;
        let (leader_exit_tx, _) = watch::channel(None);
        let on_exit = Arc::new(handlers.on_exit);
        let inner = Arc::new(RpcInner {
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            exited: AtomicBool::new(false),
            expected_exit: AtomicBool::new(false),
            stderr_tail: Mutex::new(String::new()),
            process_group_id,
            leader_exit_tx,
            on_exit: on_exit.clone(),
        });
        // stderr tail collector
        {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut stderr = BufReader::new(stderr);
                loop {
                    match read_bounded_line(&mut stderr, max_frame_bytes).await {
                        Ok(Some(line)) => {
                            append_stderr_tail(&inner, &String::from_utf8_lossy(&line)).await;
                        }
                        Ok(None) => return,
                        Err(error) => {
                            if error.kind() != std::io::ErrorKind::InvalidData {
                                return;
                            }
                            // Oversized line: discard the remainder in bounded
                            // chunks and keep draining so the pipe stays open.
                            let _ = discard_line_rest(&mut stderr).await;
                            append_stderr_tail(&inner, "[truncated oversized stderr line]").await;
                        }
                    }
                }
            });
        }
        // Poll the leader independently of stdout. A descendant may inherit the
        // pipe forever, but it must not make a dead leader reusable. The
        // monitor only records the reaped status; the stdout reader below owns
        // the final drain and the single `mark_exited` call, so buffered final
        // frames are still delivered before exit is reported.
        {
            let inner = inner.clone();
            tokio::spawn(async move {
                loop {
                    let status = {
                        let mut child = inner.child.lock().await;
                        child.try_wait()
                    };
                    match status {
                        Ok(Some(status)) => {
                            let _ = inner.leader_exit_tx.send(Some(LeaderExit {
                                code: status.code(),
                                wait_error: None,
                            }));
                            return;
                        }
                        Ok(None) => {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                LEADER_POLL_INTERVAL_MS,
                            ))
                            .await
                        }
                        Err(error) => {
                            let _ = inner.leader_exit_tx.send(Some(LeaderExit {
                                code: None,
                                wait_error: Some(error.to_string()),
                            }));
                            return;
                        }
                    }
                }
            });
        }
        // stdout reader. Owns the single `mark_exited` call: once the leader
        // is reaped it keeps draining buffered frames until EOF or a bounded
        // deadline, then reports the exit. The reader never waits on the pipe
        // indefinitely: a descendant holding stdout open cannot keep a dead
        // leader alive past the deadline.
        {
            let inner = inner.clone();
            let mut leader_rx = inner.leader_exit_tx.subscribe();
            tokio::spawn(async move {
                let mut chunk_state: Option<ChunkState> = None;
                let mut stdout = BufReader::new(stdout);
                let mut leader_exit: Option<LeaderExit> = None;
                let mut drain_deadline: Option<tokio::time::Instant> = None;
                loop {
                    if leader_exit.is_none() {
                        // `borrow_and_update` marks this snapshot seen so a
                        // later `changed()` only fires on a new leader status.
                        if let Some(exit) = leader_rx.borrow_and_update().clone() {
                            leader_exit = Some(exit);
                            drain_deadline = Some(
                                tokio::time::Instant::now()
                                    + std::time::Duration::from_millis(LEADER_DRAIN_GRACE_MS),
                            );
                        }
                    }
                    let read = read_bounded_line(&mut stdout, max_frame_bytes);
                    let line = if let Some(deadline) = drain_deadline {
                        match tokio::time::timeout_at(deadline, read).await {
                            Ok(line) => line,
                            Err(_) => {
                                finish_leader_exit(&inner, leader_exit.clone()).await;
                                let mut child = inner.child.lock().await;
                                terminate_child(&mut child, inner.process_group_id).await;
                                return;
                            }
                        }
                    } else {
                        read.await
                    };
                    match line {
                        Ok(Some(line)) => {
                            if trim_ascii_whitespace(&line).is_empty() {
                                continue;
                            }
                            let Ok(frame) = serde_json::from_slice::<Value>(&line) else {
                                continue; // non-JSON noise on stdout
                            };
                            let frame = match push_chunk(&mut chunk_state, frame) {
                                Ok(Some(frame)) => frame,
                                Ok(None) => continue,
                                Err(error) => {
                                    let tail = inner.stderr_tail.lock().await.clone();
                                    let msg = util::redact_secrets(&if tail.is_empty() {
                                        format!("RPC protocol error: {error}")
                                    } else {
                                        format!("RPC protocol error: {error}. Stderr: {tail}")
                                    });
                                    mark_exited(&inner, on_exit.clone(), None, msg).await;
                                    let mut child = inner.child.lock().await;
                                    terminate_child(&mut child, inner.process_group_id).await;
                                    return;
                                }
                            };
                            let ftype = frame.get("type").and_then(Value::as_str).unwrap_or("");
                            match ftype {
                                "response" => {
                                    if let Some(late_error) = resolve_response(&inner, frame).await
                                    {
                                        (handlers.on_event)(late_error);
                                    }
                                }
                                "ready" => {
                                    if let Some(cb) = &handlers.on_ready {
                                        cb(frame);
                                    } else {
                                        (handlers.on_event)(frame);
                                    }
                                }
                                _ => (handlers.on_event)(frame),
                            }
                        }
                        Ok(None) => {
                            if let Some(exit) = leader_exit.clone() {
                                finish_leader_exit(&inner, Some(exit)).await;
                            } else {
                                let tail = inner.stderr_tail.lock().await.clone();
                                mark_exited(&inner, on_exit.clone(), None, tail).await;
                            }
                            let mut child = inner.child.lock().await;
                            terminate_child(&mut child, inner.process_group_id).await;
                            return;
                        }
                        Err(error) => {
                            let msg = util::redact_secrets(&format!("RPC protocol error: {error}"));
                            mark_exited(&inner, on_exit.clone(), None, msg).await;
                            let mut child = inner.child.lock().await;
                            terminate_child(&mut child, inner.process_group_id).await;
                            return;
                        }
                    }
                }
            });
        }
        Self { inner }
    }

    /// Mark the next exit as expected (stop/suspend/restart paths).
    pub fn expect_exit(&self) {
        self.inner.expected_exit.store(true, Ordering::SeqCst);
    }

    /// Whether the exit was initiated by us (stop/suspend/restart).
    pub fn expected_exit(&self) -> bool {
        self.inner.expected_exit.load(Ordering::SeqCst)
    }

    pub fn is_exited(&self) -> bool {
        self.inner.exited.load(Ordering::SeqCst)
    }

    pub async fn stderr_tail(&self) -> String {
        util::redact_secrets(&self.inner.stderr_tail.lock().await)
    }

    /// Send a command frame `{id, type: command, ...args}` and await its response.
    pub async fn call(&self, command: &str, args: Map<String, Value>) -> AppResult<Value> {
        self.call_with_timeout(command, args, DEFAULT_CMD_TIMEOUT_SECS)
            .await
    }

    /// A timed-out or failed stdin write can leave a partial JSONL line in the
    /// pipe, permanently desyncing every later frame. Fail the client (same
    /// `mark_exited` + group termination as the reader error paths) so later
    /// calls fail with the existing "exited" error and the backend restarts.
    async fn fail_on_broken_stdin(&self) {
        let tail = self.inner.stderr_tail.lock().await.clone();
        let msg = util::redact_secrets(&if tail.is_empty() {
            "Stdin write to the harness process failed; the JSONL stream may be desynced."
                .to_string()
        } else {
            format!("Stdin write to the harness process failed; the JSONL stream may be desynced. Stderr: {tail}")
        });
        mark_exited(&self.inner, self.inner.on_exit.clone(), None, msg).await;
        let mut child = self.inner.child.lock().await;
        terminate_child(&mut child, self.inner.process_group_id).await;
    }

    pub async fn call_with_timeout(
        &self,
        command: &str,
        mut args: Map<String, Value>,
        timeout_secs: u64,
    ) -> AppResult<Value> {
        if self.is_exited() {
            return Err(AppError::new("The harness process has exited."));
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.inner.pending.lock().await;
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(AppError::new(
                    "Too many harness requests are still pending.",
                ));
            }
            pending.insert(id, Pending { tx });
            if self.inner.exited.load(Ordering::SeqCst) {
                pending.remove(&id);
                return Err(AppError::new("The harness process has exited."));
            }
        }
        args.insert("id".to_string(), json!(id.to_string()));
        args.insert("type".to_string(), json!(command));
        let mut frame = serde_json::to_string(&Value::Object(args))
            .map_err(|e| AppError::new(format!("Could not encode request: {e}")))?;
        frame.push('\n');
        let write = async {
            let mut stdin = self.inner.stdin.lock().await;
            stdin.write_all(frame.as_bytes()).await?;
            stdin.flush().await
        };
        let write_result =
            tokio::time::timeout(std::time::Duration::from_secs(STDIN_IO_TIMEOUT_SECS), write)
                .await;
        match write_result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.inner.pending.lock().await.remove(&id);
                self.fail_on_broken_stdin().await;
                return Err(AppError::new(format!(
                    "Could not write to the harness process: {error}"
                )));
            }
            Err(error) => {
                self.inner.pending.lock().await.remove(&id);
                self.fail_on_broken_stdin().await;
                return Err(AppError::new(format!(
                    "Could not write to the harness process: {error}"
                )));
            }
        }
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), rx).await {
            Ok(Ok(Ok(data))) => Ok(data),
            Ok(Ok(Err(err))) => Err(AppError::new(err)),
            Ok(Err(_)) => Err(AppError::new("The harness process ended unexpectedly.")),
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                Err(AppError::new(format!(
                    "The harness did not answer `{command}` in time."
                )))
            }
        }
    }

    /// Fire-and-forget frame write (extension_ui_response etc.).
    pub async fn send(&self, frame: Value) -> AppResult<()> {
        if self.is_exited() {
            return Err(AppError::new("The harness process has exited."));
        }
        let mut s = serde_json::to_string(&frame)
            .map_err(|e| AppError::new(format!("Could not encode frame: {e}")))?;
        s.push('\n');
        let write = async {
            let mut stdin = self.inner.stdin.lock().await;
            stdin.write_all(s.as_bytes()).await?;
            stdin.flush().await
        };
        let write_result =
            tokio::time::timeout(std::time::Duration::from_secs(STDIN_IO_TIMEOUT_SECS), write)
                .await;
        match write_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.fail_on_broken_stdin().await;
                Err(AppError::new(format!(
                    "Could not write to the harness process: {error}"
                )))
            }
            Err(error) => {
                self.fail_on_broken_stdin().await;
                Err(AppError::new(format!(
                    "Could not write to the harness process: {error}"
                )))
            }
        }
    }

    /// Graceful stop: SIGTERM, then SIGKILL after a grace period.
    pub async fn shutdown(&self) {
        self.expect_exit();
        let mut child = self.inner.child.lock().await;
        terminate_child(&mut child, self.inner.process_group_id).await;
    }
}

async fn terminate_child(child: &mut Child, process_group_id: Option<i32>) -> bool {
    #[cfg(unix)]
    if let Some(pgid) = process_group_id {
        // Only signal the group while the leader is still unreaped. `wait`
        // reaps the leader; signalling the pgid afterwards could hit a
        // recycled process group, so check liveness before every signal.
        if matches!(child.try_wait(), Ok(None)) {
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
        }
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await,
            Ok(Ok(_))
        ) {
            return true;
        }
        if matches!(child.try_wait(), Ok(None)) {
            kill_process_group(process_group_id);
            let _ = child.start_kill();
        }
        return matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await,
            Ok(Ok(_))
        );
    }
    match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
        Ok(Ok(_)) => true,
        _ => matches!(child.kill().await, Ok(())),
    }
}

fn kill_process_group(process_group_id: Option<i32>) {
    #[cfg(unix)]
    if let Some(pgid) = process_group_id {
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = process_group_id;
}

async fn resolve_response(inner: &Arc<RpcInner>, mut frame: Value) -> Option<Value> {
    let id = frame
        .get("id")
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .and_then(|s| s.parse::<u64>().ok());
    let pending = if let Some(id) = id {
        inner.pending.lock().await.remove(&id)
    } else {
        None
    };
    if let Some(p) = pending {
        let success = frame
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if success {
            let _ =
                p.tx.send(Ok(frame.get("data").cloned().unwrap_or(Value::Null)));
        } else {
            let err = util::redact_secrets(
                frame
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("The harness reported an error."),
            );
            let _ = p.tx.send(Err(err));
        }
        return None;
    }
    // OMP acknowledges prompt immediately, then can send a second failure
    // using that same id if async scheduling fails. The first response has
    // already resolved its caller; do not discard the consequential error.
    if frame.get("success").and_then(Value::as_bool) == Some(false) {
        if let Some(error) = frame.get_mut("error") {
            if let Some(text) = error.as_str() {
                *error = Value::String(util::redact_secrets(text));
            }
        }
        Some(frame)
    } else {
        None
    }
}

async fn fail_all(inner: &Arc<RpcInner>, msg: &str) {
    let mut pending = inner.pending.lock().await;
    for (_, p) in pending.drain() {
        let _ = p.tx.send(Err(msg.to_string()));
    }
}

async fn mark_exited(
    inner: &Arc<RpcInner>,
    on_exit: Arc<Box<dyn Fn(Option<i32>, String, bool) + Send + Sync>>,
    code: Option<i32>,
    stderr: String,
) {
    if inner.exited.swap(true, Ordering::SeqCst) {
        return;
    }
    fail_all(inner, "Harness process ended").await;
    let expected = inner.expected_exit.load(Ordering::SeqCst);
    on_exit(code, util::redact_secrets(&stderr), expected);
}

/// Feed one parsed frame into the reassembler. Returns `Ok(Some(frame))` for a
/// complete frame, `Ok(None)` when more chunks are needed, `Err` on violation.
fn push_chunk(state: &mut Option<ChunkState>, frame: Value) -> Result<Option<Value>, String> {
    let is_chunk = frame.get("type").and_then(Value::as_str) == Some("rpc_chunk");
    if !is_chunk {
        if state.is_some() {
            return Err("rpc chunk sequence interrupted".into());
        }
        if !frame.is_object() {
            return Err("rpc frame must be an object".into());
        }
        return Ok(Some(frame));
    }
    let chunk_id = frame
        .get("chunkId")
        .and_then(Value::as_str)
        .ok_or("invalid rpc chunk metadata")?;
    if chunk_id.is_empty() || chunk_id.len() > 128 {
        return Err("invalid rpc chunk metadata".into());
    }
    let index = frame
        .get("index")
        .and_then(Value::as_u64)
        .ok_or("invalid rpc chunk metadata")? as usize;
    let count = frame
        .get("count")
        .and_then(Value::as_u64)
        .ok_or("invalid rpc chunk metadata")? as usize;
    let byte_length = frame
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or("invalid rpc chunk metadata")? as usize;
    let max_chunks = MAX_REASSEMBLED_BYTES.div_ceil(CHUNK_PAYLOAD_BYTES);
    if count < 2
        || count > max_chunks
        || index >= count
        || byte_length < MAX_FRAME_BYTES
        || byte_length > MAX_REASSEMBLED_BYTES
    {
        return Err("invalid rpc chunk metadata".into());
    }
    let data_b64 = frame
        .get("data")
        .and_then(Value::as_str)
        .ok_or("invalid rpc chunk data")?;
    let engine = base64::engine::general_purpose::STANDARD;
    let data = engine
        .decode(data_b64)
        .map_err(|_| "invalid rpc chunk data")?;
    if engine.encode(&data) != data_b64 {
        return Err("invalid rpc chunk data".into());
    }
    if data.len() > CHUNK_PAYLOAD_BYTES {
        return Err("rpc chunk payload exceeds the transport limit".into());
    }
    match state.as_mut() {
        None => {
            if index != 0 {
                return Err("rpc chunk sequence must start at index 0".into());
            }
            *state = Some(ChunkState {
                chunk_id: chunk_id.to_string(),
                count,
                byte_length,
                next_index: 1,
                received: data.len(),
                chunks: vec![data],
            });
        }
        Some(st) => {
            if st.chunk_id != chunk_id
                || st.count != count
                || st.byte_length != byte_length
                || st.next_index != index
            {
                return Err("rpc chunk sequence mismatch".into());
            }
            st.received += data.len();
            st.chunks.push(data);
            st.next_index += 1;
            if st.received > st.byte_length {
                return Err("rpc chunk sequence exceeds declared length".into());
            }
        }
    }
    let st = state.as_ref().expect("chunk state");
    if st.next_index < st.count {
        return Ok(None);
    }
    if st.received != st.byte_length {
        return Err("rpc chunk sequence length mismatch".into());
    }
    let st = state.take().expect("chunk state");
    let mut buf = Vec::with_capacity(st.byte_length);
    for c in st.chunks {
        buf.extend_from_slice(&c);
    }
    let text = String::from_utf8(buf).map_err(|_| "rpc chunk payload is not valid UTF-8")?;
    let value: Value =
        serde_json::from_str(&text).map_err(|_| "rpc chunk payload is not valid JSON")?;
    if !value.is_object() {
        return Err("rpc frame must be an object".into());
    }
    Ok(Some(value))
}

/// Project high-volume harness frames to the data the UI actually consumes:
/// - `message_update.message` and `assistantMessageEvent.partial` are
///   cumulative snapshots; stream deltas carry the change instead.
/// - `agent_end.messages` repeats completed message_end frames and can be huge.
/// - `toolcall_start` keeps id/name extracted from the partial before removal.
pub fn normalize_outgoing_frame(mut frame: Value) -> Value {
    let ftype = frame.get("type").and_then(Value::as_str).unwrap_or("");
    if ftype == "agent_end" {
        if let Some(obj) = frame.as_object_mut() {
            obj.remove("messages");
        }
        return frame;
    }
    if matches!(
        ftype,
        "tool_execution_update" | "tool_stream_update" | "tool_update"
    ) {
        if let Some(obj) = frame.as_object_mut() {
            for key in ["partialResult", "result", "partial"] {
                if let Some(partial) = obj.get_mut(key) {
                    cap_partial_result(partial);
                }
            }
        }
        return frame;
    }
    if ftype != "message_update" {
        return frame;
    }
    if let Some(obj) = frame.as_object_mut() {
        obj.remove("message");
        if let Some(ame) = obj.get_mut("assistantMessageEvent") {
            if let Some(ame_obj) = ame.as_object_mut() {
                let partial = ame_obj.remove("partial");
                let is_toolcall_start =
                    ame_obj.get("type").and_then(Value::as_str) == Some("toolcall_start");
                let content_index = ame_obj
                    .get("contentIndex")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize);
                if let (true, Some(partial), Some(idx)) =
                    (is_toolcall_start, partial.as_ref(), content_index)
                {
                    if let Some(content) = partial
                        .get("content")
                        .and_then(Value::as_array)
                        .and_then(|arr| arr.get(idx))
                    {
                        if let Some(id) = content.get("id").and_then(Value::as_str) {
                            ame_obj.insert("id".to_string(), json!(id));
                        }
                        if let Some(name) = content.get("name").and_then(Value::as_str) {
                            ame_obj.insert("name".to_string(), json!(name));
                        }
                    }
                }
            }
        }
    }
    frame
}

/// Tool progress frames repeat all output so far, so a long command streams
/// O(n²) bytes to the webview, which only renders a short tail. Forward the tail.
const MAX_PARTIAL_TEXT_BYTES: usize = 64 * 1024;

fn keep_tail(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut cut = text.len() - max;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    text.replace_range(..cut, "");
}

fn cap_partial_result(partial: &mut Value) {
    let Some(obj) = partial.as_object_mut() else {
        return;
    };
    if let Some(content) = obj.get_mut("content").and_then(Value::as_array_mut) {
        for block in content {
            if let Some(Value::String(text)) = block.get_mut("text") {
                keep_tail(text, MAX_PARTIAL_TEXT_BYTES);
            }
        }
    }
    if let Some(details) = obj.get_mut("details").and_then(Value::as_object_mut) {
        for value in details.values_mut() {
            if let Value::String(text) = value {
                keep_tail(text, MAX_PARTIAL_TEXT_BYTES);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_message_update_bulk() {
        let frame = json!({
            "type": "message_update",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "huge"}]},
            "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "hi", "partial": {"big": true}}
        });
        let out = normalize_outgoing_frame(frame);
        assert!(out.get("message").is_none());
        let ame = out.get("assistantMessageEvent").unwrap();
        assert!(ame.get("partial").is_none());
        assert_eq!(ame.get("delta").and_then(Value::as_str), Some("hi"));
    }

    #[test]
    fn drops_redundant_agent_end_messages_but_keeps_completion() {
        let frame = json!({
            "type": "agent_end",
            "messages": [{ "role": "assistant", "content": [{ "type": "text", "text": "final" }] }],
            "isTerminal": false,
            "willRetry": true
        });
        let out = normalize_outgoing_frame(frame);
        assert!(out.get("messages").is_none());
        assert_eq!(out["isTerminal"], false);
        assert_eq!(out["willRetry"], true);
    }

    #[test]
    fn toolcall_start_surfaces_id_name() {
        let frame = json!({
            "type": "message_update",
            "message": {},
            "assistantMessageEvent": {
                "type": "toolcall_start",
                "contentIndex": 1,
                "partial": {"content": [
                    {"type": "text", "text": "x"},
                    {"type": "toolCall", "id": "call_1", "name": "bash", "arguments": {}}
                ]}
            }
        });
        let out = normalize_outgoing_frame(frame);
        let ame = out.get("assistantMessageEvent").unwrap();
        assert_eq!(ame.get("id").and_then(Value::as_str), Some("call_1"));
        assert_eq!(ame.get("name").and_then(Value::as_str), Some("bash"));
        assert!(ame.get("partial").is_none());
    }

    #[test]
    fn caps_cumulative_tool_partials_to_utf8_tail() {
        let long = format!("{}é-end", "x".repeat(MAX_PARTIAL_TEXT_BYTES * 2));
        let frame = json!({
            "type": "tool_execution_update",
            "toolCallId": "t1",
            "partialResult": {
                "content": [{"type": "text", "text": long}],
                "details": {"output": long, "exitCode": 0}
            }
        });
        let out = normalize_outgoing_frame(frame);
        let text = out["partialResult"]["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= MAX_PARTIAL_TEXT_BYTES);
        assert!(text.ends_with("é-end"));
        let output = out["partialResult"]["details"]["output"].as_str().unwrap();
        assert!(output.len() <= MAX_PARTIAL_TEXT_BYTES);
        assert_eq!(out["partialResult"]["details"]["exitCode"], 0);
        assert_eq!(out["toolCallId"], "t1");
    }

    #[test]
    fn keep_tail_respects_char_boundaries() {
        let mut text = "aéé".to_string();
        keep_tail(&mut text, 3);
        assert_eq!(text, "é");
    }

    #[test]
    fn chunk_reassembly_roundtrip() {
        // Payload > 1 MiB so a real multi-chunk sequence is exercised.
        let payload = json!({"type": "response", "id": "7", "success": true, "data": {"x": "y".repeat(2_000_000)}});
        let bytes = serde_json::to_vec(&payload).unwrap();
        let engine = base64::engine::general_purpose::STANDARD;
        let count = bytes.len().div_ceil(CHUNK_PAYLOAD_BYTES);
        let mut state = None;
        let mut out = None;
        for i in 0..count {
            let end = ((i + 1) * CHUNK_PAYLOAD_BYTES).min(bytes.len());
            let chunk = json!({"type": "rpc_chunk", "chunkId": "rpc-1", "index": i, "count": count, "byteLength": bytes.len(), "data": engine.encode(&bytes[i * CHUNK_PAYLOAD_BYTES..end])});
            out = push_chunk(&mut state, chunk).unwrap();
        }
        assert_eq!(out.unwrap(), payload);
    }

    #[test]
    fn chunk_sequence_violations() {
        let engine = base64::engine::general_purpose::STANDARD;
        let mut state = None;
        // Must start at index 0.
        let bad = json!({"type": "rpc_chunk", "chunkId": "c", "index": 1, "count": 2, "byteLength": MAX_FRAME_BYTES + 2, "data": engine.encode(b"ab")});
        assert!(push_chunk(&mut state, bad).is_err());
        // Total byteLength below one frame is not a valid chunked payload.
        let small = json!({"type": "rpc_chunk", "chunkId": "c", "index": 0, "count": 2, "byteLength": 10, "data": engine.encode(b"ab")});
        assert!(push_chunk(&mut state, small).is_err());
        // Non-chunk frame mid-sequence is an interruption.
        let first = json!({"type": "rpc_chunk", "chunkId": "c", "index": 0, "count": 2, "byteLength": MAX_FRAME_BYTES + 2, "data": engine.encode(b"ab")});
        assert!(push_chunk(&mut state, first).unwrap().is_none());
        assert!(push_chunk(&mut state, json!({"type": "other"})).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_chunk_terminates_harness_and_fails_closed() {
        use std::process::Stdio;
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("printf '%s\\n' '{\"type\":\"rpc_chunk\",\"chunkId\":\"bad\",\"index\":0,\"count\":2,\"byteLength\":4,\"data\":\"YWJjZA==\"}'; exec sleep 30")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let done = std::sync::Arc::new(parking_lot::Mutex::new(Some(tx)));
        let client = RpcClient::attach(
            child,
            stdout,
            stderr,
            RpcHandlers {
                on_event: Box::new(|_| {}),
                on_exit: Box::new(move |code, error, expected| {
                    if let Some(tx) = done.lock().take() {
                        let _ = tx.send((code, error, expected));
                    }
                }),
                on_ready: None,
            },
        );
        let (_, error, expected) = tokio::time::timeout(std::time::Duration::from_secs(8), rx)
            .await
            .unwrap()
            .unwrap();
        assert!(!expected);
        assert!(error.contains("RPC protocol error"));
        assert!(client.is_exited());
        assert!(client.call("get_state", Map::new()).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn forwards_prompt_failure_after_immediate_ack() {
        use std::process::Stdio;
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("IFS= read -r line; printf '%s\\n' '{\"type\":\"response\",\"id\":\"1\",\"command\":\"prompt\",\"success\":true}' '{\"type\":\"response\",\"id\":\"1\",\"command\":\"prompt\",\"success\":false,\"error\":\"Model could not start\"}'")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let late_error = Arc::new(parking_lot::Mutex::new(Some(tx)));
        let client = RpcClient::attach(
            child,
            stdout,
            stderr,
            RpcHandlers {
                on_event: Box::new(move |frame| {
                    if frame.get("type").and_then(Value::as_str) == Some("response") {
                        if let Some(tx) = late_error.lock().take() {
                            let _ = tx.send(frame);
                        }
                    }
                }),
                on_exit: Box::new(|_, _, _| {}),
                on_ready: None,
            },
        );
        assert!(client.call("prompt", Map::new()).await.is_ok());
        let late = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .expect("late RPC error must be forwarded")
            .unwrap();
        assert_eq!(late["error"], "Model could not start");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_group_shutdown_reaps_descendant() {
        use std::process::Stdio;
        let pid_file = std::env::temp_dir().join(format!("omp-pgid-{}", uuid::Uuid::new_v4()));
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let mut child = tokio::process::Command::new("/bin/sh")
            .process_group(0)
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let client = RpcClient::attach(
            child,
            stdout,
            stderr,
            RpcHandlers {
                on_event: Box::new(|_| {}),
                on_exit: Box::new(|_, _, _| {}),
                on_ready: None,
            },
        );
        let descendant: i32 = loop {
            if let Ok(pid) = std::fs::read_to_string(&pid_file) {
                break pid.trim().parse().unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        client.shutdown().await;
        for _ in 0..100 {
            if unsafe { libc::kill(descendant, 0) } != 0 {
                std::fs::remove_file(pid_file).ok();
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("descendant survived process-group shutdown");
    }
}
