use super::*;
use crate::{ChannelConfig, Cli, Config};
use clap::Parser;
use std::fs;

fn options(yes: bool) -> Command {
    Command::Setup(Options {
        channel: Channel { channel: "discord".into(), force: false, client: false },
        yes,
    })
}

fn root(name: &str) -> PathBuf {
    let root = crate::tests::test_root(name);

    crate::init_at(&root).unwrap();
    root
}

#[tokio::test]
async fn guided_setup_confirms_initialization_and_preserves_files() {
    let home = root("worktree-setup");
    let workspace = krabb_file::directory(&home, Path::new("data/workspaces/repository")).unwrap();
    let config = fs::read(home.join("config.toml")).unwrap();
    let instructions = fs::read(home.join("data/system/KRAB.md")).unwrap();

    run_with(options(false), &home, false, |yes, json, question| {
        assert!(!yes && !json);
        assert!(question.contains("empty initial commit"));
        assert!(question.contains("discord"));
        assert!(question.contains(workspace.to_str().unwrap()));
        Ok(false)
    })
    .await
    .unwrap();

    assert!(!workspace.join(".git").exists());
    assert_eq!(fs::read(home.join("config.toml")).unwrap(), config);

    run_with(options(false), &home, false, |_, _, _| Ok(true)).await.unwrap();

    assert_eq!(fs::read(home.join("data/system/KRAB.md")).unwrap(), instructions);
    assert_eq!(git(&workspace, &["ls-tree", "HEAD"]).unwrap(), "");
    assert!(crate::load_config_at(&home).unwrap().channels["discord"].worktree);

    let checkout = crate::isolate_at("discord-123", &home).unwrap().unwrap();

    assert!(checkout.join(".git").is_file());

    assert_eq!(
        git(&checkout, &["rev-parse", "HEAD"]).unwrap(),
        git(&workspace, &["rev-parse", "HEAD"]).unwrap()
    );

    assert_eq!(crate::isolate_at("discord-123", &home).unwrap().unwrap(), checkout);

    let head = git(&workspace, &["rev-parse", "HEAD"]).unwrap();

    run_with(options(true), &home, true, |yes, json, question| {
        assert!(yes && json);
        assert!(question.contains("Existing commits"));
        Ok(true)
    })
    .await
    .unwrap();

    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]).unwrap(), head);

    run_with(options(true), &home, true, crate::confirm_action).await.unwrap();

    run(
        Command::Disable(Channel { channel: "discord".into(), force: false, client: false }),
        &home,
        true,
    )
    .await
    .unwrap();

    assert!(!crate::load_config_at(&home).unwrap().channels["discord"].worktree);
    assert!(home.join("worktrees/discord-123").exists());

    fs::remove_dir_all(home).unwrap();
}

#[cfg(windows)]
#[test]
fn git_paths_use_drive_and_unc_paths_without_verbatim_prefixes() {
    for (path, expected) in [
        (r"\\?\C:\home\worktrees\chat", r"C:\home\worktrees\chat"),
        (r"\\?\UNC\server\share\worktrees\chat", r"\\server\share\worktrees\chat"),
        (r"C:\home\worktrees\chat", r"C:\home\worktrees\chat"),
        (r"\\server\share\worktrees\chat", r"\\server\share\worktrees\chat"),
        (r"worktrees\chat", r"worktrees\chat"),
    ] {
        assert_eq!(git_path(Path::new(path)), Path::new(expected));
    }
}

