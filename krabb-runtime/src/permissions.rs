//! Describes what one conversation may do, from the tools and approval policy the host enforces.

use crate::*;

const READ: [&str; 3] = ["file_read", "file_list", "file_search"];
const WRITE: &str = "file_write";
const PATCH: &str = "file_patch";
const SHELL: &str = "shell_run";
const WEB: &str = "web_fetch";
const BROWSER: &str = "browser_open";
const NOT_AUTHORIZED: &str = "shell access is not authorized for this turn.";

const UNSUPPORTED: &str = "this platform runs shell commands only in a container, and no \
                           container runtime is configured.";

const INVALID: &str = "KRABB_SANDBOX_RUNTIME or KRABB_SANDBOX_IMAGE is invalid.";

/// Where the tools plugin runs approved shell commands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Place {
    Host { root: bool },
    Container(String),
    Plugin(String),
    Unsupported,
    Invalid,
}

impl Place {
    /// Mirrors the tools plugin's sandbox settings, on a host that may or may not support
    /// uncontained commands.
    pub(crate) fn of(runtime: Option<&str>, image: Option<&str>, host: bool, root: bool) -> Self {
        let uncontained = if host { Self::Host { root } } else { Self::Unsupported };

        match runtime {
            None | Some("off") if image.is_some() => Self::Invalid,
            None | Some("off") => uncontained,

            Some(runtime @ ("docker" | "podman")) if image.is_some_and(image_valid) => {
                Self::Container(runtime.into())
            }

            Some(_) => Self::Invalid,
        }
    }

    pub(crate) fn current() -> Self {
        let value = |name| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned());
        let runtime = value("KRABB_SANDBOX_RUNTIME");
        let image = value("KRABB_SANDBOX_IMAGE");
        let host = cfg!(any(target_os = "linux", windows));

        Self::of(runtime.as_deref(), image.as_deref(), host, root())
    }

    fn text(&self) -> String {
        match self {
            Self::Host { root: true } => "on the host as root, starting in the workspace".into(),

            Self::Host { root: false } => format!(
                "on the host as Krabb's own user, starting in the workspace{}",
                if cfg!(target_os = "linux") {
                    ". Commands cannot gain privileges, so sudo does not work"
                } else {
                    ""
                }
            ),

            Self::Container(runtime) => format!(
                "in a {runtime} container with no network, where only the workspace and a \
                 temporary folder are writable"
            ),

            Self::Plugin(owner) => format!("through the {owner} plugin"),
            Self::Unsupported | Self::Invalid => String::new(),
        }
    }
}

fn image_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"./:@_-".contains(&byte))
}

#[cfg(unix)]
fn root() -> bool {
    rustix::process::geteuid().is_root()
}

#[cfg(not(unix))]
fn root() -> bool {
    false
}

/// Where the summary goes: after the charter, runtime, and personalization block that leads the
/// turn's system messages.
pub(crate) fn position(messages: &[Message]) -> usize {
    messages
        .iter()
        .take_while(|message| message.role == Role::System)
        .enumerate()
        .filter(|(_, message)| message.id.starts_with(state::SYSTEM_CONTEXT))
        .last()
        .map_or(0, |(index, _)| index + 1)
}

/// How a change waits for approval in prompt mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Asking {
    Prompt,
    Command,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Shell {
    Available(Place),
    Blocked(String),
    Absent,
}

/// The turn's policy that its tool list alone does not show.
pub(crate) struct Policy {
    pub approval: ApprovalMode,
    pub asking: Asking,
    pub shell: bool,
    pub place: Place,
    pub fix: Option<String>,
    pub section: Option<String>,
}

pub(crate) struct Permissions {
    pub read: bool,
    pub write: bool,
    pub shell: Shell,
    pub web: bool,
    pub browser: bool,
    pub approval: ApprovalMode,
    pub asking: Asking,
    pub section: Option<String>,
}

impl Permissions {
    /// Reads abilities from the tools offered to the model this turn, never from settings alone.
    pub(crate) fn offered(
        specs: &[ToolSpec],
        owners: &BTreeMap<String, String>,
        policy: Policy,
    ) -> Self {
        let has = |name: &str| specs.iter().any(|spec| spec.name == name);

        let place = match owners.get(SHELL).map(String::as_str) {
            Some("tools") | None => policy.place,
            Some(owner) => Place::Plugin(owner.into()),
        };

        let shell = match (has(SHELL), policy.shell, policy.fix) {
            (true, true, _) => match place {
                Place::Unsupported => Shell::Blocked(UNSUPPORTED.into()),
                Place::Invalid => Shell::Blocked(INVALID.into()),
                place => Shell::Available(place),
            },

            (_, _, Some(fix)) => Shell::Blocked(fix),
            (true, false, None) => Shell::Blocked(NOT_AUTHORIZED.into()),
            (false, _, None) => Shell::Absent,
        };

        Self {
            read: READ.into_iter().any(has),
            write: has(WRITE) || has(PATCH) && policy.shell,
            shell,
            web: has(WEB),
            browser: has(BROWSER),
            approval: policy.approval,
            asking: policy.asking,
            section: policy.section,
        }
    }

