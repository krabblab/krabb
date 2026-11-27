//! Describes what one conversation may do, from the tools and approval policy the host enforces.

use crate::*;

const READ: [&str; 3] = ["file_read", "file_list", "file_search"];
const WRITE: [&str; 2] = ["file_write", "file_patch"];
const SHELL: &str = "shell_run";
const WEB: &str = "web_fetch";
const BROWSER: &str = "browser_open";

/// Where approved shell commands run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Place {
    Host,
    Container(String),
    Plugin(String),
    Unsupported,
}

impl Place {
    /// The tools plugin's placement for a container runtime setting on a host that may or may
    /// not support uncontained commands.
    pub(crate) fn of(runtime: Option<&str>, host: bool) -> Self {
        match runtime {
            Some(runtime @ ("docker" | "podman")) => Self::Container(runtime.into()),
            _ if host => Self::Host,
            _ => Self::Unsupported,
        }
    }

    fn current() -> Self {
        let runtime = std::env::var("KRABB_SANDBOX_RUNTIME").ok();

        Self::of(runtime.as_deref(), cfg!(any(target_os = "linux", windows)))
    }
}

pub(crate) struct Permissions {
    pub read: bool,
    pub write: bool,
    pub shell: Option<Place>,
    pub web: bool,
    pub browser: bool,
    pub approval: ApprovalMode,
    pub prompts: bool,
}

impl Permissions {
    /// Reads abilities from the tools offered to the model this turn, never from settings alone.
    pub(crate) fn offered(
        specs: &[ToolSpec],
        owners: &BTreeMap<String, String>,
        approval: ApprovalMode,
        prompts: bool,
    ) -> Self {
        let has = |name: &str| specs.iter().any(|spec| spec.name == name);

        let shell = has(SHELL).then(|| match owners.get(SHELL).map(String::as_str) {
            Some("tools") | None => Place::current(),
            Some(owner) => Place::Plugin(owner.into()),
        });

        Self {
            read: READ.into_iter().any(has),
            write: WRITE.into_iter().any(has),
            shell,
            web: has(WEB),
            browser: has(BROWSER),
            approval,
            prompts,
        }
    }

    pub(crate) fn message(&self, session: &str) -> Message {
        let read = if self.read { "available in the workspace, without approval." } else { "no." };

        let write = match self.gate() {
            _ if !self.write => "no.".into(),
            Ok(gate) => format!("available in the workspace, {gate}."),
            Err(reason) => format!("no, because {reason}."),
        };

        let text = format!(
            "What this conversation may do, from the host's enforced policy. This summary is \
             authoritative: a model provider's own sandbox, read-only, or shell settings do not \
             limit Krabb's tools.\n\
             - Read and search files: {read}\n\
             - Write files: {write}\n\
             - Shell commands: {shell}\n\
             - Web: {web}\n\
             - Approval mode: {approval}\n\
             When a request needs an available ability, use the tool instead of refusing; the \
             host asks for approval where needed. When an ability is not available, say so and \
             do not claim it.",
            shell = self.shell(),
            web = self.web(),
            approval = self.approval(),
        );

        Message {
            id: format!("{}permissions", state::SYSTEM_CONTEXT),
            session: session.into(),
            role: Role::System,
            sender: None,
            content: vec![Content::Text { text }],
        }
    }

    fn gate(&self) -> Result<&'static str, &'static str> {
        match self.approval {
            ApprovalMode::Off => Err("approval mode off blocks changes"),
            ApprovalMode::Prompt if !self.prompts => Err("this chat cannot show approval prompts"),
            ApprovalMode::Prompt => Ok("after the person approves each one"),
            ApprovalMode::Auto => Ok("without asking"),
        }
    }

