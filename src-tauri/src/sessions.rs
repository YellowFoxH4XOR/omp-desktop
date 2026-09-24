use crate::dto::{AgentInfo, HarnessKind};
use crate::error::{AppError, AppResult};
use crate::util;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::Path;

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

type CacheEntry = (u64, std::time::SystemTime, ScannedSession);
static SESSION_CACHE: std::sync::LazyLock<
    Mutex<std::collections::HashMap<(HarnessKind, std::path::PathBuf), CacheEntry>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Parse the head of a session JSONL file.
/// OMP: first line may be a `title` slot, then a `session` header.
/// Pi: first line is the `session` header; `session_info` may follow.
fn parse_session_file(kind: HarnessKind, path: &Path) -> Option<ScannedSession> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
    let cache_key = (kind, path.to_path_buf());
    if let Some((len, time, session)) = SESSION_CACHE.lock().get(&cache_key) {
        if *len == metadata.len() && *time == modified {
            return Some(session.clone());
        }
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
    for (index, line) in std::io::BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else { break };
        // After the fixed header, parse only possible title changes and the
        // first user message. Pi session_info may occur anywhere in the log.
        if index > 2
            && !line.contains("\"session_info\"")
            && (first_user.is_some() || !line.contains("\"role\":\"user\""))
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
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
    let session = ScannedSession {
        session_file: path.to_string_lossy().to_string(),
        session_id,
        cwd,
        title,
        created_at,
        modified_unix,
    };
    SESSION_CACHE
        .lock()
        .insert(cache_key, (metadata.len(), modified, session.clone()));
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
    use std::io::BufRead;
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
    let mut entries = std::collections::HashMap::<String, (Option<String>, Option<Value>)>::new();
    let mut leaf = None;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) == Some("session") {
            continue;
        }
        let parent = entry
            .get("parentId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let message = match entry.get("type").and_then(Value::as_str) {
            Some("message") => entry.get("message").cloned(),
            Some("custom_message")
                if entry.get("display").and_then(Value::as_bool) != Some(false) =>
            {
                Some(json!({
                    "role":"custom",
                    "customType":entry.get("customType"),
                    "content":entry.get("content"),
                    "display":true,
                    "details":entry.get("details")
                }))
            }
            Some("branch_summary") => {
                Some(json!({"role":"branchSummary","summary":entry.get("summary")}))
            }
            Some("compaction") => {
                Some(json!({"role":"compactionSummary","summary":entry.get("summary")}))
            }
            _ => None,
        };
        entries.insert(id.to_string(), (parent, message));
        leaf = Some(id.to_string());
    }
    let mut seen = std::collections::HashSet::new();
    let mut messages = Vec::new();
    while let Some(id) = leaf {
        if !seen.insert(id.clone()) {
            break;
        }
        let Some((parent, message)) = entries.get(&id) else {
            break;
        };
        if let Some(message) = message {
            messages.push(message.clone());
        }
        leaf = parent.clone();
    }
    messages.reverse();
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;

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
