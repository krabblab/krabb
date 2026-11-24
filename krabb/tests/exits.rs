//! Pins the exit codes documented in krabb-docs/commands.md.

use std::process::{Command, Output, Stdio};

fn krabb(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_krabb"))
        .args(args)
        .env("KRABB_HOME", home)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn documented_exit_codes() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();

    let home = std::env::temp_dir().join(format!(
        "krabb-exits-{}-{}",
        std::process::id(),
        nanos.as_nanos()
    ));

    std::fs::create_dir_all(&home).unwrap();

    assert_eq!(krabb(&home, &["--version"]).status.code(), Some(0));
    assert_eq!(krabb(&home, &["--no-such-option"]).status.code(), Some(2));

    let rollback = krabb(&home, &["update", "--rollback", "--yes"]);

    assert_eq!(rollback.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rollback.stderr).contains("nothing to roll back"));

    let prompt = krabb(&home, &["init", "--force"]);

    assert_eq!(prompt.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&prompt.stderr).contains("Use --yes"));

    let _ = std::fs::remove_dir_all(home);
}
