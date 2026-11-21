//! Command-line arguments for `krabb`.

use crate::*;

#[derive(Debug, Parser)]
#[command(
    name = NAME,
    version = VERSION,
    about = "A self-hostable agent OS for native and external AI agents.",
    before_help = BANNER,
    styles = cli_styles(),
    disable_help_flag = true,
    disable_version_flag = true
)]
pub(crate) struct Cli {
    #[arg(long, global = true, help = "Render command output as JSON where supported.")]
    pub(crate) json: bool,
    #[arg(long, global = true, help = "Show diagnostic error details.")]
    pub(crate) debug: bool,
    #[arg(long, global = true, help = "Show diagnostic progress and timing.")]
    pub(crate) verbose: bool,
    #[arg(
        short = 'v',
        visible_short_alias = 'V',
        long = "version",
        action = ArgAction::Version,
        help = "Print version."
    )]
    pub(crate) version_flag: Option<bool>,
    #[arg(
        short = 'h',
        visible_short_alias = 'H',
        long = "help",
        global = true,
        action = ArgAction::Help,
        required = false,
        help = "Print help."
    )]
    pub(crate) help: Option<bool>,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(about = "Initialize Krabb configuration and its global workspace.")]
    Init(InitArgs),
    #[command(about = "Check configuration, plugins, credentials, and local state.")]
    Doctor(DoctorArgs),
    #[command(about = "Read or update Krabb configuration.")]
    Config(ConfigArgs),
    #[command(about = "Set up optional Git worktrees for channel conversations.")]
    Worktree {
        #[command(subcommand)]
        command: worktree::Command,
    },
    #[command(about = "Assign an existing workspace file to global or a channel/client.")]
    Workspace {
        path: PathBuf,
        #[arg(long)]
        owner: String,
    },
    #[command(about = "Show installation and daemon health.")]
    Status(Output),
    #[command(about = "Print the Krabb version.")]
    Version(Output),
    #[command(about = "Generate shell completion scripts.")]
    Completion {
        #[arg(value_enum, help = "Shell to generate completions for.")]
        shell: CompletionShell,
    },

    #[command(about = "Manage installed plugins.")]
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },

    #[command(about = "Send one prompt through an intelligence provider.")]
    Ask(Ask),

    #[command(hide = true, about = "Schedule and manage autonomous agent tasks.")]
    Schedule(ScheduleArgs),

    #[command(hide = true, about = "Search and manage scoped agent memories.")]
    Memory(NativePluginCommandArgs),

    #[command(hide = true, about = "Open the Krabb terminal interface.")]
    Tui(NativePluginCommandArgs),

    #[command(hide = true, about = "Explore and manage structured SQLite data.")]
    Data(NativePluginCommandArgs),

    #[command(
        about = "Install, uninstall, and control the native service.",
        arg_required_else_help = true,
        subcommand_required = true
    )]
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },

    #[command(about = "Update Krabb and the plugins it needs, or roll back the last update.")]
    Update(upgrade::Options),

    #[command(about = "Export the local configuration and plugin lock.")]
    Export(KrabfileExport),
    #[command(about = "Validate a Krabfile without changing local state.")]
    Validate(KrabfileValidate),
    #[command(about = "Import a configuration and plugin lock.")]
    Import(KrabfileImport),
    #[command(external_subcommand, hide = true)]
    External(Vec<String>),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Fish,
    #[value(name = "powershell")]
    PowerShell,
    Zsh,
}

