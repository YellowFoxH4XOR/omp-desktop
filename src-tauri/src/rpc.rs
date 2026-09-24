use crate::error::{AppError, AppResult};
use crate::util;
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{oneshot, Mutex};

const MAX_FRAME_BYTES: usize = 1_048_576; // 1 MiB per wire frame
const MAX_REASSEMBLED_BYTES: usize = 67_108_864; // 64 MiB reassembled
const CHUNK_PAYLOAD_BYTES: usize = 262_144; // 256 KiB per chunk payload
const STDERR_TAIL_BYTES: usize = 16 * 1024;
const DEFAULT_CMD_TIMEOUT_SECS: u64 = 60;

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> std::io::Result<Option<String>> {
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
    String::from_utf8(buf)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
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
        mut child: Child,
        stdout: ChildStdout,
        stderr: ChildStderr,
        handlers: RpcHandlers,
    ) -> Self {
        let stdin = child.stdin.take().expect("child stdin piped");
        let inner = Arc::new(RpcInner {
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            exited: AtomicBool::new(false),
            expected_exit: AtomicBool::new(false),
            stderr_tail: Mutex::new(String::new()),
        });
        // stderr tail collector
        {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut stderr = BufReader::new(stderr);
                while let Ok(Some(line)) = read_bounded_line(&mut stderr, MAX_FRAME_BYTES).await {
                    let mut tail = inner.stderr_tail.lock().await;
                    tail.push_str(&line);
                    tail.push('\n');
                    if tail.len() > STDERR_TAIL_BYTES {
                        let mut cut = tail.len() - STDERR_TAIL_BYTES;
                        while !tail.is_char_boundary(cut) {
                            cut += 1;
                        }
                        tail.drain(..cut);
                    }
                }
            });
        }
        // stdout reader + exit monitor
        {
            let inner = inner.clone();
            tokio::spawn(async move {
                let mut chunk_state: Option<ChunkState> = None;
                let mut stdout = BufReader::new(stdout);
                loop {
                    match read_bounded_line(&mut stdout, MAX_FRAME_BYTES).await {
                        Ok(Some(line)) => {
                            if line.trim().is_empty() {
                                continue;
                            }
                            let parsed: Result<Value, _> = serde_json::from_str(&line);
                            let frame = match parsed {
                                Ok(v) => v,
                                Err(_) => continue, // non-JSON noise on stdout
                            };
                            // Reassemble rpc_chunk streams (OMP protocol v2).
                            let frame = match push_chunk(&mut chunk_state, frame) {
                                Ok(Some(f)) => f,
                                Ok(None) => continue,
                                Err(e) => {
                                    // Protocol violation: treat as fatal.
                                    let tail = inner.stderr_tail.lock().await.clone();
                                    let msg = util::redact_secrets(&if tail.is_empty() {
                                        format!("RPC protocol error: {e}")
                                    } else {
                                        format!("RPC protocol error: {e}. Stderr: {tail}")
                                    });
                                    fail_all(&inner, &msg).await;
                                    let mut child = inner.child.lock().await;
                                    let _ = terminate_child(&mut child).await;
                                    drop(child);
                                    mark_exited(&inner, handlers.on_exit, None, msg).await;
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
                        Ok(None) => break, // EOF
                        Err(error) => {
                            let msg = util::redact_secrets(&format!("RPC protocol error: {error}"));
                            fail_all(&inner, &msg).await;
                            let mut child = inner.child.lock().await;
                            let _ = terminate_child(&mut child).await;
                            drop(child);
                            mark_exited(&inner, handlers.on_exit, None, msg).await;
                            return;
                        }
                    }
                }
                let code = {
                    let mut child = inner.child.lock().await;
                    match tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
                        .await
                    {
                        Ok(Ok(status)) => status.code(),
                        _ => {
                            terminate_child(&mut child).await;
                            None
                        }
                    }
                };
                let tail = inner.stderr_tail.lock().await.clone();
                fail_all(&inner, "Harness process ended").await;
                mark_exited(&inner, handlers.on_exit, code, tail).await;
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
            pending.insert(id, Pending { tx });
        }
        args.insert("id".to_string(), json!(id.to_string()));
        args.insert("type".to_string(), json!(command));
        let mut frame = serde_json::to_string(&Value::Object(args))
            .map_err(|e| AppError::new(format!("Could not encode request: {e}")))?;
        frame.push('\n');
        {
            let mut stdin = self.inner.stdin.lock().await;
            if let Err(e) = stdin.write_all(frame.as_bytes()).await {
                self.inner.pending.lock().await.remove(&id);
                return Err(AppError::new(format!(
                    "Could not write to the harness process: {e}"
                )));
            }
            let _ = stdin.flush().await;
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
        let mut stdin = self.inner.stdin.lock().await;
        stdin
            .write_all(s.as_bytes())
            .await
            .map_err(|e| AppError::new(format!("Could not write to the harness process: {e}")))?;
        let _ = stdin.flush().await;
        Ok(())
    }

    /// Graceful stop: SIGTERM, then SIGKILL after a grace period.
    pub async fn shutdown(&self) {
        self.expect_exit();
        let mut child = self.inner.child.lock().await;
        terminate_child(&mut child).await;
    }
}

async fn terminate_child(child: &mut Child) -> bool {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let pgid = pid as i32;
        // The harness is spawned as its own process-group leader. Signal the
        // group so tools/PTY descendants cannot outlive stop, restart, or app
        // shutdown. A negative pid targets that group on Unix.
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
            Ok(Ok(_)) => {
                // The leader may exit before one of its descendants. Ensure the
                // whole group is gone even when wait() returned successfully.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
                return true;
            }
            _ => unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            },
        }
    }
    #[cfg(unix)]
    if child.id().is_some() {
        return matches!(child.kill().await, Ok(()));
    }
    false
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
    on_exit: Box<dyn Fn(Option<i32>, String, bool) + Send + Sync>,
    code: Option<i32>,
    stderr: String,
) {
    if inner.exited.swap(true, Ordering::SeqCst) {
        return;
    }
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
    if ftype != "message_update" {
        return frame;
    }
    if let Some(obj) = frame.as_object_mut() {
        obj.remove("message");
        if let Some(ame) = obj.get_mut("assistantMessageEvent") {
            if let Some(ame_obj) = ame.as_object_mut() {
                let is_toolcall_start =
                    ame_obj.get("type").and_then(Value::as_str) == Some("toolcall_start");
                if is_toolcall_start {
                    let content_index = ame_obj
                        .get("contentIndex")
                        .and_then(Value::as_u64)
                        .map(|v| v as usize);
                    if let (Some(partial), Some(idx)) =
                        (ame_obj.get("partial").cloned(), content_index)
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
                ame_obj.remove("partial");
            }
        }
    }
    frame
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
