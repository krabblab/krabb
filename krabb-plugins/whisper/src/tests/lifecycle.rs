use crate::*;
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    pid: Option<u32>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take()
            && PathBuf::from(format!("/proc/{pid}")).exists()
        {
            let _ = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status();
        }

        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn capture_reaps_timeout_and_overflow_children_before_returning() {
    let mut fixture = Fixture {
        root: std::env::temp_dir().join(format!("krabb-whisper-lifecycle-{}", std::process::id())),
        pid: None,
    };

    fs::create_dir_all(&fixture.root).unwrap();

    let source = fixture.root.join("transcriber.rs");

    fs::write(
        &source,
        r#"
use std::{fs, io::Write, thread, time::{Duration, Instant}};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    fs::write("pid.pending", std::process::id().to_string()).unwrap();
    fs::rename("pid.pending", "pid").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !std::path::Path::new("go").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let bytes = vec![b'x'; args[2].parse::<usize>().unwrap()];
    match args[1].as_str() {
        "stdout" => { let _ = std::io::stdout().write_all(&bytes); }
        "stderr" => { let _ = std::io::stderr().write_all(&bytes); }
        _ => {}
    }
    while Instant::now() < deadline {
        thread::park_timeout(Duration::from_millis(10));
    }
}
"#,
    )
    .unwrap();

    let executable = fixture.root.join("transcriber");

    let compiled = std::process::Command::new("rustc")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();

    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));

    for mode in ["timeout", "stdout", "stderr"] {
        let mut command = Command::new(&executable);

        command.current_dir(&fixture.root).arg(mode).arg((OUTPUT_LIMIT + 1).to_string());

        let running = tokio::spawn(capture(command));

        timeout(Duration::from_secs(5), async {
            while !fixture.root.join("pid").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("transcriber readiness deadline");

        let pid = fs::read_to_string(fixture.root.join("pid")).unwrap().parse::<u32>().unwrap();

        fixture.pid = Some(pid);

        assert!(PathBuf::from(format!("/proc/{pid}")).exists());

        let error = if mode == "timeout" {
            tokio::time::pause();
            tokio::time::advance(COMMAND_LIMIT).await;
            let result = running.await.unwrap().unwrap_err();
            tokio::time::resume();
            result
        } else {
            fs::write(fixture.root.join("go"), []).unwrap();
            timeout(Duration::from_secs(5), running)
                .await
                .expect("output budget must fail before the execution deadline")
                .unwrap()
                .unwrap_err()
        };

        assert_eq!(
            error.kind(),
            if mode == "timeout" {
                std::io::ErrorKind::TimedOut
            } else {
                std::io::ErrorKind::FileTooLarge
            }
        );

        assert!(!PathBuf::from(format!("/proc/{pid}")).exists(), "{mode}: child not reaped");

        fixture.pid = None;
        fs::remove_file(fixture.root.join("pid")).unwrap();

        let _ = fs::remove_file(fixture.root.join("go"));

        let mut recovery = Command::new("sh");

        recovery.args(["-c", "printf 'recovered transcription'"]);

        let output = capture(recovery).await.unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"recovered transcription");
    }
}
