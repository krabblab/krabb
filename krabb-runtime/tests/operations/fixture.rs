use krabb_core::types::{IpcRequest, IpcResponse};
use serde_json::{Value, json};

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, Command},
};

pub fn record_adapter() {
    use std::io::Write;
    let home = PathBuf::from(std::env::var_os("KRABB_HOME").unwrap());

    let mut file =
        fs::OpenOptions::new().create(true).append(true).open(home.join("adapters.jsonl")).unwrap();

    writeln!(file, "{}", std::process::id()).unwrap();
    file.sync_all().unwrap();
}

#[cfg(unix)]
fn install_binary(binary: &Path, role: &str) {
    use std::os::unix::fs::PermissionsExt;

    let executable = std::env::current_exe().unwrap();
    let quoted = executable.to_str().unwrap().replace('\'', "'\\''");

    fs::write(binary, format!("#!/bin/sh\nKRABB_OPERATION_ROLE={role} exec '{quoted}' \"$@\"\n"))
        .unwrap();

    fs::set_permissions(binary, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn install_binary(binary: &Path, _role: &str) {
    let executable = std::env::current_exe().unwrap();

    if fs::hard_link(&executable, binary).is_err() {
        fs::copy(&executable, binary).unwrap();
    }
}

pub struct Fixture {
    pub root: PathBuf,
    pub home: PathBuf,
    pub host: Option<Child>,
    pub backend: Option<Child>,
}

impl Fixture {
    pub async fn new(name: &str) -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let parent = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));

        fs::create_dir_all(&parent).unwrap();

        let root = parent.join(format!("krabb-operations-{name}-{}-{nonce}", std::process::id()));

        fs::create_dir(&root).unwrap();

        let mut fixture = Self { home: root.join("home"), root, host: None, backend: None };

        fixture.cli(&["init"]).await;

        fs::write(
            fixture.home.join("config.toml"),
            concat!(
                "provider='fixture'\nmodel='model'\napproval='auto'\n",
                "[clients.tui]\ntools=true\n",
            ),
        )
        .unwrap();

        for (id, manifest) in [
            ("runner", include_str!("../../../krabb-plugins/runner/krabb-plugin.toml").to_owned()),
            (
                "fixture",
                format!(
                    "id='fixture'\nversion='{}'\nprotocol={{major=0,minor=1}}\n\
                    capabilities=['model']\n",
                    env!("CARGO_PKG_VERSION")
                ),
            ),
        ] {
            let source = fixture.root.join(id);

            fs::create_dir_all(source.join("bin")).unwrap();
            fs::write(source.join("krabb-plugin.toml"), manifest).unwrap();

            let binary = source
                .join("bin")
                .join(format!("krabb-plugin-{id}{}", std::env::consts::EXE_SUFFIX));

            install_binary(&binary, id);

            fixture
                .cli(&[
                    "plugin",
                    "install",
                    id,
                    "--source",
                    source.to_str().unwrap(),
                    "--yes",
                    "--link",
                ])
                .await;
        }

        fixture.start_backend().await;
        fixture.start_host().await;
        fixture
    }

    pub fn command(&self, role: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());

        command
            .env_clear()
            .env("KRABB_HOME", &self.home)
            .env("KRABB_OPERATION_ROLE", role)
            .kill_on_drop(true);

        for name in
            ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP", "LLVM_PROFILE_FILE", "RUST_BACKTRACE"]
        {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }

        command
    }

    async fn cli(&self, args: &[&str]) {
        let output =
            tokio::time::timeout(Duration::from_secs(30), self.command("cli").args(args).output())
                .await
                .unwrap()
                .unwrap();

        assert!(
            output.status.success(),
            "CLI {args:?} failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub async fn start_backend(&mut self) {
        let mut command = self.command("backend");

        command.stdout(Stdio::null()).stderr(Stdio::inherit());
        self.backend = Some(command.spawn().unwrap());

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(url) = fs::read_to_string(self.home.join("backend.url"))
                    && TcpStream::connect(url.trim_start_matches("http://")).await.is_ok()
                {
                    break;
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Backend did not start.");
    }

    pub async fn start_host(&mut self) {
        let mut command = self.command("host");

        command
            .stdout(Stdio::null())
            .stderr(Stdio::from(fs::File::create(self.root.join("host.stderr")).unwrap()));

        self.host = Some(command.spawn().unwrap());

        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if self.rpc("status", json!({})).await.is_ok() {
                    break;
                }

                if let Some(status) = self.host.as_mut().unwrap().try_wait().unwrap() {
                    panic!(
                        "Host exited {status}: {}",
                        fs::read_to_string(self.root.join("host.stderr")).unwrap()
                    );
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Host did not start.");
    }

    pub async fn rpc(&self, method: &str, params: Value) -> io::Result<Value> {
        let port = fs::read_to_string(self.home.join("run/ipc.port"))?;
        let token = fs::read_to_string(self.home.join("run/ipc.token"))?;
        let stream = TcpStream::connect(format!("127.0.0.1:{}", port.trim())).await?;
        let (input, mut output) = stream.into_split();
        let request = IpcRequest::call(1, token.trim(), method, params);

        output.write_all(&serde_json::to_vec(&request)?).await?;
        output.write_all(b"\n").await?;

        let mut response = String::new();

        tokio::time::timeout(
            Duration::from_secs(5),
            BufReader::new(input).read_line(&mut response),
        )
        .await
        .map_err(io::Error::other)??;

        let response: IpcResponse = serde_json::from_str(&response)?;

        response.result.ok_or_else(|| {
            io::Error::other(response.error.map(|error| error.message).unwrap_or_default())
        })
    }

    pub async fn admit(&self) -> String {
        self.rpc("session.ensure", json!({"id":"conversation", "model":"model"})).await.unwrap();

        let receipt = self
            .rpc(
                "session.enqueue",
                json!({
                    "session":"conversation", "client":"tui", "plugin":"fixture",
                    "id":"request-1", "text":"Prepare the remote report with the orchid dataset."
                }),
            )
            .await
            .unwrap();

        receipt["task"].as_str().unwrap().into()
    }

    pub fn job(&self, id: &str) -> Value {
        let bytes = match fs::read(self.home.join("data/sessions/index.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Value::Null,
            Err(error) => panic!("Cannot observe task catalog: {error}"),
        };

        let index: Value = serde_json::from_slice(&bytes).unwrap();

        index["jobs"][id].clone()
    }

    pub async fn until(&self, id: &str, label: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        // Loaded Windows runners can take tens of seconds to restart a host and its plugins.
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let job = self.job(id);

                if predicate(&job) {
                    break job;
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Timed out waiting for {label}: {}\n{}",
                self.job(id),
                fs::read_to_string(self.root.join("host.stderr")).unwrap()
            )
        })
    }

    pub fn effects(&self, method: &str) -> usize {
        fs::read_to_string(self.home.join("backend/ledger.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| entry["method"] == method)
            .count()
    }

    pub fn adapter(&self) -> u32 {
        fs::read_to_string(self.home.join("adapters.jsonl"))
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .parse()
            .unwrap()
    }

    pub async fn kill(child: &mut Option<Child>) {
        if let Some(mut child) = child.take() {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
        }
    }

    pub async fn finish(mut self) {
        let shutdown = self.rpc("shutdown", json!({})).await;

        if let Some(mut host) = self.host.take() {
            let Ok(status) = tokio::time::timeout(Duration::from_secs(30), host.wait()).await
            else {
                panic!(
                    "Host did not stop after shutdown {shutdown:?}: {}",
                    fs::read_to_string(self.root.join("host.stderr")).unwrap_or_default()
                );
            };

            status.unwrap();
        }

        Self::kill(&mut self.backend).await;
        fs::remove_dir_all(&self.root).unwrap();
    }
}
