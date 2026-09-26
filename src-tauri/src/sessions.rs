use crate::util;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

const SESSION_CACHE_CAPACITY: usize = 512;
const SESSION_CACHE_BYTE_BUDGET: usize = 1024 * 1024;
const MAX_SESSION_METADATA_FIELD_BYTES: usize = 4 * 1024;
const MAX_SESSION_CWD_BYTES: usize = 8 * 1024;
const MAX_METADATA_SCAN_BYTES: u64 = 64 * 1024 * 1024;
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
    Fragment,
    Oversized,
}

/// Read one newline-delimited record without allowing a hostile session file
/// to grow an allocation beyond Pi's frame limit. An unterminated tail is a
/// normal in-progress write and is never parsed.
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
                BoundedRecord::Fragment
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

/// Read bounded recent text for orchestration without starting a thread's
/// process (which could load user extensions). Images and tool payloads are
/// omitted here; this is a report, not a reconstructed session snapshot.
pub fn recent_text(path: &Path) -> crate::error::AppResult<Vec<Value>> {
    recent_text_at(&util::private_session_file(path)?)
}

fn recent_text_at(path: &Path) -> crate::error::AppResult<Vec<Value>> {
    if std::fs::metadata(path)?.len() > MAX_METADATA_SCAN_BYTES {
        return Err(crate::error::AppError::new("This session exceeds the 64 MiB inspection limit. Recent output is unavailable; do not treat an older prefix as current."));
    }
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut record = Vec::new();
    let mut messages = std::collections::VecDeque::new();
    let mut scanned = 0usize;
    loop {
        match read_bounded_record(&mut reader, &mut record, MAX_JSONL_RECORD_BYTES)? {
            BoundedRecord::Eof | BoundedRecord::Fragment => break,
            BoundedRecord::Oversized => {
                return Err(crate::error::AppError::new(
                    "Session contains an oversized record; recent output is unavailable.",
                ))
            }
            BoundedRecord::Line => {}
        }
        scanned += record.len();
        if scanned > MAX_METADATA_SCAN_BYTES as usize {
            return Err(crate::error::AppError::new(
                "Session grew beyond the inspection limit; recent output is unavailable.",
            ));
        }
        let Ok(entry) = serde_json::from_slice::<Value>(trimmed_record(&record)) else {
            continue;
        };
        let Some(message) = entry.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        let text = if let Some(text) = message.get("content").and_then(Value::as_str) {
            text.to_string()
        } else {
            message
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };
        if !text.is_empty() {
            messages.push_back(serde_json::json!({"role":role,"text":util::redact_secrets(&bounded_text(&text, 4096))}));
            if messages.len() > 24 {
                messages.pop_front();
            }
        }
    }
    Ok(messages.into_iter().collect())
}

fn trimmed_record(record: &[u8]) -> &[u8] {
    let record = record.strip_suffix(b"\n").unwrap_or(record);
    record.strip_suffix(b"\r").unwrap_or(record)
}

#[derive(Debug, Clone)]
pub struct ScannedSession {
    pub session_file: String,
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub created_at: String,
    pub modified_unix: u64,
}

/// Scan Pi's session directory for `cwd`, newest first.
pub fn scan_sessions(cwd: &Path) -> Vec<ScannedSession> {
    // No environment override or external layout is considered. Reject symlink
    // escapes before reading any private session metadata.
    for dir in [
        util::pidesk_root(),
        util::agent_dir(),
        util::agent_dir().join("sessions"),
    ] {
        if util::check_owned_path(&dir, true).is_err() {
            return Vec::new();
        }
    }
    scan_session_directory(&util::session_dir_for(cwd), cwd)
}

fn scan_session_directory(dir: &Path, cwd: &Path) -> Vec<ScannedSession> {
    let mut out = Vec::new();
    if util::check_owned_path(dir, true).is_err() {
        return out;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
            || util::check_owned_path(&path, false).is_err()
        {
            continue;
        }
        if let Some(session) = parse_session_file(&path) {
            // Verify the header's cwd as well as the directory layout.
            if util::resolve_path(Path::new(&session.cwd)) == util::resolve_path(cwd) {
                out.push(session);
            }
        }
    }
    out.sort_by(|a, b| b.modified_unix.cmp(&a.modified_unix));
    out
}

