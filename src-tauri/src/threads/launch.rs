//! What a spawn passes to Pi: resume and mode arguments, the appended
//! system prompt, and the slash commands read back from it.

use super::*;

pub(super) fn resume_arguments(session_id: &str, session_file: &str) -> AppResult<Vec<String>> {
    match (session_id, session_file) {
        ("", "") => Ok(Vec::new()),
        ("", _) | (_, "") => Err(AppError::new(
            "This thread's saved session mapping is incomplete. Restore its session file before reopening it.",
        )),
        (_, mapped) if Path::new(mapped).is_file() => Ok(vec![
            "--session".into(),
            util::private_session_file(Path::new(mapped))?
                .to_string_lossy()
                .into_owned(),
        ]),
        (id, mapped) => {
            // Pi reports its journal path before the first turn is written, so a
            // thread opened but never messaged has no file yet. `--session-id`
            // looks the id up in the private session dir (finding a moved
            // journal) and otherwise recreates the same session identity;
            // `--session <missing path>` would mint a different id instead.
            util::private_session_target(Path::new(mapped)).map_err(|_| {
                AppError::new(format!(
                    "The saved session file {mapped} is missing or has moved. Restore it before reopening this thread."
                ))
            })?;
            if !is_valid_session_id(id) {
                return Err(AppError::new(
                    "This thread's saved session id is invalid and cannot be reopened.",
                ));
            }
            Ok(vec!["--session-id".into(), id.to_string()])
        }
    }
}

/// Tells the model what πDesk's transcript can render. Passed as a flag rather
/// than written to `APPEND_SYSTEM.md`, which stays free for the user's own text.
pub(super) const RENDERING_GUIDE: &str = "You are running inside πDesk, a desktop app that renders your replies as GitHub-flavored Markdown. \
When a diagram would help (architecture, flows, sequences, state machines, ER models), write it as a fenced ```mermaid code block; πDesk renders Mermaid as a real diagram. \
Prefer Mermaid over ASCII or box-drawing art, keep node labels short, and use plain code fences only for actual code or terminal output. \
When the user writes @path (for example @src/app.ts or @docs/), it names that file or folder relative to your working directory; read it before answering about it.";

/// Threads run on πDesk's private Pi, but their shell sees the user's real
/// home, where other tools (the terminal Pi, shared MCP files) keep their own
/// config and credentials. Point the agent at its own profile instead.
pub(super) const PROFILE_GUIDE: &str = "You run on πDesk's own private Pi profile, not the user's terminal Pi. Your Pi settings, installed extensions, MCP server config (mcp.json), and sessions live in $PI_CODING_AGENT_DIR (~/.pidesk/agent); extensions keep their files under ~/.pidesk/home. To see which MCP servers you have, use the mcp tool (for example mcp({}) or mcp({ search: \"…\" })), not config files. ~/.pi, ~/.config/mcp, ~/.agents, and other apps' config directories belong to the user's other tools and may hold credentials: do not go looking in them. Questions about your MCP servers, extensions, or settings are about your own profile. If the user explicitly asks about another tool's files, πDesk will ask them to allow the read first.";

/// Plan/Auto modes, loaded into every Pi πDesk starts. The file is rewritten
/// atomically on each spawn so it always matches this build.
pub(super) const MODES_EXTENSION: &str = include_str!("../pidesk-modes.mjs");
pub(super) const MODES_COMMAND: &str = "pidesk-mode";

/// Write the bundled modes extension (also the agent shell's real-home hook)
/// where every πDesk-launched Pi, including the terminal's, loads it.
pub(crate) fn write_modes_extension(root: &Path) -> AppResult<PathBuf> {
    let directory = root.join("extensions");
    util::ensure_private_directory(root, &directory)?;
    let extension = directory.join("pidesk-modes.mjs");
    let temp = directory.join(format!("pidesk-modes-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temp, MODES_EXTENSION)?;
    std::fs::rename(temp, &extension)?;
    Ok(extension)
}

pub(super) fn modes_arguments(root: &Path, plan: bool, locked: bool) -> AppResult<Vec<String>> {
    let extension = write_modes_extension(root)?;
    let mut args = vec![
        "--extension".to_string(),
        extension.to_string_lossy().into_owned(),
    ];
    if plan || locked {
        args.push("--pidesk-plan".into());
    }
    if locked {
        args.push("--pidesk-plan-locked".into());
    }
    Ok(args)
}

pub(super) fn spawn_arguments(session_dir: &Path) -> Vec<String> {
    // sessionDir is read before project trust, so --no-approve alone does
    // not isolate it. The explicit directory is authoritative for Pi.
    vec![
        "--mode".into(),
        "rpc".into(),
        "--no-approve".into(),
        "--session-dir".into(),
        session_dir.to_string_lossy().into_owned(),
        "--append-system-prompt".into(),
        RENDERING_GUIDE.into(),
        "--append-system-prompt".into(),
        PROFILE_GUIDE.into(),
    ]
}

/// Pi's `get_commands` reply as bounded, display-safe entries. πDesk's own
/// host controls (`pidesk-*`) are not user commands.
pub(super) fn commands_from(value: &Value) -> Vec<crate::dto::CommandInfo> {
    const MAX_COMMANDS: usize = 300;
    let clip = |text: &str, max: usize| text.chars().take(max).collect::<String>();
    value
        .get("commands")
        .and_then(Value::as_array)
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| {
                    let name = command.get("name")?.as_str()?.trim();
                    if name.is_empty()
                        || name.len() > 120
                        || name.starts_with("pidesk-")
                        || name.chars().any(char::is_whitespace)
                    {
                        return None;
                    }
                    Some(crate::dto::CommandInfo {
                        name: name.to_string(),
                        description: command
                            .get("description")
                            .and_then(Value::as_str)
                            .map(|text| clip(text.trim(), 200))
                            .filter(|text| !text.is_empty()),
                        source: command
                            .get("source")
                            .and_then(Value::as_str)
                            .filter(|source| matches!(*source, "extension" | "skill" | "prompt"))
                            .unwrap_or("extension")
                            .to_string(),
                    })
                })
                .take(MAX_COMMANDS)
                .collect()
        })
        .unwrap_or_default()
}

/// Pi accepts letters, digits, `.`, `_`, and `-`; a leading alphanumeric keeps
/// the value from ever being parsed as a CLI flag.
pub(super) fn is_valid_session_id(id: &str) -> bool {
    id.len() <= 128
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}
