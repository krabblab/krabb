use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run one private test in a process with its own home and no inherited credentials.
pub async fn isolated(test: &str, environment: &[(&str, &str)]) -> bool {
    if std::env::var("KRABB_INBOX_FIXTURE").as_deref() == Ok(test) {
        return true;
    }

    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();

    let home =
        Home(std::env::temp_dir().join(format!("krabb-inbox-{}-{nonce}", std::process::id())));

    fs::create_dir(&home.0).unwrap();

    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());

    command
        .args(["--exact", test, "--nocapture"])
        .env_clear()
        .env("KRABB_HOME", &home.0)
        .env("KRABB_INBOX_FIXTURE", test)
        .envs(environment.iter().copied())
        .kill_on_drop(true);

    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }

    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("inbox subprocess timed out")
        .unwrap();

    assert!(
        output.status.success(),
        "inbox subprocess failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    false
}

/// Preserve a durable snapshot while a nonempty directory rejects replacement writes.
pub struct Blocked {
    path: PathBuf,
    saved: PathBuf,
}

impl Blocked {
    pub fn new(path: &Path) -> Self {
        let saved = path.with_extension("saved");

        fs::rename(path, &saved).unwrap();
        fs::create_dir(path).unwrap();
        fs::write(path.join("blocker"), b"block replacement").unwrap();
        Self { path: path.to_owned(), saved }
    }

    pub fn snapshot(&self) -> Vec<u8> {
        fs::read(&self.saved).unwrap()
    }
}

impl Drop for Blocked {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
        fs::rename(&self.saved, &self.path).unwrap();
    }
}
