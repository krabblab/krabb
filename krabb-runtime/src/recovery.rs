use std::sync::Arc;

use super::{Live, Plugins, Stop, retry_delay};

pub(super) async fn channel(
    plugins: &Plugins,
    id: &str,
    previous: &Arc<Live>,
    failures: &mut u32,
    stop: &Stop,
) -> bool {
    loop {
        *failures = failures.saturating_add(1);
        let delay = retry_delay(*failures);

        tracing::warn!(
            plugin = id,
            attempt = *failures,
            delay_seconds = delay.as_secs(),
            "Channel failed; retrying after backoff."
        );

        tokio::select! {
            biased;
            _ = stop.notified() => return false,

            _ = tokio::time::sleep(delay) => {}
        }

        let _lifecycle = tokio::select! {
            biased;
            _ = stop.notified() => return false,
            guard = plugins.lifecycle.read() => guard,
        };

        let Some(current) = plugins.get(id).await else {
            return true;
        };

        if !Arc::ptr_eq(previous, &current) {
            return true;
        }

        let result = tokio::select! {
            biased;
            _ = stop.notified() => return false,
            result = current.restart() => result,
        };

        match result {
            Ok(()) => {
                tracing::info!(plugin = id, "Channel restarted after a failure.");
                return true;
            }

            Err(error) => {
                tracing::warn!(plugin = id, error = %super::diagnostic(error.to_string()),
                    "Channel restart failed; retrying.");
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{Process, temp};

    #[tokio::test]
    async fn recovery_restarts_only_the_current_channel() {
        for mode in ["current", "removed", "replaced"] {
            let root = temp(&format!("channel-recovery-{mode}")).unwrap();

            std::fs::create_dir_all(&root).unwrap();

            let script = r#"import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
path.write_text(path.read_text() + 'start\n' if path.exists() else 'start\n')

for line in sys.stdin:
    r = json.loads(line)
    result = {'id': 'telegram', 'version': '0.1.0', 'protocol': {'major': 0, 'minor': 1}, 'capabilities': ['channel']} if r['method'] == 'hello' else {}
    print(json.dumps({'jsonrpc': '2.0', 'id': r['id'], 'result': result}), flush=True)
    if r['method'] == 'shutdown':
        break
"#;

            let marker = root.join("original");

            let process = Process::start_with("python3", ["-c", script, marker.to_str().unwrap()])
                .await
                .unwrap();

            let plugins = Plugins::default();

            plugins.insert(Live::new(process)).await;

            let original = plugins.get("telegram").await.unwrap();

            match mode {
                "removed" => {
                    plugins.remove("telegram").await.unwrap();
                }

                "replaced" => {
                    let marker = root.join("replacement");

                    let process =
                        Process::start_with("python3", ["-c", script, marker.to_str().unwrap()])
                            .await
                            .unwrap();

                    plugins.insert(Live::new(process)).await;
                }

                _ => {}
            }

            let mut failures = 0;

            assert!(channel(&plugins, "telegram", &original, &mut failures, &Stop::new()).await);
            assert_eq!(failures, 1);

            assert_eq!(
                std::fs::read_to_string(&marker).unwrap().lines().count(),
                if mode == "current" { 2 } else { 1 }
            );

            if mode == "replaced" {
                assert_eq!(std::fs::read_to_string(root.join("replacement")).unwrap(), "start\n");
            }

            original.stop().await.unwrap();

            for (_, plugin) in plugins.all().await {
                plugin.stop().await.unwrap();
            }

            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