struct CacheEntry {
    len: u64,
    modified: std::time::SystemTime,
    session: ScannedSession,
    last_access: u64,
    bytes: usize,
}

struct SessionCache {
    entries: HashMap<PathBuf, CacheEntry>,
    access_clock: u64,
    capacity: usize,
    byte_budget: usize,
    total_bytes: usize,
}

fn cache_entry_bytes(path: &Path, session: &ScannedSession) -> usize {
    path.as_os_str().len()
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
        path: &Path,
        len: u64,
        modified: std::time::SystemTime,
    ) -> Option<ScannedSession> {
        let access = self.next_access();
        let entry = self.entries.get_mut(path)?;
        if entry.len != len || entry.modified != modified {
            return None;
        }
        entry.last_access = access;
        Some(entry.session.clone())
    }

    fn insert(
        &mut self,
        path: PathBuf,
        len: u64,
        modified: std::time::SystemTime,
        session: ScannedSession,
    ) {
        let session = bounded_session(session);
        let bytes = cache_entry_bytes(&path, &session);
        if bytes > self.byte_budget {
            return;
        }
        let access = self.next_access();
        if let Some(previous) = self.entries.remove(&path) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.bytes);
        }
        while self.entries.len() >= self.capacity
            || self.total_bytes.saturating_add(bytes) > self.byte_budget
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.entries.insert(
            path,
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

/// Parse Pi's JSONL metadata. Pi sessions always start with a `session`
/// record; later `session_info` records may rename them.
fn parse_session_file(path: &Path) -> Option<ScannedSession> {
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
    if let Some(session) = SESSION_CACHE.lock().get(path, metadata.len(), modified) {
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
    let mut reader = std::io::BufReader::new(file);
    let mut line = Vec::new();
    let mut scanned_bytes = 0u64;
    let mut first_record = true;

    loop {
        match read_bounded_record(&mut reader, &mut line, MAX_JSONL_RECORD_BYTES) {
            Ok(BoundedRecord::Line) => {}
            Ok(BoundedRecord::Fragment | BoundedRecord::Eof) => break,
            Ok(BoundedRecord::Oversized) | Err(_) => return None,
        }
        scanned_bytes = scanned_bytes.saturating_add(line.len() as u64);
        if scanned_bytes > MAX_METADATA_SCAN_BYTES {
            return None;
        }
        let Ok(value) = serde_json::from_slice::<Value>(trimmed_record(&line)) else {
            if first_record {
                return None;
            }
            continue;
        };
        if first_record {
            first_record = false;
            if value.get("type").and_then(Value::as_str) != Some("session") {
                return None;
            }
        }
        match value.get("type").and_then(Value::as_str) {
            Some("session") if session_id.is_empty() => {
                session_id = value.get("id").and_then(Value::as_str)?.to_string();
                cwd = value.get("cwd").and_then(Value::as_str)?.to_string();
                created_at = value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
            }
            Some("session_info") => {
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
                if message
                    .and_then(|message| message.get("role"))
                    .and_then(Value::as_str)
                    == Some("user")
                {
                    let content = message.and_then(|message| message.get("content"));
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
    SESSION_CACHE.lock().insert(
        path.to_path_buf(),
        metadata.len(),
        modified,
        session.clone(),
    );
    Some(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_text_rejects_oversized_records_instead_of_stale_prefixes() {
        let path =
            std::env::temp_dir().join(format!("pidesk-intern-record-{}", uuid::Uuid::new_v4()));
        let mut bytes = b"{\"message\":{\"role\":\"assistant\",\"content\":\"old\"}}\n".to_vec();
        bytes.extend(vec![b'x'; MAX_JSONL_RECORD_BYTES + 1]);
        bytes.push(b'\n');
        std::fs::write(&path, bytes).unwrap();
        assert!(recent_text_at(&path)
            .unwrap_err()
            .to_string()
            .contains("oversized record"));
        std::fs::remove_file(path).unwrap();
    }

    fn session(id: &str) -> ScannedSession {
        ScannedSession {
            session_file: id.to_string(),
            session_id: id.to_string(),
            cwd: "/tmp".to_string(),
            title: id.to_string(),
            created_at: String::new(),
            modified_unix: 0,
        }
    }

    #[test]
    fn session_cache_is_lru_bounded() {
        let mut cache = SessionCache::new(2);
        let now = std::time::SystemTime::UNIX_EPOCH;
        let key = |id: &str| PathBuf::from(id);
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
        let dir = std::env::temp_dir().join(format!("pidesk-huge-title-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        let huge_title = "x".repeat(MAX_SESSION_METADATA_FIELD_BYTES * 2);
        std::fs::write(
            &path,
            format!(
                "{{\"type\":\"session\",\"id\":\"session-1\",\"cwd\":\"/tmp/repo\"}}\n{{\"type\":\"session_info\",\"name\":\"{huge_title}\"}}\n"
            ),
        )
        .unwrap();
        let parsed = parse_session_file(&path).unwrap();
        assert_eq!(parsed.title.len(), MAX_SESSION_METADATA_FIELD_BYTES);
        let cached = SESSION_CACHE.lock().get(
            &path,
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
    fn session_cache_evicts_at_byte_budget() {
        let now = std::time::SystemTime::UNIX_EPOCH;
        let first = PathBuf::from("first");
        let second = PathBuf::from("second");
        let mut cache = SessionCache::with_byte_budget(8, 1);
        cache.insert(first.clone(), 1, now, session("first"));
        assert!(cache.entries.is_empty());
        cache.byte_budget = cache_entry_bytes(&second, &session("second"));
        cache.insert(first.clone(), 1, now, session("first"));
        cache.insert(second.clone(), 1, now, session("second"));
        assert!(cache.total_bytes <= cache.byte_budget);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key(&second));
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
    fn pi_title_uses_latest_name_or_first_prompt() {
        let dir =
            std::env::temp_dir().join(format!("pidesk-session-title-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let named = dir.join("named.jsonl");
        std::fs::write(&named, "{\"type\":\"session\",\"id\":\"pi-1\",\"cwd\":\"/tmp/repo\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"First prompt\"}}\n{\"type\":\"session_info\",\"name\":\"First name\"}\n{\"type\":\"session_info\",\"name\":\"Latest name\"}\n").unwrap();
        assert_eq!(parse_session_file(&named).unwrap().title, "Latest name");

        let untitled = dir.join("untitled.jsonl");
        std::fs::write(&untitled, "{\"type\":\"session\",\"id\":\"pi-2\",\"cwd\":\"/tmp/repo\"}\n{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Investigate race condition\"}]}}\n").unwrap();
        assert_eq!(
            parse_session_file(&untitled).unwrap().title,
            "Investigate race condition"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn discovery_skips_symlinked_session_files_and_directories() {
        let dir = std::env::temp_dir().join(format!("pidesk-discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("private.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"session\",\"version\":3,\"id\":\"private\",\"cwd\":\"/tmp/project\"}\n",
        )
        .unwrap();
        let cwd = Path::new("/tmp/project");
        assert_eq!(scan_session_directory(&dir, cwd).len(), 1);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&path, dir.join("external.jsonl")).unwrap();
            assert_eq!(scan_session_directory(&dir, cwd).len(), 1);
            std::os::unix::fs::symlink(&dir, dir.join("linked")).unwrap();
            assert!(scan_session_directory(&dir.join("linked"), cwd).is_empty());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn non_pi_header_is_not_discovered() {
        let dir =
            std::env::temp_dir().join(format!("pidesk-session-header-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("other.jsonl");
        std::fs::write(&path, "{\"type\":\"title\",\"title\":\"Other\"}\n{\"type\":\"session\",\"id\":\"other\",\"cwd\":\"/tmp/repo\"}\n").unwrap();
        assert!(parse_session_file(&path).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