#[derive(Debug, Args)]
pub(crate) struct KrabfileExport {
    #[arg(value_name = "PATH", help = "Write the Krabfile to PATH or a directory.")]
    pub(crate) destination: Option<PathBuf>,
    #[arg(long, value_name = "PATH", help = "Write the Krabfile to PATH or a directory.")]
    pub(crate) path: Option<PathBuf>,
    #[arg(long, help = "Overwrite an existing Krabfile.")]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct KrabfileValidate {
    #[arg(long, value_name = "PATH", help = "Read the Krabfile from PATH.")]
    pub(crate) path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct KrabfileImport {
    #[arg(long, value_name = "PATH", help = "Read the Krabfile from PATH.")]
    pub(crate) path: Option<PathBuf>,
    #[arg(short, long, help = "Skip the confirmation prompt.")]
    pub(crate) yes: bool,
    #[arg(long, help = "Replace existing local state.")]
    pub(crate) force: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PluginCommand {
    #[command(about = "Securely configure an installed plugin's secrets and settings.")]
    Setup(setup::Options),
    #[command(about = "List installed plugins.")]
    List(ListOutput),
    #[command(visible_alias = "add", about = "Install and activate one or more plugins.")]
    Install(PluginInstall),
    #[command(about = "Review available plugin updates; pass -y to apply them.")]
    Update(PluginUpdate),
    #[command(about = "List the published versions of an official plugin.")]
    Versions(Id),
    #[command(
        visible_alias = "stop",
        about = "Detach a plugin from the runtime without uninstalling it."
    )]
    Detach(Id),
    #[command(visible_alias = "start", about = "Attach an installed plugin to the running daemon.")]
    Attach(Id),
    #[command(
        name = "uninstall",
        visible_alias = "remove",
        about = "Uninstall one or more plugins."
    )]
    Remove(PluginUninstall),
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    #[command(about = "Create a durable session.")]
    New(SessionNew),
    #[command(about = "List durable sessions.")]
    List(ListOutput),
    #[command(about = "Show a session transcript and metadata.")]
    Show(Id),
    #[command(about = "Fork a session into a new session.")]
    Fork(Fork),
    #[command(about = "Change the model used by a session.")]
    Model(SessionModel),
    #[command(about = "Cancel an active session turn.")]
    Cancel(Id),
    #[command(about = "Delete a session and reclaim its worktree.")]
    Delete(SessionDelete),
}

#[derive(Debug, Subcommand)]
pub(crate) enum DeliveryCommand {
    #[command(about = "List pending and uncertain deliveries.")]
    List(ListOutput),
    #[command(about = "Retry a delivery explicitly.")]
    Retry(Name),
    #[command(about = "Drop one delivery or all matching deliveries without retrying them.")]
    Drop(delivery::Drop),
}

#[derive(Debug, Parser)]
#[command(name = "session", about = "Manage durable agent sessions.", styles = cli_styles())]
pub(crate) struct SessionCli {
    #[arg(long, global = true, help = "Render command output as JSON where supported.")]
    pub(crate) json: bool,
    #[arg(long, global = true, hide = true)]
    pub(crate) debug: bool,
    #[arg(long, global = true, hide = true)]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: SessionCommand,
}

#[derive(Debug, Parser)]
#[command(name = "delivery", about = "Manage pending channel deliveries.", styles = cli_styles())]
pub(crate) struct DeliveryCli {
    #[arg(long, global = true, help = "Render command output as JSON where supported.")]
    pub(crate) json: bool,
    #[arg(long, global = true, hide = true)]
    pub(crate) debug: bool,
    #[arg(long, global = true, hide = true)]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: DeliveryCommand,
}

#[derive(Debug, Args)]
#[command(disable_help_subcommand = true)]
pub(crate) struct ScheduleArgs {
    #[command(subcommand)]
    pub(crate) command: Option<ScheduleCommand>,
}

#[derive(Debug, Args)]
pub(crate) struct NativePluginCommandArgs {
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ScheduleCommand {
    #[command(about = "Show schedule help.")]
    Help,
    #[command(about = "Propose a one-time or recurring task.")]
    Add(ScheduleValues),
    #[command(about = "List scheduled tasks.")]
    List(ScheduleValues),
    #[command(about = "Approve and activate a task.")]
    Approve(ScheduleValues),
    #[command(about = "Run an active task now.")]
    Run(ScheduleValues),
    #[command(about = "Retry a task paused after failure or skipped while offline.")]
    Retry(ScheduleValues),
    #[command(about = "Pause an active task.")]
    Pause(ScheduleValues),
    #[command(about = "Resume a paused task.")]
    Resume(ScheduleValues),
    #[command(about = "Cancel a task.")]
    Cancel(ScheduleValues),
    #[command(about = "Show task execution history.")]
    History(ScheduleValues),
    #[command(hide = true)]
    Remove(ScheduleValues),
}

#[derive(Debug, Args)]
pub(crate) struct ScheduleValues {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) values: Vec<String>,
}

