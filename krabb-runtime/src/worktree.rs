use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use super::{ConfigCommand, ConfigSet, PluginError};

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Check the workspace, initialize Git after confirmation, and enable isolation.
    Setup(Options),
    /// Use the shared workspace for a channel without deleting existing worktrees.
    Disable(Channel),
}

#[derive(Debug, Args)]
pub(super) struct Channel {
    /// Channel plugin ID, such as discord or telegram.
    channel: String,
    /// Restart the running daemon to apply the setting.
    #[arg(long)]
    force: bool,
    /// Configure a client instead of a channel integration.
    #[arg(long)]
    client: bool,
}

#[derive(Debug, Args)]
pub(super) struct Options {
    #[command(flatten)]
    channel: Channel,
    /// Approve enabling isolation and any required Git initialization without prompting.
    #[arg(short, long)]
    yes: bool,
}

#[derive(Debug)]
struct Workspace {
    root: PathBuf,
    initialize: bool,
    commit: bool,
}

pub(super) async fn run(command: Command, home: &Path, json: bool) -> Result<(), PluginError> {
    run_with(command, home, json, super::confirm_action).await
}

async fn run_with(
    command: Command,
    home: &Path,
    json: bool,
    confirm: impl FnOnce(bool, bool, &str) -> Result<bool, PluginError>,
) -> Result<(), PluginError> {
    let (channel, setup) = match command {
        Command::Setup(options) => (options.channel, Some(options.yes)),
        Command::Disable(channel) => (channel, None),
    };

    if !super::valid(&channel.channel) {
        return Err("Invalid channel ID.".into());
    }

    if super::load_config_text(home)?.is_none() {
        return Err("Configuration was not found. Run krabb init first.".into());
    }

    super::load_config_at(home)?.validate()?;

    if let Some(yes) = setup {
        let repository = krabb_file::directory(home, Path::new("data/workspaces/repository"))?;
        let workspace = inspect(&repository)?;
        let preparation = if workspace.initialize {
            " This initializes a Git repository with an empty initial commit."
        } else if workspace.commit {
            " This creates an empty initial commit in the existing Git repository."
        } else {
            " Existing commits will be used as the starting point."
        };

        let question = format!(
            "Enable worktrees for {} in {}?{preparation} Existing files and staged changes \
             will not be committed or modified.",
            channel.channel,
            workspace.root.display()
        );

        if !confirm(yes, json, &question)? {
            super::print_cancelled("Worktree setup", json);
            return Ok(());
        }

        prepare(&workspace)?;
    }

    super::config_command(
        ConfigCommand::Set(ConfigSet {
            assignment: format!(
                "{}.{}.worktree={}",
                if channel.client { "clients" } else { "channels" },
                channel.channel,
                setup.is_some()
            ),
            force: channel.force,
        }),
        home,
        json,
    )
    .await
}

fn inspect(root: &Path) -> Result<Workspace, PluginError> {
    let root = std::fs::canonicalize(root)
        .map_err(|_| "Workspace not found. Run krabb init or check KRABB_ROOT.")?;

    if !root.is_dir() {
        return Err("The configured workspace must be a directory.".into());
    }

    let output = super::command_output(
        super::worktree_command(&root).args(["rev-parse", "--show-toplevel"]),
    )
    .map_err(|error| {
        format!("Git could not inspect the workspace. Install Git and retry: {error}")
    })?;

    if !output.status.success() {
        let bare = git(&root, &["rev-parse", "--is-bare-repository"]);

        if bare.is_ok_and(|value| value == "true") || root.join(".git").symlink_metadata().is_ok() {
            return Err("The workspace has unusable Git metadata. Repair it before setup.".into());
        }

        return Ok(Workspace { root, initialize: true, commit: true });
    }

    let top = PathBuf::from(String::from_utf8(output.stdout)?.trim());

    if std::fs::canonicalize(top)? != root {
        return Err("The workspace is inside another Git repository. Set KRABB_ROOT to the repository root and retry.".into());
    }

    super::check_checkout(&root)?;

    let commit = git(&root, &["rev-parse", "--verify", "HEAD^{commit}"]).is_err();

    if commit {
        git(&root, &["symbolic-ref", "--quiet", "HEAD"])?;
    } else {
        baseline(&root)?;
    }

    Ok(Workspace { root, initialize: false, commit })
}

fn prepare(workspace: &Workspace) -> Result<(), PluginError> {
    // Recheck after confirmation so a new repository or HEAD is never overwritten.
    let current = inspect(&workspace.root)?;

    if current.initialize != workspace.initialize || current.commit != workspace.commit {
        return Err("Workspace Git state changed during setup. Run the command again.".into());
    }

    if current.initialize {
        git(
            &current.root,
            &["-c", "init.templateDir=", "init", "--quiet", "--initial-branch=main"],
        )?;
        let info = krabb_file::directory(&current.root, Path::new(".git/info"))?;
        krabb_file::save(info.join("exclude"), b".krabb/\n")?;
    }

    if current.commit {
        let tree = git(&current.root, &["hash-object", "-t", "tree", "-w", "--stdin"])?;
        let commit = git(
            &current.root,
            &[
                "-c",
                "user.name=Krabb",
                "-c",
                "user.email=krabb@localhost",
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                &tree,
                "-m",
                "Initialize Krabb workspaces.",
            ],
        )?;

        git(&current.root, &["update-ref", "HEAD", &commit, ""])?;
    }

    git(&current.root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    super::check_checkout(&current.root)?;

    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String, PluginError> {
    let output = super::command_output(super::worktree_command(root).args(args))?;

    if !output.status.success() {
        return Err(format!("Git {} failed; the channel setting was not changed.", args[0]).into());
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests;

pub(super) fn conversation(session: &str, root: &Path) -> Result<PathBuf, PluginError> {
    let path =
        super::isolate_at(session, root)?.ok_or("The conversation workspace is unavailable.")?;
    krabb_file::workspace_state(root, &path)?;

    Ok(path)
}

pub(super) fn media(workspace: &Path, channel: &str) -> std::io::Result<PathBuf> {
    let relative = media_path(workspace, channel);
    krabb_file::directory(workspace, &relative)
}

pub(super) fn media_path(workspace: &Path, channel: &str) -> PathBuf {
    if isolated(workspace) {
        PathBuf::from("attachments")
    } else {
        Path::new("attachments").join(String::from(super::sharing::principal(channel)))
    }
}

pub(super) fn isolated(path: &Path) -> bool {
    path.parent().and_then(Path::file_name).is_some_and(|name| name == "worktrees")
}

pub(super) fn tool(name: &str) -> bool {
    matches!(name, "read" | "write" | "list" | "search")
}

#[cfg(test)]
pub(super) fn initialize(root: &Path) -> Result<(), PluginError> {
    prepare(&inspect(root)?)
}

pub(super) fn baseline(root: &Path) -> Result<(), PluginError> {
    let paths = git(root, &["ls-tree", "-z", "--name-only", "HEAD"])?;

    if paths.split('\0').any(|path| {
        path.split('/')
            .next()
            .is_some_and(|name| name.trim_end_matches(['.', ' ']).eq_ignore_ascii_case(".krabb"))
    }) {
        return Err("The Git baseline contains reserved .krabb runtime data. Remove it from the committed baseline before enabling worktrees.".into());
    }

    Ok(())
}
