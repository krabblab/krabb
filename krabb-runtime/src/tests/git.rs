use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

pub(super) fn command() -> Command {
    let mut command = Command::new("git");

    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }

    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "core.hooksPath=/dev/null"]);

    command
}

fn run(root: &Path, args: &[&str]) -> String {
    let output = command().current_dir(root).args(args).output().unwrap();

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn fixtures_preserve_the_parent_worktree_during_commit_hooks() {
    let root = super::test_root("git-hook");
    let repo = root.join("repo");
    let worktree = root.join("worktree");
    let hooks = root.join("hooks");

    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&hooks).unwrap();

    run(&repo, &["init", "-q", "--initial-branch=main"]);
    run(&repo, &["config", "user.name", "Fixture Owner"]);
    run(&repo, &["config", "user.email", "owner@example.invalid"]);
    run(&repo, &["config", "commit.gpgsign", "false"]);

    fs::write(repo.join("file"), "original\n").unwrap();
    run(&repo, &["add", "file"]);
    run(&repo, &["commit", "-qm", "base"]);
    run(&repo, &["worktree", "add", "-b", "feature", worktree.to_str().unwrap()]);

    let before = run(&worktree, &["rev-parse", "HEAD"]);
    let config = fs::read(repo.join(".git/config")).unwrap();

    fs::write(worktree.join("file"), "staged change\n").unwrap();
    run(&worktree, &["add", "file"]);

    let tree = run(&worktree, &["write-tree"]);
    let executable = std::env::current_exe().unwrap();
    let executable = executable.to_str().unwrap().replace('\'', "'\\''");
    let hook = hooks.join("pre-commit");

    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nset -eu\ntest -n \"$GIT_DIR\"\n\
             '{executable}' --exact tests::resolves_git_sources_and_verified_archives\n\
             '{executable}' --exact tests::isolates_group_workspaces\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();

    run(
        &worktree,
        &["-c", &format!("core.hooksPath={}", hooks.display()), "commit", "-qm", "intended"],
    );

    assert_eq!(run(&worktree, &["rev-parse", "HEAD^"]), before);
    assert_eq!(run(&repo, &["rev-parse", "HEAD"]), before);
    assert_eq!(run(&worktree, &["rev-parse", "HEAD^{tree}"]), tree);
    assert_eq!(run(&worktree, &["write-tree"]), tree);
    assert_eq!(run(&worktree, &["branch", "--show-current"]), "feature");
    assert_eq!(run(&worktree, &["status", "--porcelain"]), "");
    assert_eq!(fs::read(repo.join(".git/config")).unwrap(), config);
    assert_eq!(fs::read_to_string(worktree.join("file")).unwrap(), "staged change\n");

    fs::remove_dir_all(root).unwrap();
}
