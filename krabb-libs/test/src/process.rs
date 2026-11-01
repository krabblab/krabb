use tokio::process::Command;

pub(crate) fn command(test: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());

    command.args(["--exact", test, "--nocapture"]).env_clear().kill_on_drop(true);

    // Winsock and temporary directories depend on these Windows runtime settings.
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }

    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }

    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn isolated_child_can_use_platform_services() {
        if std::env::var_os("KRABB_PROCESS_FIXTURE").is_some() {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let root = std::env::temp_dir().join(format!("krabb-child-{}", std::process::id()));

            std::fs::create_dir(&root).unwrap();

            assert!(listener.local_addr().unwrap().ip().is_loopback());

            std::fs::remove_dir(root).unwrap();
            return;
        }

        let mut command = command("process::tests::isolated_child_can_use_platform_services");
        command.env("KRABB_PROCESS_FIXTURE", "1");

        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .expect("isolated child timed out")
            .unwrap();

        assert!(
            output.status.success(),
            "isolated child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
