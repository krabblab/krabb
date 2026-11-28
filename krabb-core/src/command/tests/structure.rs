use crate::command::{
    Action, Dialect, Hazard, Os, classify, device, host, parse, share_host, system, url_host,
};

#[test]
fn parse_keeps_programs_arguments_and_targets_even_when_unsafe() {
    let parsed = parse("sudo -u admin rm -rf /tmp/x > log.txt; echo $(whoami) &", Dialect::Sh);

    assert!(parsed.hazards.contains(&Hazard::Substitution));
    assert!(parsed.hazards.contains(&Hazard::Background));

    let programs: Vec<&str> = parsed.segments.iter().map(|s| s.program.as_str()).collect();

    assert_eq!(programs, ["rm", "echo", "whoami"]);

    let remove = &parsed.segments[0];

    assert!(remove.elevated);
    assert_eq!(remove.wrappers, ["sudo"]);
    assert_eq!(remove.args, ["-rf", "/tmp/x"]);
    assert_eq!(remove.outputs, ["log.txt"]);
    assert_eq!(remove.operands(), ["/tmp/x"]);
    assert_eq!(remove.depth, 0);

    let discarded = parse("ls > /dev/null < in.txt", Dialect::Sh);

    assert!(discarded.segments[0].outputs.is_empty());
    assert_eq!(discarded.segments[0].inputs, ["in.txt"]);
}

#[test]
fn parse_reads_nested_commands_in_their_own_dialect() {
    let parsed = parse("cmd /c \"del /q C:\\x\"", Dialect::Cmd);
    let inner = &parsed.segments[1];

    assert_eq!((inner.program.as_str(), inner.dialect, inner.depth), ("del", Dialect::Cmd, 1));
    assert_eq!(inner.operands(), ["C:\\x"]);

    let parsed = parse("powershell -Command \"Remove-Item x\"", Dialect::Cmd);

    assert_eq!(parsed.segments[1].dialect, Dialect::PowerShell);

    let parsed = parse("find . -exec rm -f {} ;", Dialect::Sh);

    assert_eq!(parsed.segments[1].program, "rm");
    assert_eq!(parsed.segments[1].args, ["-f", "{}"]);

    let parsed = parse("runas /user:admin \"del x\"", Dialect::Cmd);

    assert!(parsed.segments[1].elevated);
}

#[test]
fn wrappers_unwrap_to_the_program_they_run() {
    for (command, program, wrappers) in [
        ("nohup nice -n 5 ls", "ls", vec!["nohup", "nice"]),
        ("timeout -s KILL 5 cat x", "cat", vec!["timeout"]),
        ("env -u HOME FOO=1 ls", "ls", vec!["env"]),
        ("time -p command ls", "ls", vec!["time", "command"]),
        ("doas -u root ls", "ls", vec!["doas"]),
        ("sudo -- ls", "ls", vec!["sudo"]),
        ("sudo", "sudo", vec![]),
        ("env", "env", vec![]),
        ("timeout 5", "timeout", vec![]),
    ] {
        let parsed = parse(command, Dialect::Sh);

        assert_eq!(parsed.segments[0].program, program, "{command}");
        assert_eq!(parsed.segments[0].wrappers, wrappers, "{command}");
    }
}

#[test]
fn packages_name_their_manager_action_and_scope() {
    let packages =
        classify("sudo apt-get install -y ripgrep fd-find", Dialect::Sh).packages.unwrap();

    assert_eq!(packages.manager, "apt-get");
    assert_eq!(packages.action, Action::Install);
    assert!(packages.system);
    assert_eq!(packages.names, ["ripgrep", "fd-find"]);

    let packages = classify("brew info jq", Dialect::Sh).packages.unwrap();

    assert_eq!((packages.action, packages.system), (Action::Query, false));

    let packages = classify("msiexec /x app.msi", Dialect::Cmd).packages.unwrap();

    assert_eq!((packages.action, packages.names), (Action::Remove, vec!["app.msi".to_owned()]));

    let packages = classify("rpm -e x", Dialect::Sh).packages.unwrap();

    assert_eq!(packages.action, Action::Remove);
    assert!(classify("pacman -Zz", Dialect::Sh).packages.is_none());
    assert!(classify("apt frobnicate", Dialect::Sh).packages.is_none());
}