    pub(crate) fn message(&self, session: &str) -> Message {
        let read = if self.read { "available in the workspace, without approval." } else { "no." };

        let write = match self.gate() {
            _ if !self.write => "no.".into(),
            Ok(gate) => format!("available in the workspace, {gate}."),
            Err(reason) => format!("no, because {reason}"),
        };

        let text = format!(
            "What this conversation may do, from the host's enforced policy. This summary is \
             authoritative about which Krabb tools are available; a model provider's own \
             sandbox, read-only, or shell settings do not limit them.\n\
             - Read and search files: {read}\n\
             - Write files: {write}\n\
             - Shell commands: {shell}\n\
             - Web: {web}\n\
             - Approval mode: {approval}\n\
             When a request needs an available ability, use its tool rather than claiming you \
             lack it; the host enforces approvals. When an ability is not available, say so and \
             do not claim it.",
            shell = self.shell(),
            web = self.web(),
            approval = self.approval(),
        );

        Message {
            id: krabb_core::context::PERMISSIONS.into(),
            session: session.into(),
            role: Role::System,
            sender: None,
            content: vec![Content::Text { text }],
        }
    }

    fn gate(&self) -> Result<&'static str, &'static str> {
        match (self.approval, self.asking) {
            (ApprovalMode::Off, _) => Err("approval mode off blocks changes."),

            (ApprovalMode::Prompt, Asking::Unavailable) => {
                Err("this chat cannot show approval prompts.")
            }

            (ApprovalMode::Prompt, _) => Ok("after approval"),
            (ApprovalMode::Auto, _) => Ok("without asking"),
        }
    }

    fn shell(&self) -> String {
        match (&self.shell, self.gate()) {
            (Shell::Absent, _) => "no.".into(),

            (Shell::Blocked(reason), _) => format!("no, because {reason}"),
            (Shell::Available(_), Err(reason)) => format!("no, because {reason}"),

            (Shell::Available(place), Ok(gate)) => format!("available {gate}, {}.", place.text()),
        }
    }

