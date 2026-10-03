//! Mapping Pi's JSON values onto πDesk's DTOs.

use super::*;

// ----------------------------------------------------------------------
// Value → DTO mapping
// ----------------------------------------------------------------------

pub(super) fn value_to_model(v: &Value) -> Option<ModelInfo> {
    Some(ModelInfo {
        provider: v.get("provider").and_then(Value::as_str)?.to_string(),
        id: v.get("id").and_then(Value::as_str)?.to_string(),
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        context_window: v.get("contextWindow").and_then(Value::as_u64),
        reasoning: v.get("reasoning").and_then(Value::as_bool),
        max_tokens: v.get("maxTokens").and_then(Value::as_u64),
        images: v
            .get("input")
            .and_then(Value::as_array)
            .map(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("image"))),
        cost: v.get("cost").and_then(|cost| {
            Some(crate::dto::ModelCost {
                input: cost.get("input").and_then(Value::as_f64)?,
                output: cost.get("output").and_then(Value::as_f64)?,
            })
        }),
    })
}

pub(super) fn build_session_state(state: &Value, stats: Option<&Value>) -> SessionState {
    SessionState {
        session_id: state
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        session_file: state
            .get("sessionFile")
            .and_then(Value::as_str)
            .map(String::from),
        session_name: state
            .get("sessionName")
            .and_then(Value::as_str)
            .map(String::from),
        model: state.get("model").and_then(value_to_model),
        thinking_level: state
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(String::from),
        is_streaming: state
            .get("isStreaming")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        context_usage: stats
            .and_then(|s| s.get("contextUsage"))
            .map(build_context_usage),
    }
}

pub(super) fn build_context_usage(v: &Value) -> ContextUsage {
    ContextUsage {
        tokens: v.get("tokens").and_then(Value::as_u64),
        context_window: v.get("contextWindow").and_then(Value::as_u64).unwrap_or(0),
        percent: v.get("percent").and_then(Value::as_f64),
    }
}

pub(super) fn build_usage(stats: &Value) -> Usage {
    let t = stats.get("tokens").cloned().unwrap_or(Value::Null);
    let num = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        tokens: TokenCounts {
            input: num("input"),
            output: num("output"),
            cache_read: num("cacheRead"),
            cache_write: num("cacheWrite"),
            total: num("total"),
        },
        cost: stats.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        context_usage: stats.get("contextUsage").map(build_context_usage),
    }
}