impl ScheduleCommand {
    pub(crate) fn into_values(self) -> Vec<String> {
        let (action, values) = match self {
            Self::Help => ("help", Vec::new()),
            Self::Add(args) => ("add", args.values),
            Self::List(args) => ("list", args.values),
            Self::Approve(args) => ("approve", args.values),
            Self::Run(args) => ("run", args.values),
            Self::Retry(args) => ("retry", args.values),
            Self::Pause(args) => ("pause", args.values),
            Self::Resume(args) => ("resume", args.values),
            Self::Cancel(args) => ("cancel", args.values),
            Self::History(args) => ("history", args.values),
            Self::Remove(args) => ("remove", args.values),
        };

        std::iter::once(action.to_owned()).chain(values).collect()
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum ServiceCommand {
    #[command(about = "Install the native service definition.")]
    Install(ServiceInstall),
    #[command(name = "uninstall", about = "Uninstall the native service.")]
    Remove(ServiceRemove),
    #[command(about = "Show native service status.")]
    Status,
    #[command(about = "Start the native service.")]
    Start,
    #[command(about = "Stop the native service.")]
    Stop,
    #[command(about = "Restart the native service.")]
    Restart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceAction {
    Start,
    Stop,
    Restart,
}

#[derive(Debug, Args)]
#[command(arg_required_else_help = true, subcommand_required = true)]
pub(crate) struct ConfigArgs {
    #[command(subcommand)]
    pub(crate) command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    #[command(about = "Show all settings or one setting.")]
    Get(ConfigGet),
    #[command(about = "Set one setting using KEY=VALUE.")]
    Set(ConfigSet),
}

#[derive(Debug, Args)]
pub(crate) struct ConfigGet {
    #[arg(value_name = "KEY", help = "Dotted setting name, such as clients.tui.tools.")]
    pub(crate) key: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigSet {
    #[arg(value_name = "KEY=VALUE", help = "Dotted key and TOML value to set.")]
    pub(crate) assignment: String,
    #[arg(long, help = "Restart the running daemon to apply restart-required settings.")]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ServiceInstall {
    #[arg(long, help = "Replace an existing service definition.")]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ServiceRemove {
    #[arg(short, long, help = "Skip the confirmation prompt.")]
    pub(crate) yes: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SessionNew {
    #[arg(help = "Session identifier.")]
    pub(crate) id: String,
    #[arg(long, help = "Model identifier.")]
    pub(crate) model: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct Fork {
    #[arg(help = "Source session identifier.")]
    pub(crate) source: String,
    #[arg(help = "Target session identifier.")]
    pub(crate) target: String,
}

#[derive(Debug, Args)]
pub(crate) struct SessionModel {
    #[arg(help = "Session identifier.")]
    pub(crate) id: String,
    #[arg(help = "Model name, or inherit to follow a channel's configured model.")]
    pub(crate) model: String,
}

#[derive(Debug, Args)]
pub(crate) struct SessionDelete {
    #[arg(help = "Session identifier.")]
    pub(crate) id: String,
    #[arg(short, long, help = "Skip the confirmation prompt.")]
    pub(crate) yes: bool,
}

#[derive(Debug, Args)]
pub(crate) struct Output {
    #[arg(long, help = "Render the result as JSON.")]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ListOutput {
    #[arg(value_name = "PAGE", default_value_t = 1, help = "Page number to display.")]
    pub(crate) page: usize,
    #[arg(long, help = "Render the complete result as JSON.")]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PluginUpdate {
    #[arg(short, long, help = "Apply the updates listed by the preview.")]
    pub(crate) yes: bool,
    #[arg(long, help = "Render the preview or update result as JSON.")]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    #[arg(long, help = "Reset all Krabb home data and recreate its defaults.")]
    pub(crate) force: bool,
    #[arg(short, long, help = "Skip the confirmation prompt.")]
    pub(crate) yes: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    #[arg(long, help = "Create missing safe local state without overwriting configuration.")]
    pub(crate) fix: bool,
}

#[derive(Debug, Args)]
pub(crate) struct Source {
    pub(crate) id: String,
    pub(crate) source: Option<String>,
    pub(crate) revision: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) yes: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PluginInstall {
    #[arg(
        value_name = "ID[@VERSION]",
        required = true,
        num_args = 1..,
        help = "Official plugin IDs, optionally pinned to a published version or @latest."
    )]
    pub(crate) ids: Vec<String>,
    #[arg(
        long,
        value_name = "SOURCE",
        help = "Use a local path, Git URL, or verified archive (one plugin only)."
    )]
    pub(crate) source: Option<String>,
    #[arg(long, value_name = "REVISION", help = "Pin the selected Git source revision.")]
    pub(crate) revision: Option<String>,
    #[arg(long, help = "Link a local build instead of copying its executable.")]
    pub(crate) link: bool,
    #[arg(
        long,
        help = "Install the newest version, pre-releases included; pre-releases are pinned."
    )]
    pub(crate) unstable: bool,
    #[arg(short, long, help = "Skip replacement confirmation.")]
    pub(crate) yes: bool,
}

pub(crate) struct SourceRoot {
    pub(crate) path: PathBuf,
    pub(crate) temp: Option<PathBuf>,
}
