use krabb_core::{
    plugin::{Emitter, serve_events},
    types::{Capability, Hello, Protocol, Request, Response},
};

use serde_json::json;
use std::{fs, io::Write, path::PathBuf};

pub async fn serve() -> krabb_core::Result<()> {
    serve_events(
        Hello {
            formats: Vec::new(),
            protocol: Protocol::CURRENT,
            id: "fixture".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![Capability::Model],
            commands: Vec::new(),
        },
        |request, _: Emitter| async move {
            let Request::Call { id, method, params, .. } = request else { return Ok(None) };

            if method != "generate" {
                return Ok(None);
            }

            let home = PathBuf::from(std::env::var_os("KRABB_HOME").unwrap());
            let mut ledger = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(home.join("generations.jsonl"))?;
            writeln!(ledger, "{params}")?;
            ledger.sync_all()?;
            let count = fs::read_to_string(home.join("generations.jsonl"))?.lines().count();
            let result = if count == 1 {
                let card = fs::read_to_string(home.join("backend.url"))?;
                let mut arguments =
                    json!({"prompt":"Prepare the orchid dataset report.","card":card});

                if home.join("omit-card").exists() {
                    arguments.as_object_mut().unwrap().remove("card");
                }

                json!({"text":"", "stop":"tool", "events":[{"kind":"tool",
                "name":"operation_runner_delegate", "args":{
                    "arguments":arguments,
                    "message":"I will verify the external report and return here when it finishes."
                }}]})
            } else {
                assert_eq!(
                    count, 2,
                    "An accepted operation must not start another generation while waiting."
                );

                assert!(
                    params.to_string().contains(super::backend::RESULT),
                    "The external artifact was not retained."
                );
                json!({"text":super::backend::RESULT,"stop":"stop"})
            };

            Ok(Some(Response::ok(id, result)))
        },
    )
    .await
}
