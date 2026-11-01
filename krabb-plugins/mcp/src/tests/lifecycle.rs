use crate::*;
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    running: tokio::task::JoinHandle<krabb_core::Result<Value>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(pid) = fs::read_to_string(self.root.join("pid"))
            && let Ok(pid) = pid.parse::<u32>()
            && PathBuf::from(format!("/proc/{pid}")).exists()
        {
            let _ = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status();
        }

        self.running.abort();

        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn lifecycle(mode: &str) {
    let root = std::env::temp_dir().join(format!(
        "krabb-mcp-{mode}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));

    fs::create_dir(&root).unwrap();

    let script = r#"
set -eu
cd "$1"
printf '%s' "$$" > pid.pending
mv pid.pending pid
read -r initialize
case "$2" in
    timeout) touch ready; exec sleep 60 ;;
    malformed) printf '%s\n' 'malformed'; exec sleep 60 ;;
esac
printf '%s\n' '{"jsonrpc":"2.0","id":"krabb-initialize","result":{"protocolVersion":"2025-06-18"}}'
read -r initialized
read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"survives-shutdown"}]}}'
if read -r unexpected; then exit 9; fi
touch ready
exec sleep 60
"#;

    let params = json!({
        "approved": true,
        "command": "sh", "args": ["-c", script, "fixture", root, mode],
        "request": {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}
    });

    let running = tokio::spawn(async move { stdio(&params).await });

    let mut fixture = Fixture { root, running };
    let marker = if mode == "malformed" { "pid" } else { "ready" };

    timeout(Duration::from_secs(5), async {
        while !fixture.root.join(marker).exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("child must publish readiness");

    let pid = fs::read_to_string(fixture.root.join("pid")).unwrap().parse::<u32>().unwrap();
    let process = PathBuf::from(format!("/proc/{pid}"));

    if mode != "malformed" {
        assert!(process.exists(), "fixture must remain alive until the deadline");
        assert!(!fixture.running.is_finished(), "call must wait for response or child exit");

        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(if mode == "timeout" { 30 } else { 2 })).await;
        tokio::time::resume();
    }

    let result = timeout(Duration::from_secs(5), &mut fixture.running)
        .await
        .expect("stdio must return after its deadline and reap its child")
        .unwrap();

    // /proc includes zombies: disappearance requires termination AND reaping before return.

    assert!(!process.exists(), "{mode}: direct child still alive or unreaped after return");

    match mode {
        "timeout" => assert!(result.unwrap_err().to_string().contains("MCP process timed out")),
        "malformed" => assert!(matches!(result, Err(krabb_core::Error::Json(_)))),
        _ => assert_eq!(
            result.unwrap(),
            json!({"jsonrpc":"2.0", "id":2,
                "result":{"tools":[{"name":"survives-shutdown"}]}})
        ),
    }

    let recovered = timeout(
        Duration::from_secs(5),
        stdio(&json!({
            "approved": true, "command": "sh", "args": ["-c", r#"
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18"}}'
read -r line
"#],

            "request": {"jsonrpc":"2.0", "id":1, "method":"initialize"}
        })),
    )
    .await
    .expect("subsequent request must complete")
    .unwrap();

    assert_eq!(recovered["result"]["protocolVersion"], VERSION);
}

#[tokio::test]
async fn stdio_deadline_reaps_stalled_initialization() {
    lifecycle("timeout").await;
}

#[tokio::test]
async fn stdio_protocol_error_reaps_live_child() {
    lifecycle("malformed").await;
}

#[tokio::test]
async fn stdio_preserves_response_when_child_refuses_to_exit() {
    lifecycle("response").await;
}
