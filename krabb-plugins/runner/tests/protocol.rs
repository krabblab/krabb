use krabb_core::{
    operation::Spec,
    types::{Capability, Hello},
};

use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[tokio::test]
async fn runner_negotiates_only_supported_operations_and_rejects_invalid_work_before_dispatch() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_krabb-plugin-runner"));

    command
        .env_clear()
        .env("KRABB_HOME", env!("CARGO_TARGET_TMPDIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    for name in ["LLVM_PROFILE_FILE", "SystemRoot", "WINDIR", "PATH"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }

    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();

    for (id, method, params) in [
        (1, "hello", json!({})),
        (2, "operation.list", json!({})),
        (3, "operation.start", json!({"id":"start-3", "name":"unsupported", "arguments":{}})),
        (
            4,
            "operation.start",
            json!({"id":"start-4", "name":"delegate", "arguments":{"prompt":""}}),
        ),
        (
            5,
            "operation.start",
            json!({
                "id":"start-5", "name":"delegate", "arguments":{"prompt":"Prepare the report."}
            }),
        ),
        (6, "operation.inspect", json!({"name":"delegate", "handle":"invalid"})),
        (7, "command", json!({"name":"a2a", "args":[]})),
        (8, "shutdown", json!({})),
    ] {
        let request = json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params});

        input.write_all(format!("{request}\n").as_bytes()).await.unwrap();

        let line = tokio::time::timeout(Duration::from_secs(5), output.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        let response: Value = serde_json::from_str(&line).unwrap();

        assert_eq!(response["id"], id);

        match id {
            1 => {
                let hello: Hello = serde_json::from_value(response["result"].clone()).unwrap();

                assert_eq!(hello.id, "runner");
                assert_eq!(hello.capabilities, vec![Capability::Runner]);
                assert!(hello.commands.is_empty());
            }

            2 => {
                let declarations: Vec<Spec> =
                    serde_json::from_value(response["result"]["operations"].clone()).unwrap();

                assert_eq!(declarations.len(), 1);

                let operation = &declarations[0];

                operation.validate().unwrap();

                assert_eq!(operation.name, "delegate");
                assert!(operation.cancel);
            }

            3..=6 => assert!(response["error"].is_object()),
            7 => assert_eq!(response["error"]["code"], -32601),
            _ => assert!(response["error"].is_null()),
        }
    }

    drop(input);

    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );

    assert!(output.next_line().await.unwrap().is_none());
}
