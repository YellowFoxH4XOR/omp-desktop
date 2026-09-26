//! Live smoke tests against πDesk's private Pi. Never installs or uses system Pi.
//! Ignored by default:
//! `cargo test --test rpc_live -- --ignored`

use pidesk_lib::rpc::{RpcClient, RpcHandlers};
use serde_json::{json, Map, Value};
use std::process::Stdio;
use std::sync::mpsc::channel;
use tokio::process::Command;

fn spawn(args: &[&str], cwd: &std::path::Path) -> (RpcClient, std::sync::mpsc::Receiver<Value>) {
    let home = std::env::var_os("HOME").expect("HOME");
    let root = std::path::PathBuf::from(&home).join(".pidesk");
    let bin = root.join("runtime/node_modules/.bin/pi");
    assert!(
        bin.is_file(),
        "Install private Pi through πDesk before running live tests."
    );
    let mut child = Command::new(bin)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("PI_CODING_AGENT_DIR", root.join("agent"))
        .env("PI_CODING_AGENT_SESSION_DIR", root.join("agent/sessions"))
        .env("PI_TELEMETRY", "0")
        .env("PI_SKIP_VERSION_CHECK", "1")
        .arg("--no-approve")
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
        },
    );
    (client, rx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn pi_state_and_messages() {
    let dir = std::env::temp_dir().join("pi-rpc-live");
    std::fs::create_dir_all(&dir).unwrap();
    let (client, rx) = spawn(&["--mode", "rpc", "--no-session"], &dir);
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
    drop(rx);
}

async fn smoke_prompt(args: &[&str]) {
    let dir = std::env::temp_dir().join(format!("pidesk-rpc-prompt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (client, events) = spawn(args, &dir);
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
        if event.get("type").and_then(Value::as_str) == Some("agent_settled") {
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
#[ignore = "sends one real model prompt through Pi"]
async fn pi_prompt_streams_reply() {
    smoke_prompt(&["--mode", "rpc", "--no-session", "--no-tools"]).await;
}
