use super::*;

fn requirement(program: Option<&str>, setting: Option<&str>) -> Requirement {
    Requirement {
        program: program.map(str::to_owned),
        setting: setting.map(str::to_owned),
        help: "Install it.".into(),
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

    let multiline =
        Requirement { help: "Install it.\nThen retry.".into(), ..Requirement::default() };

    assert!(validate(&[Requirement { program: Some("docker".into()), ..multiline }]).is_err());
    assert!(validate(&vec![requirement(Some("docker"), None); REQUIREMENT_LIMIT + 1]).is_err());
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

    assert_eq!(found("signal-cli", path), Some(program.canonicalize().unwrap()));
    assert_eq!(found(program.to_str().unwrap(), ""), Some(program.canonicalize().unwrap()));
    assert_eq!(found("notes", path), None);
    assert_eq!(found("missing", path), None);
    assert_eq!(found("signal-cli", "relative"), None);

    let help = |requirements: &[Requirement], path: &str| lacking(&root, requirements, path);
    let signal = requirement(Some("signal-cli"), Some("KRABB_TEST_UNSET_COMMAND"));
    let sandbox = requirement(None, Some("KRABB_TEST_UNSET_RUNTIME"));

    assert_eq!(help(std::slice::from_ref(&signal), path), None);
    assert_eq!(help(std::slice::from_ref(&signal), ""), Some("Install it.".into()));
    assert_eq!(help(&[sandbox], ""), None, "an unset setting with no program requires nothing");

    let _ = std::fs::remove_dir_all(root);
}
