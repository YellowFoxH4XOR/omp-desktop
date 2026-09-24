use crate::dto::{AgentInfo, HarnessKind};
use crate::error::{AppError, AppResult};
use crate::util;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const SESSION_CACHE_CAPACITY: usize = 512;
const SESSION_CACHE_BYTE_BUDGET: usize = 1024 * 1024;
const MAX_SESSION_METADATA_FIELD_BYTES: usize = 4 * 1024;
const MAX_SESSION_CWD_BYTES: usize = 8 * 1024;
const MAX_METADATA_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSCRIPT_RECORDS: usize = 50_000;
const MAX_JSONL_RECORD_BYTES: usize = 8 * 1024 * 1024;

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn bounded_session(mut session: ScannedSession) -> ScannedSession {
    session.session_id = bounded_text(&session.session_id, MAX_SESSION_METADATA_FIELD_BYTES);
    session.title = bounded_text(&session.title, MAX_SESSION_METADATA_FIELD_BYTES);
    session.created_at = bounded_text(&session.created_at, MAX_SESSION_METADATA_FIELD_BYTES);
    session.cwd = bounded_text(&session.cwd, MAX_SESSION_CWD_BYTES);
    session
}

#[derive(Debug)]
enum BoundedRecord {
    Eof,
    Line,
    Oversized,
}

/// Read one newline-delimited record without allowing malformed or hostile
/// files to grow the allocation beyond the protocol limit.
fn read_bounded_record(
    reader: &mut impl BufRead,
    record: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<BoundedRecord> {
    record.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if record.is_empty() {
                BoundedRecord::Eof
            } else {
                BoundedRecord::Line
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if record.len().saturating_add(take) > max_bytes {
            return Ok(BoundedRecord::Oversized);
        }
        record.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            return Ok(BoundedRecord::Line);
        }
    }
}

fn trimmed_record(record: &[u8]) -> &[u8] {
    let record = record.strip_suffix(b"\n").unwrap_or(record);
    record.strip_suffix(b"\r").unwrap_or(record)
}

/// Metadata recovered from one harness session file on disk.
#[derive(Debug, Clone)]
pub struct ScannedSession {
    pub session_file: String,
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub created_at: String,
    /// File mtime in unix seconds; used for recency ordering.
    pub modified_unix: u64,
}

/// Scan the harness session directory for `cwd` and return every session file
/// that belongs to it, newest first.
pub fn scan_sessions(kind: HarnessKind, cwd: &Path) -> Vec<ScannedSession> {
    let dir = util::session_dir_for(kind, cwd);
    let mut out = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(s) = parse_session_file(kind, &path) {
            // When PI_CODING_AGENT_SESSION_DIR flattens the layout, sessions
            // from many cwds share one dir; keep only this project's.
            if util::resolve_path(Path::new(&s.cwd)) != util::resolve_path(cwd) {
                continue;
            }
            out.push(s);
        }
    }
    out.sort_by(|a, b| b.modified_unix.cmp(&a.modified_unix));
    out
}

type CacheKey = (HarnessKind, PathBuf);

struct CacheEntry {
    len: u64,
    modified: std::time::SystemTime,
    session: ScannedSession,
    last_access: u64,
    bytes: usize,
}

struct SessionCache {
    entries: HashMap<CacheKey, CacheEntry>,
    access_clock: u64,
    capacity: usize,
    byte_budget: usize,
    total_bytes: usize,
}

fn cache_entry_bytes(key: &CacheKey, session: &ScannedSession) -> usize {
    key.1.as_os_str().len()
        + session.session_file.len()
        + session.session_id.len()
        + session.cwd.len()
        + session.title.len()
        + session.created_at.len()
        + std::mem::size_of::<CacheEntry>()
}

impl SessionCache {
    fn new(capacity: usize) -> Self {
        Self::with_byte_budget(capacity, SESSION_CACHE_BYTE_BUDGET)
    }

