use super::*;
use krabb_core::scope::Principal;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    Session { client: String, session: String },
    Channel(AuthorizedDestination),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Origin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Principal>,
    #[serde(rename = "channel")]
    pub integration: String,
    pub chat: String,
    pub session: String,
    pub thread: Option<String>,
    pub private: Option<bool>,
    pub sender: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

impl Origin {
    pub fn source(&self) -> Principal {
        self.source.clone().unwrap_or_else(|| sharing::principal(&self.integration))
    }

    pub fn scope(&self) -> String {
        scope(&self.source(), &self.session, &self.chat, self.thread.as_deref())
    }

    pub fn destination(&self) -> Result<Destination, PluginError> {
        match self.source() {
            Principal::Client(client) => {
                if client != self.integration || !valid(&client) || !valid(&self.session) {
                    return Err("Invalid client session destination.".into());
                }

                Ok(Destination::Session { client, session: self.session.clone() })
            }

            Principal::Channel(channel) => {
                if channel != self.integration {
                    return Err("The channel identity does not match its routing record.".into());
                }

                let destination = AuthorizedDestination {
                    channel,
                    chat: self.chat.clone(),
                    private: self.private.unwrap_or(false),
                    thread: self.thread.clone(),
                };

                destination.validate_channel()?;
                Ok(Destination::Channel(destination))
            }

            Principal::Agent(_) => Err("Agents do not provide a user delivery endpoint.".into()),
        }
    }
}

pub(super) fn scope(source: &Principal, session: &str, chat: &str, thread: Option<&str>) -> String {
    match source {
        Principal::Client(id) if id == client::TUI => format!("tui:{session}"),
        Principal::Client(id) => format!("client:{id}:{session}"),
        Principal::Agent(id) => format!("agent:{id}:{session}"),

        Principal::Channel(id) => {
            let scope = format!("provider:{id}:{chat}");

            match thread {
                Some(thread) => format!("{scope}:thread:{thread}"),
                None => scope,
            }
        }
    }
}

/// Recovers the principal that owns a conversation scope; unknown scopes stay local to the TUI.
pub(super) fn principal(scope: &str) -> Principal {
    let mut parts = scope.splitn(3, ':');

    match (parts.next(), parts.next().filter(|id| !id.is_empty())) {
        (Some("client"), Some(id)) => Principal::Client(id.into()),
        (Some("agent"), Some(id)) => Principal::Agent(id.into()),
        (Some("provider"), Some(id)) => sharing::principal(id),
        _ => Principal::Client(client::TUI.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn origin(source: Option<&str>, integration: &str) -> Origin {
        serde_json::from_value(json!({
            "source":source, "channel":integration, "chat":"7", "session":"conversation",
            "thread":null, "private":true, "sender":null,
        }))
        .unwrap()
    }

    #[test]
    fn every_scope_kind_recovers_its_owning_principal() {
        for source in [
            Principal::Client(client::CLI.into()),
            Principal::Client("desktop".into()),
            Principal::Client(client::TUI.into()),
            Principal::Agent("planner".into()),
            Principal::Channel("telegram".into()),
        ] {
            assert_eq!(principal(&scope(&source, "conversation", "7", Some("4"))), source);
        }

        for unknown in ["global", "", "client:", "unknown:desktop:conversation"] {
            assert_eq!(principal(unknown), Principal::Client(client::TUI.into()), "{unknown}");
        }
    }

    #[test]
    fn explicit_endpoint_types_do_not_depend_on_client_names() {
        let local = origin(Some("clients.desktop"), "desktop");

        assert_eq!(
            local.destination().unwrap(),
            Destination::Session { client: "desktop".into(), session: "conversation".into() }
        );

        assert_eq!(local.scope(), "client:desktop:conversation");

        let channel = origin(Some("channels.telegram"), "telegram");

        assert!(matches!(channel.destination().unwrap(), Destination::Channel(_)));

        let mismatch = origin(Some("clients.desktop"), "telegram");

        assert!(mismatch.destination().is_err());
    }

    #[test]
    fn legacy_routing_fields_round_trip_without_adding_an_endpoint_type() {
        let legacy = origin(None, client::TUI);
        let value = serde_json::to_value(&legacy).unwrap();

        assert_eq!(legacy.source(), Principal::Client(client::TUI.into()));
        assert_eq!(value["channel"], client::TUI);
        assert!(value.get("source").is_none());
        assert_eq!(serde_json::from_value::<Origin>(value).unwrap(), legacy);
    }
}
