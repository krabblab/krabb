use super::*;

fn requirement(program: Option<&str>, setting: Option<&str>) -> Requirement {
    Requirement {
        program: program.map(str::to_owned),
        setting: setting.map(str::to_owned),
        help: "Install it.".into(),
        ..Requirement::default()
    }
}

#[test]
fn declarations_name_a_plain_program_or_a_krabb_setting_with_one_line_of_help() {
    assert!(validate(&[requirement(Some("signal-cli"), Some("KRABB_SIGNAL_COMMAND"))]).is_ok());

    assert!(validate(&[requirement(None, Some("KRABB_SANDBOX_RUNTIME"))]).is_ok());
    assert!(validate(&[requirement(None, None)]).is_err());
    assert!(validate(&[requirement(Some("../bin/sh"), None)]).is_err());
    assert!(validate(&[requirement(Some(".hidden"), None)]).is_err());
    assert!(validate(&[requirement(Some("docker"), Some("PATH"))]).is_err());
    assert!(validate(&vec![requirement(Some("docker"), None); REQUIREMENT_LIMIT + 1]).is_err());

    let off = Requirement { off: vec!["off".into()], ..requirement(None, Some("KRABB_RUNTIME")) };

    assert!(validate(std::slice::from_ref(&off)).is_ok());
    assert!(validate(&[Requirement { off: vec!["no way".into()], ..off }]).is_err());

    for help in ["Install it.\nThen retry.", "Install \u{1b}[2Jit.", "Install \u{202e}it.", " "] {
        let declared = Requirement { help: help.into(), ..requirement(Some("docker"), None) };

        assert!(validate(&[declared]).is_err(), "{help:?}");
    }
}

#[cfg(unix)]
#[test]
fn programs_are_found_only_as_executable_files_in_absolute_path_entries() {
    use std::os::unix::fs::PermissionsExt;

    let root = crate::tests::test_root("requirement-path");
    let bin = root.join("bin");
    let program = bin.join("signal-cli");
    let plain = bin.join("notes");

    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(&plain, "text").unwrap();

    let path = std::env::join_paths([Path::new("relative"), &bin]).unwrap();
    let path = path.to_str().unwrap();

    assert_eq!(locate("signal-cli", path), Some(program.canonicalize().unwrap()));
    assert_eq!(locate(program.to_str().unwrap(), ""), Some(program.canonicalize().unwrap()));
    assert_eq!(locate("bin/signal-cli", path), None, "a relative path depends on the cwd");
    assert_eq!(locate("notes", path), None);
    assert_eq!(locate("missing", path), None);
    assert_eq!(locate("signal-cli", "relative"), None);

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn a_setting_overrides_the_program_and_its_off_values_need_none() {
    use std::os::unix::fs::PermissionsExt;

    let root = crate::tests::test_root("requirement-setting");
    let program = root.join("docker");

    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&program, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path = root.to_str().unwrap();
    let signal = [requirement(Some("signal-cli"), Some("KRABB_SIGNAL_COMMAND"))];

    let sandbox = [Requirement {
        off: vec!["off".into()],
        ..requirement(None, Some("KRABB_SANDBOX_RUNTIME"))
    }];

    let set = |value: &'static str| move |_: &str| Some(value.to_owned());
    let unset = |_: &str| None;

    assert_eq!(unmet(&signal, path, unset), Some("Install it.".into()));
    assert_eq!(unmet(&signal, path, set("docker")), None, "the setting names another program");
    assert_eq!(unmet(&sandbox, path, unset), None, "an unset setting with no program needs none");
    assert_eq!(unmet(&sandbox, path, set("off")), None);
    assert_eq!(unmet(&sandbox, path, set("docker")), None);
    assert_eq!(unmet(&sandbox, path, set("podman")), Some("Install it.".into()));

    let _ = std::fs::remove_dir_all(root);
}
