//! Slash commands shared by every channel and client.
//!
//! Exact command text is recognized here; natural-language requests reach the same commands
//! through the model's `conversation` tool, so no keyword parsing happens in the host.

use crate::Capability;

/// Prefix that addresses Krabb on platforms that reserve short command names, such as Slack.
pub const ROOT: &str = "krabb";

/// Bound on command names offered to native menus; platforms reject longer ones.
pub const NAME_LIMIT: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Need {
    /// Always available.
    Nothing,
    /// Needs a loaded plugin with this capability.
    Capability(Capability),
}

/// Who may run a command, on top of what it needs loaded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    /// Every caller of the conversation.
    Anyone,
    /// Only local clients, never channel users.
    Local,
    /// Only callers whose conversation allows tools.
    Tools,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Spec {
    pub name: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub need: Need,
    pub access: Access,
}

impl Spec {
    /// Whether channel menus may offer this command.
    pub fn channel(&self) -> bool {
        self.access != Access::Local
    }
}

pub const SPECS: &[Spec] = &[
    Spec {
        name: "help",
        summary: "Show the commands available here",
        usage: "/help [command]",
        need: Need::Nothing,
        access: Access::Anyone,
    },
    Spec {
        name: "status",
        summary: "Show this conversation's model and activity",
        usage: "/status",
        need: Need::Nothing,
        access: Access::Anyone,
    },
    Spec {
        name: "model",
        summary: "Show, list, or choose this conversation's model",
        usage: "/model [list | <model>]",
        need: Need::Capability(Capability::Model),
        access: Access::Anyone,
    },
    Spec {
        name: "session",
        summary: "Start a new conversation, cancel a reply, or clear history",
        usage: "/session [new | cancel | clear]",
        need: Need::Nothing,
        access: Access::Anyone,
    },
    Spec {
        name: "task",
        summary: "List or control background tasks",
        usage: "/task [pause | resume | cancel | approve | deny <id>]",
        need: Need::Nothing,
        access: Access::Anyone,
    },
    Spec {
        name: "workspace",
        summary: "Show where this conversation's files live",
        usage: "/workspace",
        need: Need::Nothing,
        access: Access::Anyone,
    },
    Spec {
        name: "approvals",
        summary: "List actions waiting for approval here",
        usage: "/approvals",
        need: Need::Nothing,
        access: Access::Anyone,
    },
    Spec {
        name: "schedule",
        summary: "List scheduled work",
        usage: "/schedule",
        need: Need::Capability(Capability::Scheduler),
        access: Access::Local,
    },
    Spec {
        name: "memory",
        summary: "Search memories available here",
        usage: "/memory <query>",
        need: Need::Capability(Capability::Memory),
        access: Access::Tools,
    },
    Spec {
        name: "data",
        summary: "List saved databases and tables",
        usage: "/data [<database>]",
        need: Need::Capability(Capability::Store),
        access: Access::Tools,
    },
    Spec {
        name: "plugin",
        summary: "List the active plugins",
        usage: "/plugin",
        need: Need::Nothing,
        access: Access::Anyone,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invocation<'a> {
    pub spec: &'static Spec,
    pub arguments: &'a str,
    /// Name after `/krabb` that matched no command; answered with help.
    pub unknown: Option<&'a str>,
}

pub fn find(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|spec| spec.name.eq_ignore_ascii_case(name))
}

/// Recognizes `/name args` and `/krabb name args`.
///
/// Unknown `/words` stay ordinary text so paths and other slashes reach the model. A
/// `/name@bot` addressed form is left alone: only its channel knows which bot it names, so
/// that channel strips its own address before the host sees the text.
pub fn parse(text: &str) -> Option<Invocation<'_>> {
    let text = text.trim();
    let rest = text.strip_prefix('/').filter(|rest| !rest.starts_with(char::is_whitespace))?;
    let (name, arguments) = split(rest);

    if name.eq_ignore_ascii_case(ROOT) {
        let (token, arguments) = split(arguments);
        let help = find("help")?;

        if token.is_empty() {
            return Some(Invocation { spec: help, arguments: "", unknown: None });
        }

        return Some(match find(token) {
            Some(spec) => Invocation { spec, arguments, unknown: None },
            None => Invocation { spec: help, arguments: "", unknown: Some(token) },
        });
    }

    find(name).map(|spec| Invocation { spec, arguments, unknown: None })
}

fn split(text: &str) -> (&str, &str) {
    let text = text.trim();

    text.split_once(char::is_whitespace).map_or((text, ""), |(token, rest)| (token, rest.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_commands_are_recognized_with_the_krabb_prefix() {
        let status = parse("  /status  ").unwrap();

        assert_eq!((status.spec.name, status.arguments, status.unknown), ("status", "", None));
        assert_eq!(parse("/Model list").unwrap().arguments, "list");
        assert_eq!(parse("/model gpt-5 mini").unwrap().arguments, "gpt-5 mini");
        assert!(parse("/status@other_bot").is_none());

        let slack = parse("/krabb session new").unwrap();

        assert_eq!((slack.spec.name, slack.arguments), ("session", "new"));
        assert_eq!(parse("/krabb").unwrap().spec.name, "help");

        let unknown = parse("/krabb frobnicate now").unwrap();

        assert_eq!((unknown.spec.name, unknown.unknown), ("help", Some("frobnicate")));
    }

    #[test]
    fn other_text_and_unknown_slashes_stay_for_the_model() {
        for text in ["status", "/usr/bin/env", "/", "/statusx", "what is /status?", "", "/ status"]
        {
            assert!(parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn names_are_unique_lowercase_and_fit_native_menus() {
        let mut names = SPECS.iter().map(|spec| spec.name).collect::<Vec<_>>();

        names.sort_unstable();
        names.dedup();

        assert_eq!(names.len(), SPECS.len());

        for spec in SPECS {
            assert!(spec.name.len() <= NAME_LIMIT && spec.name != ROOT);
            assert!(spec.name.bytes().all(|byte| byte.is_ascii_lowercase()));
            assert!(spec.usage.starts_with(&format!("/{}", spec.name)));
            assert!(!spec.summary.is_empty() && spec.summary.len() <= 100);
        }
    }
}