    fn with_byte_budget(capacity: usize, byte_budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            access_clock: 0,
            capacity,
            byte_budget,
            total_bytes: 0,
        }
    }

    fn next_access(&mut self) -> u64 {
        self.access_clock = self.access_clock.wrapping_add(1);
        self.access_clock
    }

    fn get(
        &mut self,
        key: &CacheKey,
        len: u64,
        modified: std::time::SystemTime,
    ) -> Option<ScannedSession> {
        let access = self.next_access();
        let entry = self.entries.get_mut(key)?;
        if entry.len != len || entry.modified != modified {
            return None;
        }
        entry.last_access = access;
        Some(entry.session.clone())
    }

    fn insert(
        &mut self,
        key: CacheKey,
        len: u64,
        modified: std::time::SystemTime,
        session: ScannedSession,
    ) {
        let session = bounded_session(session);
        let bytes = cache_entry_bytes(&key, &session);
        if bytes > self.byte_budget {
            return;
        }
        let access = self.next_access();
        if let Some(previous) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        while self.entries.len() >= self.capacity
            || self.total_bytes.saturating_add(bytes) > self.byte_budget
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.entries.insert(
            key,
            CacheEntry {
                len,
                modified,
                session,
                last_access: access,
                bytes,
            },
        );
    }
}

static SESSION_CACHE: std::sync::LazyLock<Mutex<SessionCache>> =
    std::sync::LazyLock::new(|| Mutex::new(SessionCache::new(SESSION_CACHE_CAPACITY)));

/// Parse the head of a session JSONL file.
/// OMP: first line may be a `title` slot, then a `session` header.
/// Pi: first line is the `session` header; `session_info` may follow.
fn parse_session_file(kind: HarnessKind, path: &Path) -> Option<ScannedSession> {
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
    let cache_key = (kind, path.to_path_buf());
    if let Some(session) = SESSION_CACHE
        .lock()
        .get(&cache_key, metadata.len(), modified)
    {
        return Some(session);
    }
    let modified_unix = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut session_id = String::new();
    let mut cwd = String::new();
    let mut created_at = String::new();
    let mut title = String::new();
    let mut first_user = None;
    let mut omp_title_slot = false;
    let mut reader = std::io::BufReader::new(file);
    let mut line = Vec::new();
    let mut scanned_bytes = 0u64;
    loop {
        match read_bounded_record(&mut reader, &mut line, MAX_JSONL_RECORD_BYTES) {
            Ok(BoundedRecord::Line) => {}
            Ok(BoundedRecord::Eof) => break,
            Ok(BoundedRecord::Oversized) => return None,
            Err(_) => return None,
        }
        scanned_bytes = scanned_bytes.saturating_add(line.len() as u64);
        // Pi may rename a session late, so metadata extraction has a hard
        // aggregate budget rather than following an unbounded JSONL tail.
        if scanned_bytes > MAX_METADATA_SCAN_BYTES {
            return None;
        }
        let Ok(value) = serde_json::from_slice::<Value>(trimmed_record(&line)) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("session") => {
                session_id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                cwd = value
                    .get("cwd")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                created_at = value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
            }
            Some("title") => {
                omp_title_slot = true;
                if let Some(name) = value.get("title").and_then(Value::as_str) {
                    if !name.trim().is_empty() {
                        title = name.trim().to_string();
                    }
                }
            }
            Some("session_info") if kind == HarnessKind::Pi => {
                for key in ["title", "name", "sessionName"] {
                    if let Some(name) = value.get(key).and_then(Value::as_str) {
                        if !name.trim().is_empty() {
                            title = name.trim().to_string();
                            break;
                        }
                    }
                }
            }
            Some("message") if first_user.is_none() => {
                let message = value.get("message");
                if message.and_then(|m| m.get("role")).and_then(Value::as_str) == Some("user") {
                    let content = message.and_then(|m| m.get("content"));
                    let text = content.and_then(Value::as_str).or_else(|| {
                        content.and_then(Value::as_array)?.iter().find_map(|part| {
                            (part.get("type").and_then(Value::as_str) == Some("text"))
                                .then(|| part.get("text").and_then(Value::as_str))
                                .flatten()
                        })
                    });
                    first_user = text
                        .and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
                        .map(|line| line.trim().chars().take(80).collect::<String>());
                }
            }
            _ => {}
        }
        if kind == HarnessKind::Omp
            && !session_id.is_empty()
            && !cwd.is_empty()
            && (!title.is_empty() || first_user.is_some())
        {
            break;
        }
    }
    // OMP reserves a fixed-width title slot; Pi does not. In a shared flat
    // directory this is the reliable discriminator between otherwise
    // schema-compatible session headers.
    if (kind == HarnessKind::Omp) != omp_title_slot {
        return None;
    }
    if session_id.is_empty() || cwd.is_empty() {
        return None;
    }
    if title.is_empty() {
        title = first_user.unwrap_or_else(|| "New thread".to_string());
    }
    let session = bounded_session(ScannedSession {
        session_file: path.to_string_lossy().to_string(),
        session_id,
        cwd,
        title,
        created_at,
        modified_unix,
    });
    SESSION_CACHE
        .lock()
        .insert(cache_key, metadata.len(), modified, session.clone());
    Some(session)
}