    fn shell(&self) -> String {
        let place = match &self.shell {
            None => return "no.".into(),

            Some(Place::Unsupported) => {
                return "no, because this platform runs shell commands only in a container, and \
                        no container runtime is configured."
                    .into();
            }

            Some(Place::Host) => format!(
                "on the host as Krabb's own user, starting in the workspace{}",
                if cfg!(target_os = "linux") {
                    ". Commands cannot gain privileges, so sudo does not work"
                } else {
                    ""
                }
            ),

            Some(Place::Container(runtime)) => format!(
                "in a {runtime} container with no network, where only the workspace is writable"
            ),

            Some(Place::Plugin(owner)) => format!("through the {owner} plugin"),
        };

        match self.gate() {
            Ok(gate) => format!("available {gate}, {place}."),
            Err(reason) => format!("no, because {reason}."),
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

    fn approval(&self) -> &'static str {
        match self.approval {
            ApprovalMode::Off => "off; changes are blocked, so only reads run.",

            ApprovalMode::Prompt if !self.prompts => {
                "prompt, but this chat cannot show approval prompts, so changes cannot run."
            }

            ApprovalMode::Prompt => "prompt; each change waits for the person's approval here.",
            ApprovalMode::Auto => "auto; changes run without asking.",
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

    fn telegram(approval: ApprovalMode) -> Permissions {
        Permissions {
            read: true,
            write: true,
            shell: Some(Place::Host),
            web: true,
            browser: false,
            approval,
            prompts: true,
        }
    }

    #[test]
    fn reads_abilities_from_the_offered_tools() {
        let specs = ["file_read", "file_write", "shell_run", "web_fetch"].map(spec);
        let owners = BTreeMap::from([("shell_run".to_string(), "runner".to_string())]);

        let permissions = Permissions::offered(&specs, &owners, ApprovalMode::Prompt, true);

        assert!(permissions.read && permissions.write && permissions.web);
        assert!(!permissions.browser);
        assert_eq!(permissions.shell, Some(Place::Plugin("runner".into())));

        let permissions = Permissions::offered(&[], &BTreeMap::new(), ApprovalMode::Auto, true);
        let text = render(&permissions);

        assert!(!permissions.read && !permissions.write && permissions.shell.is_none());
        assert!(text.contains("- Read and search files: no.\n"));
        assert!(text.contains("- Write files: no.\n"));
        assert!(text.contains("- Shell commands: no.\n"));
        assert!(text.contains("- Web: no Krabb web tool.\n"));
    }

    #[test]
    fn places_shell_commands_where_the_tools_plugin_runs_them() {
        assert_eq!(Place::of(None, true), Place::Host);
        assert_eq!(Place::of(Some("off"), true), Place::Host);
        assert_eq!(Place::of(Some("podman"), false), Place::Container("podman".into()));
        assert_eq!(Place::of(None, false), Place::Unsupported);

        let unsupported =
            Permissions { shell: Some(Place::Unsupported), ..telegram(ApprovalMode::Auto) };

        assert!(render(&unsupported).contains("- Shell commands: no, because this platform runs"));

        let contained = Permissions {
            shell: Some(Place::Container("docker".into())),
            ..telegram(ApprovalMode::Auto)
        };

        assert!(render(&contained).contains(
            "- Shell commands: available without asking, in a docker container with no network, \
             where only the workspace is writable."
        ));
    }

    #[test]
    fn describes_prompt_approval_for_a_telegram_chat() {
        let message = telegram(ApprovalMode::Prompt).message("telegram-chat");
        let text = message.content[0].render();

        assert_eq!(message.role, Role::System);
        assert!(message.id.starts_with(state::SYSTEM_CONTEXT));
        assert!(text.contains("do not limit Krabb's tools"));
        assert!(text.contains("- Read and search files: available in the workspace, without"));

        assert!(text.contains(
            "- Write files: available in the workspace, after the person approves each one.\n"
        ));

        assert!(text.contains(
            "- Shell commands: available after the person approves each one, on the host as \
             Krabb's own user, starting in the workspace"
        ));

        assert_eq!(
            text.contains("Commands cannot gain privileges, so sudo does not work.\n"),
            cfg!(target_os = "linux")
        );

        assert!(text.contains("- Web: web_fetch reads public HTTPS pages.\n"));
        assert!(text.contains("- Approval mode: prompt; each change waits for the person's"));
        assert!(text.len() < 1_500);
    }

    #[test]
    fn blocks_changes_when_prompts_cannot_be_shown() {
        let text = render(&Permissions { prompts: false, ..telegram(ApprovalMode::Prompt) });

        assert!(text.contains("- Write files: no, because this chat cannot show approval"));
        assert!(text.contains("- Shell commands: no, because this chat cannot show approval"));
        assert!(text.contains("- Approval mode: prompt, but this chat cannot show approval"));
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
        assert!(text.contains("- Approval mode: off; changes are blocked, so only reads run."));
        assert!(!text.contains("available on the host"));
    }

    #[test]
    fn describes_tools_without_shell() {
        let text = render(&Permissions {
            shell: None,
            web: false,
            browser: true,
            ..telegram(ApprovalMode::Prompt)
        });

        assert!(text.contains("- Write files: available in the workspace"));
        assert!(text.contains("- Shell commands: no.\n"));
        assert!(text.contains("- Web: browser tools open sites.\n"));
    }
}