#[test]
fn hosts_are_recognized_from_urls_and_shares() {
    assert_eq!(url_host("https://user@example.com:8080/x?y"), Some("example.com"));
    assert_eq!(url_host("ftp://files.example.com"), Some("files.example.com"));
    assert_eq!(url_host("file:///etc/passwd"), None);
    assert_eq!(url_host("not a url://x"), None);
    assert_eq!(share_host("\\\\server\\share"), Some("server"));
    assert_eq!(share_host("//server/share"), Some("server"));
    assert_eq!(share_host("\\\\?\\UNC\\server\\share"), Some("server"));
    assert_eq!(share_host("\\\\?\\C:\\x"), None);
    assert_eq!(share_host("\\\\.\\pipe\\x"), None);
    assert_eq!(share_host("FileSystem::\\\\server\\share"), Some("server"));
    assert_eq!(share_host("C:\\x"), None);
    assert_eq!(share_host("\\\\"), None);
    assert_eq!(host("https://a.example"), Some("a.example"));
    assert_eq!(host("\\\\b\\c"), Some("b"));
    assert_eq!(host("relative/path"), None);
}

#[test]
fn devices_other_than_the_harmless_ones_are_recognized() {
    for path in ["/dev/tcp/h/80", "/dev/udp/h/53", "/dev/fd/3", "/dev/sda", "\\\\.\\COM1"] {
        assert!(device(path), "{path}");
    }

    for path in ["/dev/null", "/dev/stdin", "/dev/zero", "/devices", "dev/tcp"] {
        assert!(!device(path), "{path}");
    }
}

#[test]
fn system_paths_follow_each_operating_system() {
    for (path, os) in [
        ("/etc/hosts", Os::Linux),
        ("/usr", Os::Linux),
        ("/proc/1/mem", Os::Linux),
        ("/System/Library", Os::MacOs),
        ("/private/etc/hosts", Os::MacOs),
        ("C:\\Windows\\System32", Os::Windows),
        ("c:/program files/app", Os::Windows),
        ("D:\\Program Files (x86)\\x", Os::Windows),
        ("\\ProgramData\\x", Os::Windows),
        ("%SystemRoot%\\system32", Os::Windows),
        ("$env:ProgramFiles\\x", Os::Windows),
        ("\\\\?\\C:\\Windows\\x", Os::Windows),
    ] {
        assert!(system(path, os), "{path} {os:?}");
    }

    for (path, os) in [
        ("/etcetera", Os::Linux),
        ("/home/me/etc", Os::Linux),
        ("relative/usr", Os::Linux),
        ("/System", Os::Linux),
        ("/Users/me", Os::MacOs),
        ("C:\\Users\\me", Os::Windows),
        ("C:\\Windowsill", Os::Windows),
        ("\\\\server\\Windows", Os::Windows),
        ("C:", Os::Windows),
    ] {
        assert!(!system(path, os), "{path} {os:?}");
    }
}

#[test]
fn every_reason_reads_as_a_sentence() {
    for command in [
        "ls `x`",
        "cat <<EOF",
        "a;;b",
        "$[1]",
        "$'x'",
        "\"x",
        "x\\",
        ";",
        "ls >",
        "ls <&x",
        "(x)",
        "x &",
        "<(x)",
        "#",
    ] {
        for reason in classify(command, Dialect::Sh).reasons {
            let text = reason.to_string();

            assert!(text.starts_with(|c: char| c.is_lowercase() || c == '`'), "{command}: {text}");
        }
    }
}
