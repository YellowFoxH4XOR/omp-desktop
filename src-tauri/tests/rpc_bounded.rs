use omp_desktop_lib::rpc::{RpcClient, RpcHandlers};
use serde_json::Map;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

#[cfg(unix)]
fn attach_shell(script: &str, on_exit: RpcHandlers) -> RpcClient {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn test harness");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    RpcClient::attach(child, stdout, stderr, on_exit)
}

fn noop_handlers() -> RpcHandlers {
    RpcHandlers {
        on_event: Box::new(|_| {}),
        on_exit: Box::new(|_, _, _| {}),
        on_ready: None,
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_request_table_is_bounded() {
    let client = Arc::new(attach_shell("exec cat >/dev/null", noop_handlers()));
    let mut calls = Vec::new();
    for _ in 0..257 {
        let client = client.clone();
        calls.push(tokio::spawn(async move {
            client
                .call_with_timeout("get_state", Map::new(), 2)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }));
    }

    let mut rejected_for_capacity = 0;
    for call in calls {
        let error = call
            .await
            .expect("request task should finish")
            .expect_err("unanswered request should fail");
        if error.contains("Too many harness requests are still pending") {
            rejected_for_capacity += 1;
        }
    }

    assert_eq!(
        rejected_for_capacity, 1,
        "exactly the 257th concurrent request should exceed the 256-entry pending cap"
    );
    client.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_wire_line_terminates_harness_and_fails_closed() {
    let script = "head -c 1048577 /dev/zero | tr '\\0' x; printf '\\n'; exec sleep 30";
    let client = attach_shell(script, noop_handlers());

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !client.is_exited() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(client.is_exited(), "oversized wire input must fail closed");
    let error = client
        .call("get_state", Map::new())
        .await
        .expect_err("terminated harness must reject later calls")
        .to_string();
    assert!(error.contains("exited"));
}