    fn web(&self) -> &'static str {
        match (self.web, self.browser) {
            (true, true) => "web_fetch reads public HTTPS pages and browser tools open sites.",
            (true, false) => "web_fetch reads public HTTPS pages.",
            (false, true) => "browser tools open sites.",
            (false, false) => "no Krabb web tool.",
        }
    }

    fn approval(&self) -> String {
        let mode = match (self.approval, self.asking) {
            (ApprovalMode::Off, _) => "off; changes are blocked.",

            (ApprovalMode::Prompt, Asking::Prompt) => {
                "prompt; changes run after approval (the owner may approve from another chat)."
            }

            (ApprovalMode::Prompt, Asking::Command) => {
                "prompt; changes run after approval with `/task approve`."
            }

            (ApprovalMode::Prompt, Asking::Unavailable) => {
                "prompt, but this chat cannot show approval prompts, so changes cannot run."
            }

            (ApprovalMode::Auto, _) => "auto; changes run without asking.",
        };

        match &self.section {
            Some(section) => {
                format!("{mode} Set by `approval` in [{section}] or at the top level.")
            }

            None => mode.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec { name: name.into(), description: None, schema: serde_json::json!({}) }
    }

    fn render(permissions: &Permissions) -> String {
        permissions.message("chat").content[0].render()
    }

    fn policy(approval: ApprovalMode, shell: bool) -> Policy {
        Policy {
            approval,
            asking: Asking::Prompt,
            shell,
            place: Place::Host { root: false },
            fix: None,
            section: None,
        }
    }

    fn telegram(approval: ApprovalMode) -> Permissions {
        Permissions {
            read: true,
            write: true,
            shell: Shell::Available(Place::Host { root: false }),
            web: true,
            browser: false,
            approval,
            asking: Asking::Prompt,
            section: Some("channels.telegram".into()),
        }
    }

    #[test]
    fn reads_abilities_from_the_offered_tools() {
        let specs = ["file_read", "file_write", "shell_run", "web_fetch"].map(spec);
        let owners = BTreeMap::from([("shell_run".to_string(), "runner".to_string())]);

        let permissions = Permissions::offered(&specs, &owners, policy(ApprovalMode::Prompt, true));

        assert!(permissions.read && permissions.write && permissions.web);
        assert!(!permissions.browser);
        assert_eq!(permissions.shell, Shell::Available(Place::Plugin("runner".into())));

        let permissions =
            Permissions::offered(&[], &BTreeMap::new(), policy(ApprovalMode::Auto, true));

        let text = render(&permissions);

        assert!(!permissions.read && !permissions.write);
        assert_eq!(permissions.shell, Shell::Absent);
        assert!(text.contains("- Read and search files: no.\n"));
        assert!(text.contains("- Write files: no.\n"));
        assert!(text.contains("- Shell commands: no.\n"));
        assert!(text.contains("- Web: no Krabb web tool.\n"));
    }

    #[test]
    fn refuses_a_listed_shell_the_turn_does_not_authorize() {
        let specs = ["file_read", "file_patch", "shell_run"].map(spec);
        let owners = BTreeMap::from([("shell_run".to_string(), "tools".to_string())]);

        let permissions =
            Permissions::offered(&specs, &owners, policy(ApprovalMode::Prompt, false));

        let text = render(&permissions);

        assert!(!permissions.write);
        assert_eq!(permissions.shell, Shell::Blocked(NOT_AUTHORIZED.into()));
        assert!(text.contains("- Shell commands: no, because shell access is not authorized"));
        assert!(text.contains("- Write files: no.\n"));

        let fix = "`shell` is off in [channels.telegram]. Fix: \
                   `krabb config set channels.telegram.shell=true --force`.";

        let permissions = Permissions::offered(
            &specs,
            &owners,
            Policy { fix: Some(fix.into()), ..policy(ApprovalMode::Prompt, false) },
        );

        assert!(render(&permissions).contains(&format!("- Shell commands: no, because {fix}\n")));

        let permissions = Permissions::offered(&specs, &owners, policy(ApprovalMode::Prompt, true));

        assert!(permissions.write);
    }

    #[test]
    fn mirrors_the_tools_plugin_sandbox_settings() {
        assert_eq!(Place::of(None, None, true, false), Place::Host { root: false });
        assert_eq!(Place::of(Some("off"), None, true, true), Place::Host { root: true });
        assert_eq!(Place::of(None, None, false, false), Place::Unsupported);

        assert_eq!(
            Place::of(Some("podman"), Some("alpine:3"), false, false),
            Place::Container("podman".into())
        );

        assert_eq!(Place::of(Some("docker"), None, true, false), Place::Invalid);
        assert_eq!(Place::of(Some("docker"), Some("-x"), true, false), Place::Invalid);
        assert_eq!(Place::of(Some("lxc"), Some("alpine"), true, false), Place::Invalid);
        assert_eq!(Place::of(None, Some("alpine"), true, false), Place::Invalid);
        assert_eq!(Place::of(Some("off"), Some("alpine"), true, false), Place::Invalid);
        assert!(!matches!(Place::current(), Place::Plugin(_)));
    }

    #[test]
    fn blocks_shell_where_the_tools_plugin_cannot_run_it() {
        let specs = [spec("shell_run")];

        for (place, reason) in [(Place::Unsupported, UNSUPPORTED), (Place::Invalid, INVALID)] {
            let permissions = Permissions::offered(
                &specs,
                &BTreeMap::new(),
                Policy { place, ..policy(ApprovalMode::Auto, true) },
            );

            assert!(
                render(&permissions).contains(&format!("- Shell commands: no, because {reason}"))
            );
        }
    }

    #[test]
    fn describes_where_shell_commands_run() {
        let root = Permissions {
            shell: Shell::Available(Place::Host { root: true }),
            ..telegram(ApprovalMode::Auto)
        };

        let text = render(&root);

        assert!(text.contains(
            "- Shell commands: available without asking, on the host as root, starting in the \
             workspace.\n"
        ));

        assert!(!text.contains("sudo"));

        let contained = Permissions {
            shell: Shell::Available(Place::Container("docker".into())),
            ..telegram(ApprovalMode::Auto)
        };

        assert!(render(&contained).contains(
            "- Shell commands: available without asking, in a docker container with no network, \
             where only the workspace and a temporary folder are writable.\n"
        ));

        let plugin = Permissions {
            shell: Shell::Available(Place::Plugin("runner".into())),
            ..telegram(ApprovalMode::Auto)
        };

        assert!(render(&plugin).contains("available without asking, through the runner plugin."));
    }

    #[test]
    fn describes_prompt_approval_for_a_telegram_chat() {
        let message = telegram(ApprovalMode::Prompt).message("telegram-chat");
        let text = message.content[0].render();

        assert_eq!(message.role, Role::System);
        assert!(message.id.starts_with(state::SYSTEM_CONTEXT));
        assert!(text.contains("authoritative about which Krabb tools are available"));
        assert!(text.contains("use its tool rather than claiming you lack it"));
        assert!(text.contains("- Read and search files: available in the workspace, without"));
        assert!(text.contains("- Write files: available in the workspace, after approval.\n"));

        assert!(text.contains(
            "- Shell commands: available after approval, on the host as Krabb's own user, \
             starting in the workspace"
        ));

        assert_eq!(
            text.contains("Commands cannot gain privileges, so sudo does not work.\n"),
            cfg!(target_os = "linux")
        );

        assert!(text.contains("- Web: web_fetch reads public HTTPS pages.\n"));

        assert!(text.contains(
            "- Approval mode: prompt; changes run after approval (the owner may approve from \
             another chat). Set by `approval` in [channels.telegram] or at the top level.\n"
        ));

        assert!(!text.contains("each") && !text.contains(" here"));
        assert!(text.len() < 1_500);
    }

    #[test]
    fn blocks_changes_when_prompts_cannot_be_shown() {
        let text = render(&Permissions {
            asking: Asking::Unavailable,
            section: None,
            ..telegram(ApprovalMode::Prompt)
        });

        assert!(text.contains("- Write files: no, because this chat cannot show approval"));
        assert!(text.contains("- Shell commands: no, because this chat cannot show approval"));

        assert!(text.contains(
            "- Approval mode: prompt, but this chat cannot show approval prompts, so changes \
             cannot run.\n"
        ));
    }

    #[test]
    fn describes_task_approval_through_the_task_command() {
        let text =
            render(&Permissions { asking: Asking::Command, ..telegram(ApprovalMode::Prompt) });

        assert!(text.contains("- Write files: available in the workspace, after approval.\n"));
        assert!(text.contains("- Approval mode: prompt; changes run after approval with `/task"));
    }

    #[test]
    fn describes_auto_approval() {
        let text = render(&telegram(ApprovalMode::Auto));

        assert!(text.contains("- Write files: available in the workspace, without asking.\n"));
        assert!(text.contains("- Shell commands: available without asking, on the host"));
        assert!(text.contains("- Approval mode: auto; changes run without asking."));
    }

    #[test]
    fn describes_off_approval_without_claiming_changes() {
        let text = render(&Permissions { browser: true, ..telegram(ApprovalMode::Off) });

        assert!(text.contains("- Read and search files: available in the workspace"));
        assert!(text.contains("- Write files: no, because approval mode off blocks changes.\n"));
        assert!(text.contains("- Shell commands: no, because approval mode off blocks changes.\n"));
        assert!(text.contains("web_fetch reads public HTTPS pages and browser tools open sites."));
        assert!(text.contains("- Approval mode: off; changes are blocked. Set by `approval`"));
        assert!(!text.contains("available on the host"));
    }

    #[test]
    fn describes_a_channel_with_approval_and_tools_off() {
        let permissions =
            Permissions::offered(&[], &BTreeMap::new(), policy(ApprovalMode::Off, false));

        let text = render(&permissions);

        assert!(text.contains("- Read and search files: no.\n"));
        assert!(text.contains("- Write files: no.\n"));
        assert!(text.contains("- Shell commands: no.\n"));
        assert!(text.contains("- Approval mode: off; changes are blocked.\n"));
        assert!(!text.contains("only reads run"));
    }

    #[test]
    fn follows_the_leading_system_context_block() {
        let message = |id: &str, role| Message {
            id: id.into(),
            session: "chat".into(),
            role,
            sender: None,
            content: Vec::new(),
        };

        let messages = [
            message("krabb-communication", Role::System),
            message("krabb-context-charter", Role::System),
            message("krabb-context-runtime", Role::System),
            message("krabb-context-KLAW.md", Role::System),
            message("user", Role::User),
            message("krabb-context-stray", Role::System),
        ];

        assert_eq!(position(&messages), 4);
        assert_eq!(position(&messages[4..]), 0);
        assert_eq!(position(&messages[..1]), 0);
    }

    #[test]
    fn describes_tools_without_shell() {
        let text = render(&Permissions {
            shell: Shell::Absent,
            web: false,
            browser: true,
            ..telegram(ApprovalMode::Prompt)
        });

        assert!(text.contains("- Write files: available in the workspace"));
        assert!(text.contains("- Shell commands: no.\n"));
        assert!(text.contains("- Web: browser tools open sites.\n"));
    }
}