/// OMP stores subagent transcripts next to the main JSONL in its artifacts
/// directory (`<session stem>/<agent-id>.jsonl`). Exclude advisor internals.
pub fn historical_agents(session_file: &str) -> Vec<AgentInfo> {
    let dir = Path::new(session_file).with_extension("");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().ok().is_some_and(|kind| kind.is_file())
            || path.extension().and_then(|e| e.to_str()) != Some("jsonl")
        {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if id.starts_with("__") || !valid_agent_id(id) {
            continue;
        }
        let parent_id = id.rsplit_once('.').map(|(parent, _)| parent.to_string());
        let status = agent_status_from_tail(&path);
        agents.push(AgentInfo {
            id: id.to_string(),
            parent_id,
            parent_tool_call_id: None,
            name: id.rsplit('.').next().unwrap_or(id).to_string(),
            role: None,
            task: None,
            status,
            model: None,
            effort: None,
            activity: None,
            tokens: None,
            context_tokens: None,
            context_window: None,
            cost: None,
            duration_ms: None,
            tool_count: None,
            session_file: Some(path.to_string_lossy().to_string()),
            worktree_path: None,
        });
    }
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    agents
}

fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '#'))
}

fn agent_status_from_tail(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return "parked".into();
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return "parked".into();
    };
    let offset = len.saturating_sub(128 * 1024);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return "parked".into();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return "parked".into();
    }
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines().rev() {
        if !line.contains("\"role\":\"assistant\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let reason = entry
            .get("message")
            .and_then(|message| message.get("stopReason"))
            .and_then(Value::as_str);
        return match reason {
            Some("stop") => "completed",
            Some("error") => "failed",
            Some("aborted") => "aborted",
            _ => "parked",
        }
        .to_string();
    }
    "parked".into()
}