#[test]
fn initial_commit_does_not_capture_staged_or_untracked_files() {
    let home = root("worktree-index");

    let workspace = krabb_file::directory(&home, Path::new("data/workspaces/repository")).unwrap();

    git(&workspace, &["init", "-q"]).unwrap();
    fs::write(workspace.join("private.txt"), "local data").unwrap();
    git(&workspace, &["add", "private.txt"]).unwrap();

    let before = git(&workspace, &["ls-files", "--stage"]).unwrap();
    let inspected = inspect(&workspace).unwrap();

    assert!(!inspected.initialize && inspected.commit);

    prepare(&inspected).unwrap();

    assert_eq!(git(&workspace, &["ls-files", "--stage"]).unwrap(), before);
    assert_eq!(git(&workspace, &["ls-tree", "HEAD"]).unwrap(), "");
    assert_eq!(fs::read_to_string(workspace.join("private.txt")).unwrap(), "local data");

    fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn setup_rejects_invalid_configuration_and_unsafe_repositories_before_enabling() {
    let home = root("worktree-validation");
    let workspace = krabb_file::directory(&home, Path::new("data/workspaces/repository")).unwrap();
    let config = fs::read(home.join("config.toml")).unwrap();

    let invalid = Command::Setup(Options {
        channel: Channel { channel: "../discord".into(), force: false, client: false },
        yes: true,
    });

    assert!(run_with(invalid, &home, true, |_, _, _| panic!("must not prompt")).await.is_err());

    assert!(!workspace.join(".git").exists());

    prepare(&inspect(&workspace).unwrap()).unwrap();
    git(&workspace, &["config", "filter.example.clean", "must-not-run"]).unwrap();

    assert!(
        run_with(options(true), &home, true, |_, _, _| panic!("must not prompt")).await.is_err()
    );

    assert_eq!(fs::read(home.join("config.toml")).unwrap(), config);

    git(&workspace, &["config", "--unset", "filter.example.clean"]).unwrap();

    let nested = workspace.join("nested");

    fs::create_dir(&nested).unwrap();

    assert!(inspect(&nested).unwrap_err().to_string().contains("inside another"));
    assert!(inspect(&workspace.join("missing")).is_err());
    assert!(inspect(&home.join("data/system/KRAB.md")).is_err());

    let stale = Workspace { root: workspace.clone(), initialize: true, commit: true };

    assert!(prepare(&stale).is_err());

    fs::write(home.join("config.toml"), "invalid = true\n").unwrap();

    assert!(
        run_with(options(true), &home, true, |_, _, _| panic!("must not prompt")).await.is_err()
    );

    fs::remove_file(home.join("config.toml")).unwrap();

    assert!(
        run_with(options(true), &home, true, |_, _, _| panic!("must not prompt")).await.is_err()
    );

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn every_channel_defaults_to_no_worktree_and_preserves_explicit_opt_in() {
    for channel in ["discord", "telegram", "whatsapp", "signal", "slack", "custom"] {
        let config: Config = toml::from_str(&format!("[channels.{channel}]\n")).unwrap();

        assert!(!config.channels[channel].worktree);
    }

    assert!(!ChannelConfig::default().worktree);

    let config: Config = toml::from_str("[channels.discord]\nworktree=true\n").unwrap();

    assert!(config.channels["discord"].worktree);
}

#[test]
fn cli_exposes_setup_disable_and_confirmation_options() {
    let cli =
        Cli::try_parse_from(["krabb", "worktree", "setup", "discord", "--yes", "--force"]).unwrap();

    assert!(matches!(
        cli.command,
        crate::Command::Worktree {
            command: Command::Setup(Options { yes: true, channel: Channel { force: true, .. } })
        }
    ));

    assert_eq!(crate::command_label(&cli.command), "worktree");
    assert!(crate::native_command("worktree"));
    assert!(Cli::try_parse_from(["krabb", "worktree", "setup"]).is_err());
    assert!(Cli::try_parse_from(["krabb", "worktree", "disable", "discord"]).is_ok());
}

#[test]
fn setup_excludes_runtime_data_and_rejects_a_baseline_that_contains_it() {
    assert!(isolated(Path::new("home/worktrees/conversation")));

    for name in [".krabb", ".KRABB"] {
        let home = root(&format!("worktree-baseline-{name}"));

        let workspace =
            krabb_file::directory(&home, Path::new("data/workspaces/repository")).unwrap();

        prepare(&inspect(&workspace).unwrap()).unwrap();

        assert!(
            fs::read_to_string(workspace.join(".git/info/exclude")).unwrap().contains(".krabb/")
        );

        fs::create_dir_all(workspace.join(name)).unwrap();
        fs::write(workspace.join(name).join("private"), "fixture").unwrap();
        git(&workspace, &["add", "--force", &format!("{name}/private")]).unwrap();

        git(
            &workspace,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@localhost",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .unwrap();

        assert!(inspect(&workspace).unwrap_err().to_string().contains("reserved .krabb"));
        assert!(crate::isolate_at("discord-1", &home).is_err());

        fs::remove_dir_all(home).unwrap();
    }
}
