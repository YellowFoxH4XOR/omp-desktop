use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessKind {
    Omp,
    Pi,
}

impl HarnessKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessKind::Omp => "omp",
            HarnessKind::Pi => "pi",
        }
    }
    pub fn binary_name(self) -> &'static str {
        self.as_str()
    }
    pub fn display_name(self) -> &'static str {
        match self {
            HarnessKind::Omp => "OMP",
            HarnessKind::Pi => "Pi",
        }
    }
}

impl std::str::FromStr for HarnessKind {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "omp" => Ok(HarnessKind::Omp),
            "pi" => Ok(HarnessKind::Pi),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInstallation {
    pub kind: HarnessKind,
    pub path: String,
    pub version: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub path: String,
    pub display_name: String,
    pub preferred_harness: HarnessKind,
    pub is_git: bool,
    pub created_at: String,
    pub last_opened_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub project_id: String,
    pub harness: HarnessKind,
    pub session_id: String,
    pub session_file: String,
    pub cwd: String,
    pub title: String,
    pub pinned: bool,
    pub archived: bool,
    pub status: String,
    pub created_at: String,
    pub last_viewed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub context_window: u64,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub tokens: TokenCounts,
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<ContextUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilities {
    pub agents: bool,
    pub nested_agents: bool,
    pub agent_steering: bool,
    pub agent_kill: bool,
    pub agent_revive: bool,
    pub plan_mode: bool,
    pub permissions: bool,
    pub model_switching: bool,
    pub effort_levels: bool,
    pub context_usage: bool,
    pub token_usage: bool,
    pub worktrees: bool,
}

impl HarnessCapabilities {
    pub fn for_kind(kind: HarnessKind) -> Self {
        match kind {
            HarnessKind::Omp => Self {
                agents: true,
                nested_agents: true,
                agent_steering: false,
                agent_kill: false,
                agent_revive: false,
                plan_mode: false,
                permissions: true,
                model_switching: true,
                effort_levels: true,
                context_usage: true,
                token_usage: true,
                worktrees: true,
            },
            HarnessKind::Pi => Self {
                agents: false,
                nested_agents: false,
                agent_steering: false,
                agent_kill: false,
                agent_revive: false,
                plan_mode: false,
                // Pi has no capability handshake in its RPC contract. Do not
                // advertise permission UI until the host can verify support.
                permissions: false,
                // These are enabled per snapshot from successful RPC queries.
                model_switching: false,
                effort_levels: false,
                context_usage: false,
                token_usage: false,
                worktrees: true,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    pub is_streaming: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<ContextUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub thread: Thread,
    pub messages: Vec<Value>,
    pub state: SessionState,
    pub models: Vec<ModelInfo>,
    pub levels: Vec<String>,
    pub capabilities: HarnessCapabilities,
    pub agents: Vec<AgentInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiResponse {
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub confirmed: Option<bool>,
    #[serde(default)]
    pub cancelled: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesSummary {
    pub is_repo: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub files: Vec<ChangedFile>,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFile {
    pub path: String,
    pub old: String,
    pub current: String,
    pub current_hash: String,
    pub binary: bool,
    pub too_large: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginProvider {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub authenticated: bool,
}

/// Events emitted to the frontend on the `desktop-event` channel.
/// Field names are camelCase to match `BackendEvent` in types.ts.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum BackendEvent {
    Rpc {
        thread_id: String,
        frame: Value,
    },
    Exited {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        stderr: String,
        expected: bool,
    },
    GitChanged {
        thread_id: String,
    },
    InstallProgress {
        kind: HarnessKind,
        line: String,
    },
    InstallFinished {
        kind: HarnessKind,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}
