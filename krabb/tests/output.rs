//! Pins that redirected and `--json` output carry no terminal controls or decoration.

use std::process::{Command, Output, Stdio};

fn krabb(user: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_krabb"))
        .args(args)
        .env("KRABB_HOME", user.join(".krabb"))
        .env("HOME", user)
        .env("USERPROFILE", user)
        .env("XDG_CONFIG_HOME", user.join(".config"))
        .env_remove("NO_COLOR")
        .env("TERM", "xterm-256color")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// How many JSON values TEXT holds, or `None` when anything else is in it.
fn documents(text: &str) -> Option<usize> {
    let mut count = 0;

    for value in serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>() {
        value.ok()?;
        count += 1;
    }

    Some(count)
}

#[test]
fn redirected_output_is_plain_and_json_is_one_document() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();

    let user = std::env::temp_dir().join(format!(
        "krabb-output-{}-{}",
        std::process::id(),
        nanos.as_nanos()
    ));

    std::fs::create_dir_all(&user).unwrap();

    let help = krabb(&user, &["--help"]);
    let text = String::from_utf8_lossy(&help.stdout);

    assert!(help.status.success());
    assert!(text.starts_with("A self-hostable"), "{text}");
    assert!(!text.contains('█') && !text.contains('\x1b'), "{text}");

    for args in [
        &["--json", "init"][..],
        &["--json", "status"],
        &["--json", "doctor"],
        &["--json", "plugin", "list"],
        &["--json", "service", "status"],
        &["--json", "service", "uninstall", "--yes"],
        &["--json", "ask", "hello"],
    ] {
        let output = krabb(&user, args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let report = if output.status.success() { &stdout } else { &stderr };

        assert_eq!(documents(report), Some(1), "{args:?} printed {report}");
        assert!(!stdout.contains('\x1b') && !stderr.contains('\x1b'), "{args:?}: {stderr}");
    }

    for args in [&["status"][..], &["doctor"], &["plugin", "list"]] {
        let output = krabb(&user, args);

        assert!(!String::from_utf8_lossy(&output.stdout).contains('\x1b'), "{args:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains('\x1b'), "{args:?}");
    }

    let _ = std::fs::remove_dir_all(user);
}

#[test]
fn json_usage_errors_are_one_document_with_exit_code_two() {
    let user = std::env::temp_dir().join(format!("krabb-usage-{}", std::process::id()));

    std::fs::create_dir_all(&user).unwrap();

    for args in [
        &["--json", "status", "--nope"][..],
        &["--json", "plugin", "install"],
        &["--json", "service"],
    ] {
        let output = krabb(&user, args);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(documents(&stderr), Some(1), "{args:?} printed {stderr}");
        assert!(stderr.contains("\"error\""), "{stderr}");
    }

    for args in [&["status", "--nope"][..], &["ask", "--nope", "--", "--json"]] {
        let plain = krabb(&user, args);

        assert_eq!(plain.status.code(), Some(2), "{args:?}");
        assert_eq!(documents(&String::from_utf8_lossy(&plain.stderr)), None, "{args:?}");
    }

    let _ = std::fs::remove_dir_all(user);
}
