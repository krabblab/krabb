//! How plugins that declare the same system capability combine. Krabb fixes the composition of
//! each capability; a plugin declares capabilities but cannot change how they combine.

use serde::Serialize;

use crate::Capability;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Composition {
    /// Every plugin contributes at once, such as channels, tools, and clients.
    Stack,
    /// One plugin serves each conversation, chosen by the model selection.
    Select,
    /// One primary plugin takes writes and commands; others may be added for reads.
    Primary,
}

impl Composition {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stack => "stack",
            Self::Select => "select",
            Self::Primary => "primary",
        }
    }
}

impl Capability {
    pub const ALL: [Self; 14] = [
        Self::Model,
        Self::Context,
        Self::Vision,
        Self::Channel,
        Self::Store,
        Self::Memory,
        Self::Scheduler,
        Self::Tool,
        Self::Mcp,
        Self::Speech,
        Self::Client,
        Self::Resource,
        Self::Agent,
        Self::Runner,
    ];

    pub const fn name(&self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Context => "context",
            Self::Vision => "vision",
            Self::Channel => "channel",
            Self::Store => "store",
            Self::Memory => "memory",
            Self::Scheduler => "scheduler",
            Self::Tool => "tool",
            Self::Mcp => "mcp",
            Self::Speech => "speech",
            Self::Client => "client",
            Self::Resource => "resource",
            Self::Agent => "agent",
            Self::Runner => "runner",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|capability| capability.name() == name)
    }

    /// Context and vision are features of the model plugin a conversation selects, and the host
    /// runs extended operations through one runner, so those follow the model or a primary.
    pub const fn composition(&self) -> Composition {
        match self {
            Self::Model | Self::Context | Self::Vision => Composition::Select,

            Self::Store | Self::Memory | Self::Scheduler | Self::Speech | Self::Runner => {
                Composition::Primary
            }

            Self::Channel
            | Self::Tool
            | Self::Mcp
            | Self::Client
            | Self::Resource
            | Self::Agent => Composition::Stack,
        }
    }

    /// Whether additional plugins can join the primary: memory reads merge and each schedule
    /// stays with the scheduler that holds it. Other primary capabilities have one provider.
    pub const fn additional(&self) -> bool {
        matches!(self, Self::Memory | Self::Scheduler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_has_a_fixed_composition() {
        let table = Capability::ALL
            .iter()
            .map(|capability| (capability.name(), capability.composition().name()))
            .collect::<Vec<_>>();

        assert_eq!(
            table,
            [
                ("model", "select"),
                ("context", "select"),
                ("vision", "select"),
                ("channel", "stack"),
                ("store", "primary"),
                ("memory", "primary"),
                ("scheduler", "primary"),
                ("tool", "stack"),
                ("mcp", "stack"),
                ("speech", "primary"),
                ("client", "stack"),
                ("resource", "stack"),
                ("agent", "stack"),
                ("runner", "primary"),
            ]
        );
    }

    #[test]
    fn names_round_trip_and_match_the_wire_format() {
        for capability in Capability::ALL {
            assert_eq!(Capability::parse(capability.name()), Some(capability.clone()));

            assert_eq!(
                serde_json::to_value(&capability).unwrap(),
                serde_json::json!(capability.name())
            );
        }

        assert_eq!(Capability::parse("classifier"), None);
    }

    #[test]
    fn only_memory_and_scheduler_take_additional_plugins() {
        let additional = Capability::ALL
            .iter()
            .filter(|capability| capability.additional())
            .map(Capability::name)
            .collect::<Vec<_>>();

        assert_eq!(additional, ["memory", "scheduler"]);

        assert!(
            Capability::ALL
                .iter()
                .filter(|capability| capability.additional())
                .all(|capability| capability.composition() == Composition::Primary)
        );
    }

    #[test]
    fn a_plugin_hello_cannot_carry_a_composition() {
        let hello = serde_json::json!({
            "protocol": {"major": 0, "minor": 2},
            "id": "other",
            "version": "1",
            "capabilities": [{"memory": "stack"}]
        });

        assert!(serde_json::from_value::<crate::types::Hello>(hello).is_err());
    }
}
