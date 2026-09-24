//! Live smoke tests against real harness binaries. Ignored by default:
//! `cargo test --test rpc_live -- --ignored`

use omp_desktop_lib::rpc::{RpcClient, RpcHandlers};
use serde_json::{json, Map, Value};
use std::process::Stdio;
use std::sync::mpsc::channel;
use tokio::process::Command;

fn spawn(
    bin: &str,
    args: &[&str],
    cwd: &std::path::Path,
) -> (RpcClient, std::sync::mpsc::Receiver<Value>) {
    let mut child = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn harness");
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = channel::<Value>();
    let client = RpcClient::attach(
        child,
        stdout,
        stderr,
        RpcHandlers {
            on_event: Box::new(move |f| {
                let _ = tx.send(f);
            }),
            on_exit: Box::new(|_, _, _| {}),
            on_ready: None,
        },
    );
    (client, rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn omp_handshake_and_state() {
    let dir = std::env::temp_dir().join("omp-rpc-live");
    std::fs::create_dir_all(&dir).unwrap();
    let (client, rx) = spawn("omp", &["--mode", "rpc"], &dir);
    // The harness claims stdin before extension discovery; get_state can be
    // sent immediately, without timing-sensitive sleeps.
    let neg = client
        .call(
            "negotiate_protocol",
            Map::from_iter([("protocolVersion".into(), json!(2))]),
        )
        .await
        .expect("negotiate");
    assert_eq!(neg.get("protocolVersion").and_then(Value::as_u64), Some(2));
    let state = client
        .call("get_state", Map::new())
        .await
        .expect("get_state");
    assert!(state.get("sessionId").and_then(Value::as_str).is_some());
    let msgs = client
        .call(
            "get_messages_page",
            Map::from_iter([("limit".into(), json!(256))]),
        )
        .await
        .expect("get_messages_page");
    assert!(msgs.get("messages").is_some());
    let stats = client
        .call("get_session_stats", Map::new())
        .await
        .expect("stats");
    assert!(stats.get("tokens").is_some());
    client.shutdown().await;
    // Drain events to prove frames flowed.
    let mut saw = 0;
    while rx
        .recv_timeout(std::time::Duration::from_millis(200))
        .is_ok()
    {
        saw += 1;
    }
    assert!(saw > 0, "expected at least one event frame");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn pi_no_ready_state() {
    let dir = std::env::temp_dir().join("pi-rpc-live");
    std::fs::create_dir_all(&dir).unwrap();
    let (client, _rx) = spawn("pi", &["--mode", "rpc"], &dir);
    let state = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        client.call("get_state", Map::new()),
    )
    .await
    .expect("timeout")
    .expect("get_state");
    assert!(state.get("sessionId").and_then(Value::as_str).is_some());
    let msgs = client
        .call("get_messages", Map::new())
        .await
        .expect("get_messages");
    assert!(msgs.get("messages").is_some());
    client.shutdown().await;
}

async fn smoke_prompt(bin: &str, args: &[&str], terminal_event: &str) {
    let dir = std::env::temp_dir().join(format!("omp-rpc-prompt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (client, events) = spawn(bin, args, &dir);
    if bin == "omp" {
        let negotiated = client
            .call(
                "negotiate_protocol",
                Map::from_iter([("protocolVersion".into(), json!(2))]),
            )
            .await
            .expect("OMP protocol negotiation");
        assert_eq!(negotiated["protocolVersion"], 2);
    }
    let state = client
        .call("get_state", Map::new())
        .await
        .expect("session state");
    assert!(state.get("sessionId").and_then(Value::as_str).is_some());
    client
        .call(
            "prompt",
            Map::from_iter([(
                "message".into(),
                json!("Reply with exactly the word ready."),
            )]),
        )
        .await
        .expect("prompt acknowledgement");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut answer = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let event = events
            .recv_timeout(remaining)
            .expect("harness turn must settle with an assistant reply");
        if event.get("type").and_then(Value::as_str) == Some("message_end")
            && event
                .get("message")
                .and_then(|message| message.get("role"))
                .and_then(Value::as_str)
                == Some("assistant")
        {
            if let Some(content) = event
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_array)
            {
                for block in content {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        answer.push_str(text);
                    }
                }
            }
        }
        if event.get("type").and_then(Value::as_str) == Some(terminal_event)
            && (terminal_event != "agent_end"
                || event.get("isTerminal") != Some(&Value::Bool(false)))
        {
            break;
        }
    }
    client.shutdown().await;
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        answer.to_ascii_lowercase().contains("ready"),
        "expected a real assistant answer, got {answer:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "sends one real model prompt through OMP"]
async fn omp_prompt_streams_reply() {
    smoke_prompt(
        "omp",
        &[
            "--mode",
            "rpc",
            "--no-session",
            "--no-tools",
            "--model",
            "devin/swe-2",
        ],
        "agent_end",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "sends one real model prompt through Pi"]
async fn pi_prompt_streams_reply() {
    smoke_prompt(
        "pi",
        &["--mode", "rpc", "--no-session", "--no-tools"],
        "agent_settled",
    )
    .await;
}