/// Reconstruct the active branch of a saved subagent transcript. The harness
/// remains authoritative for execution; this read-only path works after its
/// in-process RPC registry has been disposed.
pub fn read_agent_messages(session_file: &str, agent_id: &str) -> AppResult<Vec<Value>> {
    if !valid_agent_id(agent_id) || agent_id.starts_with("__") {
        return Err(AppError::new("Invalid agent id."));
    }
    let dir = Path::new(session_file).with_extension("");
    let path = dir.join(format!("{agent_id}.jsonl"));
    let resolved_dir = std::fs::canonicalize(&dir)
        .map_err(|_| AppError::new("Agent transcript directory is unavailable."))?;
    let resolved_path = std::fs::canonicalize(&path)
        .map_err(|_| AppError::new("Agent transcript is unavailable."))?;
    if !resolved_path.starts_with(&resolved_dir) {
        return Err(AppError::new(
            "Agent transcript path escapes the session directory.",
        ));
    }
    let file = std::fs::File::open(&resolved_path)
        .map_err(|e| AppError::new(format!("Could not open agent transcript: {e}")))?;
    let file_len = file
        .metadata()
        .map_err(|e| AppError::new(format!("Could not inspect agent transcript: {e}")))?
        .len();
    if file_len > MAX_TRANSCRIPT_BYTES {
        return Err(AppError::new(
            "Agent transcript is too large to reconstruct.",
        ));
    }

    // Keep only parent links and byte ranges in the first pass. Values are
    // parsed again only for the active branch, avoiding a second full copy of
    // every JSONL record.
    struct NodeRef {
        parent: Option<String>,
        offset: u64,
        len: usize,
        kind: RecordKind,
    }
    #[derive(Clone, Copy)]
    enum RecordKind {
        Message,
        CustomMessage,
        BranchSummary,
        Compaction,
        Other,
    }
    let mut nodes = HashMap::<String, NodeRef>::new();
    let mut reader = std::io::BufReader::new(file);
    let mut line = Vec::new();
    let mut offset = 0u64;
    let mut leaf = None;
    let mut record_count = 0usize;
    loop {
        let record_offset = offset;
        match read_bounded_record(&mut reader, &mut line, MAX_JSONL_RECORD_BYTES) {
            Ok(BoundedRecord::Line) => {}
            Ok(BoundedRecord::Eof) => break,
            Ok(BoundedRecord::Oversized) => {
                return Err(AppError::new("Agent transcript record is too large."));
            }
            Err(e) => {
                return Err(AppError::new(format!(
                    "Could not read agent transcript: {e}"
                )))
            }
        }
        offset = offset.saturating_add(line.len() as u64);
        record_count += 1;
        if record_count > MAX_TRANSCRIPT_RECORDS || offset > MAX_TRANSCRIPT_BYTES {
            return Err(AppError::new(
                "Agent transcript is too large to reconstruct.",
            ));
        }
        let entry: Value = serde_json::from_slice(trimmed_record(&line)).map_err(|e| {
            AppError::new(format!("Agent transcript contains an invalid record: {e}"))
        })?;
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) == Some("session") {
            continue;
        }
        let kind = match entry.get("type").and_then(Value::as_str) {
            Some("message") => RecordKind::Message,
            Some("custom_message")
                if entry.get("display").and_then(Value::as_bool) != Some(false) =>
            {
                RecordKind::CustomMessage
            }
            Some("branch_summary") => RecordKind::BranchSummary,
            Some("compaction") => RecordKind::Compaction,
            _ => RecordKind::Other,
        };
        nodes.insert(
            id.to_string(),
            NodeRef {
                parent: entry
                    .get("parentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                offset: record_offset,
                len: line.len(),
                kind,
            },
        );
        leaf = Some(id.to_string());
    }

    let mut branch = Vec::new();
    let mut seen = HashSet::new();
    while let Some(id) = leaf {
        if !seen.insert(id.clone()) {
            return Err(AppError::new("Agent transcript contains a cyclic branch."));
        }
        let Some(node) = nodes.get(&id) else {
            return Err(AppError::new("Agent transcript branch is incomplete."));
        };
        branch.push((node.offset, node.len, node.kind));
        leaf = node.parent.clone();
    }
    branch.reverse();

    let mut source = std::fs::File::open(&resolved_path)
        .map_err(|e| AppError::new(format!("Could not reopen agent transcript: {e}")))?;
    let mut record = Vec::new();
    let mut messages = Vec::with_capacity(branch.len());
    for (offset, len, kind) in branch {
        if len > MAX_JSONL_RECORD_BYTES {
            return Err(AppError::new("Agent transcript record is too large."));
        }
        source
            .seek(SeekFrom::Start(offset))
            .map_err(|e| AppError::new(format!("Could not seek agent transcript: {e}")))?;
        record.resize(len, 0);
        source
            .read_exact(&mut record)
            .map_err(|e| AppError::new(format!("Could not read agent transcript: {e}")))?;
        let entry: Value = serde_json::from_slice(trimmed_record(&record)).map_err(|e| {
            AppError::new(format!("Agent transcript contains an invalid record: {e}"))
        })?;
        let message = match kind {
            RecordKind::Message => entry.get("message").cloned(),
            RecordKind::CustomMessage => Some(json!({
                "role":"custom",
                "customType":entry.get("customType"),
                "content":entry.get("content"),
                "display":true,
                "details":entry.get("details")
            })),
            RecordKind::BranchSummary => {
                Some(json!({"role":"branchSummary","summary":entry.get("summary")}))
            }
            RecordKind::Compaction => {
                Some(json!({"role":"compactionSummary","summary":entry.get("summary")}))
            }
            RecordKind::Other => None,
        };
        if let Some(message) = message {
            messages.push(message);
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cache_is_lru_bounded() {
        let mut cache = SessionCache::new(2);
        let now = std::time::SystemTime::UNIX_EPOCH;
        let session = |id: &str| ScannedSession {
            session_file: id.to_string(),
            session_id: id.to_string(),
            cwd: "/tmp".to_string(),
            title: id.to_string(),
            created_at: String::new(),
            modified_unix: 0,
        };
        let key = |id: &str| (HarnessKind::Omp, PathBuf::from(id));
        cache.insert(key("one"), 1, now, session("one"));
        cache.insert(key("two"), 1, now, session("two"));
        assert!(cache.get(&key("one"), 1, now).is_some());
        cache.insert(key("three"), 1, now, session("three"));
        assert!(cache.get(&key("two"), 1, now).is_none());
        assert!(cache.get(&key("one"), 1, now).is_some());
        assert!(cache.get(&key("three"), 1, now).is_some());
    }
    #[test]
    fn session_metadata_is_truncated_before_return_and_cache_insert() {
        let dir = std::env::temp_dir().join(format!("omp-huge-title-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let huge_title = "x".repeat(MAX_SESSION_METADATA_FIELD_BYTES * 2);
        std::fs::write(
            &path,
            format!(
                "{{\"type\":\"title\",\"title\":\"{huge_title}\"}}\n{{\"type\":\"session\",\"id\":\"session-1\",\"cwd\":\"/tmp/repo\"}}\n"
            ),
        )
        .unwrap();
        let session = parse_session_file(HarnessKind::Omp, &path).unwrap();
        assert_eq!(session.title.len(), MAX_SESSION_METADATA_FIELD_BYTES);
        let cached = SESSION_CACHE.lock().get(
            &(HarnessKind::Omp, path.clone()),
            std::fs::metadata(&path).unwrap().len(),
            std::fs::metadata(&path).unwrap().modified().unwrap(),
        );
        assert_eq!(
            cached.unwrap().title.len(),
            MAX_SESSION_METADATA_FIELD_BYTES
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn session_cache_evicts_when_byte_budget_is_reached() {
        let now = std::time::SystemTime::UNIX_EPOCH;
        let session = |id: &str| ScannedSession {
            session_file: id.to_string(),
            session_id: id.to_string(),
            cwd: "/tmp".to_string(),
            title: "title".to_string(),
            created_at: String::new(),
            modified_unix: 0,
        };
        let first_key = (HarnessKind::Omp, PathBuf::from("first"));
        let second_key = (HarnessKind::Omp, PathBuf::from("second"));
        let mut cache = SessionCache::with_byte_budget(8, 1);
        cache.insert(first_key.clone(), 1, now, session("first"));
        assert!(cache.entries.is_empty());
        cache.byte_budget = cache_entry_bytes(&second_key, &session("second"));
        cache.insert(first_key.clone(), 1, now, session("first"));
        cache.insert(second_key.clone(), 1, now, session("second"));
        assert!(cache.total_bytes <= cache.byte_budget);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key(&second_key));
        assert!(!cache.entries.contains_key(&first_key));
    }

    #[test]
    fn bounded_record_reader_rejects_oversized_lines() {
        let input = std::io::Cursor::new(b"tiny\noversized".to_vec());
        let mut reader = std::io::BufReader::new(input);
        let mut record = Vec::new();
        assert!(matches!(
            read_bounded_record(&mut reader, &mut record, 5),
            Ok(BoundedRecord::Line)
        ));
        assert!(matches!(
            read_bounded_record(&mut reader, &mut record, 5),
            Ok(BoundedRecord::Oversized)
        ));
    }

    #[test]
    fn session_titles_follow_each_harness_format() {
        let dir = std::env::temp_dir().join(format!("omp-session-titles-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let omp = dir.join("omp.jsonl");
        std::fs::write(&omp, "{\"type\":\"title\",\"title\":\"Harness title\"}\n{\"type\":\"session\",\"id\":\"omp-1\",\"cwd\":\"/tmp/repo\",\"timestamp\":\"2026-09-23T00:00:00Z\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"First prompt\"}}\n").unwrap();
        assert_eq!(
            parse_session_file(HarnessKind::Omp, &omp).unwrap().title,
            "Harness title"
        );

        let pi = dir.join("pi.jsonl");
        std::fs::write(&pi, "{\"type\":\"session\",\"id\":\"pi-1\",\"cwd\":\"/tmp/repo\",\"timestamp\":\"2026-09-23T00:00:00Z\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"First prompt\"}}\n{\"type\":\"session_info\",\"name\":\"First name\"}\n{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n{\"type\":\"session_info\",\"name\":\"Latest name\"}\n").unwrap();
        assert_eq!(
            parse_session_file(HarnessKind::Pi, &pi).unwrap().title,
            "Latest name"
        );

        let untitled = dir.join("untitled.jsonl");
        std::fs::write(&untitled, "{\"type\":\"session\",\"id\":\"pi-2\",\"cwd\":\"/tmp/repo\",\"timestamp\":\"2026-09-23T00:00:00Z\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Investigate race condition\"}]}}\n").unwrap();
        assert_eq!(
            parse_session_file(HarnessKind::Pi, &untitled)
                .unwrap()
                .title,
            "Investigate race condition"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn flat_directory_uses_title_slot_to_classify_harness() {
        let dir = std::env::temp_dir().join(format!("omp-flat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let omp = dir.join("omp.jsonl");
        std::fs::write(&omp, "{\"type\":\"title\",\"v\":1,\"title\":\"OMP\"}\n{\"type\":\"session\",\"version\":3,\"id\":\"omp\",\"timestamp\":\"2026-09-23T00:00:00Z\",\"cwd\":\"/tmp/repo\"}\n").unwrap();
        let pi = dir.join("pi.jsonl");
        std::fs::write(&pi, "{\"type\":\"session\",\"version\":3,\"id\":\"pi\",\"timestamp\":\"2026-09-23T00:00:00Z\",\"cwd\":\"/tmp/repo\"}\n").unwrap();
        assert_eq!(
            parse_session_file(HarnessKind::Omp, &omp)
                .unwrap()
                .session_id,
            "omp"
        );
        assert!(parse_session_file(HarnessKind::Pi, &omp).is_none());
        assert_eq!(
            parse_session_file(HarnessKind::Pi, &pi).unwrap().session_id,
            "pi"
        );
        assert!(parse_session_file(HarnessKind::Omp, &pi).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_nested_agent_transcript_survives_rpc_restart() {
        let dir = std::env::temp_dir().join(format!("omp-agent-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let main = dir.join("session.jsonl");
        std::fs::write(
            &main,
            "{\"type\":\"session\",\"id\":\"root\",\"cwd\":\"/tmp/repo\"}\n",
        )
        .unwrap();
        let artifacts = main.with_extension("");
        std::fs::create_dir_all(&artifacts).unwrap();
        let child = artifacts.join("Backend.DatabaseExpert.jsonl");
        std::fs::write(&child, "{\"type\":\"session\",\"id\":\"child\",\"cwd\":\"/tmp/repo\"}\n{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"Inspect database\"}}\n{\"type\":\"message\",\"id\":\"b\",\"parentId\":\"a\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done\"}],\"stopReason\":\"stop\"}}\n").unwrap();
        let agents = historical_agents(&main.to_string_lossy());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "Backend.DatabaseExpert");
        assert_eq!(agents[0].parent_id.as_deref(), Some("Backend"));
        assert_eq!(agents[0].status, "completed");
        let messages = read_agent_messages(&main.to_string_lossy(), &agents[0].id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"][0]["text"], "Done");
        assert!(read_agent_messages(&main.to_string_lossy(), "../escape").is_err());
        #[cfg(unix)]
        {
            let outside = dir.join("outside.jsonl");
            std::fs::write(&outside, "{\"type\":\"message\",\"id\":\"x\"}\n").unwrap();
            std::os::unix::fs::symlink(&outside, artifacts.join("Outsider.jsonl")).unwrap();
            assert_eq!(historical_agents(&main.to_string_lossy()).len(), 1);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
