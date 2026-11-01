use std::{fs, path::PathBuf, process::Command, time::Duration};

pub struct Repository(pub PathBuf);

impl Repository {
    pub fn new(name: &str, worker: &str, wait: bool) -> Self {
        let root = std::env::temp_dir().join(format!("krabb-git-{name}-{}", std::process::id()));

        fs::create_dir(&root).unwrap();

        let repo = Self(root);

        repo.git(&["init"]);
        fs::write(repo.0.join("note"), "before\n").unwrap();
        repo.git(&["add", "note"]);
        fs::write(repo.0.join(".gitattributes"), "note filter=probe\n").unwrap();
        fs::write(repo.0.join("note"), "after\n").unwrap();

        let executable = std::env::current_exe().unwrap();
        let executable = executable.to_str().unwrap().replace('\\', "/").replace('\'', "'\\''");
        let finish = if wait { "wait \"$worker\"" } else { "cat" };

        fs::write(
            repo.0.join("filter.sh"),
            format!(
                r#"KRABB_TEST_GIT_WORKER=1 '{executable}' --exact '{worker}' --ignored \
    --nocapture </dev/null >/dev/null 2>&1 &
worker=$!
for attempt in $(seq 1 500); do
    if test -f ready; then {finish}; exit $?; fi
    sleep 0.01
done
exit 1
"#
            ),
        )
        .unwrap();

        repo.git(&["config", "filter.probe.clean", "sh filter.sh"]);
        repo.git(&["config", "filter.probe.required", "true"]);

        repo
    }

    fn git(&self, args: &[&str]) {
        let mut command = Command::new("git");

        command.env_clear().env("PATH", std::env::var_os("PATH").unwrap_or_default());

        if let Some(system_root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system_root);
        }

        let output = command
            .env("HOME", &self.0)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.0.join("absent-config"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();

        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    pub async fn assert_stopped(&self) {
        tokio::time::sleep(Duration::from_millis(2200)).await;

        assert!(!self.0.join("leaked").exists(), "Git descendant wrote after cleanup");

        #[cfg(target_os = "linux")]

        assert_eq!(fs::read_to_string(self.0.join("ready")).unwrap(), "true true");
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn worker() {
    if std::env::var_os("KRABB_TEST_GIT_WORKER").is_none() {
        return;
    }

    #[cfg(target_os = "linux")]
    let ready = format!(
        "{} {}",
        rustix::process::setsid() == Err(rustix::io::Errno::PERM),
        rustix::process::setpgid(None, None) == Err(rustix::io::Errno::PERM),
    );

    #[cfg(windows)]
    let ready = "ready";

    fs::write("ready", ready).unwrap();
    std::thread::sleep(Duration::from_secs(2));

    let _ = fs::write("leaked", "leaked");
}
