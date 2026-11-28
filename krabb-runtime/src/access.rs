use std::fmt;

use serde::de::{self, IntoDeserializer, MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserializer, Serializer};

use super::origin::Origin;
use super::sharing::Principal;
use super::*;

pub(crate) use krabb_core::access::{Elevation, Files, Preset, Role, Visibility};

/// One role's access settings: a bare preset name, or a table that may also set the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoleSetting {
    pub preset: Option<Preset>,
    pub visibility: Option<Visibility>,
    pub files: Option<Files>,
    pub elevation: Option<Elevation>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Table {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preset: Option<Preset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<Visibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    files: Option<Files>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    elevation: Option<Elevation>,
}

impl From<Table> for RoleSetting {
    fn from(table: Table) -> Self {
        Self {
            preset: table.preset,
            visibility: table.visibility,
            files: table.files,
            elevation: table.elevation,
        }
    }
}

impl Serialize for RoleSetting {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let (Some(preset), None, None, None) =
            (self.preset, self.visibility, self.files, self.elevation)
        {
            return preset.serialize(serializer);
        }

        Table {
            preset: self.preset,
            visibility: self.visibility,
            files: self.files,
            elevation: self.elevation,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RoleSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(SettingVisitor)
    }
}

struct SettingVisitor;

impl<'de> Visitor<'de> for SettingVisitor {
    type Value = RoleSetting;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a preset name or a table of preset, visibility, files, and elevation")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<RoleSetting, E> {
        let preset = Preset::deserialize(value.into_deserializer())?;

        Ok(RoleSetting { preset: Some(preset), ..RoleSetting::default() })
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<RoleSetting, A::Error> {
        Table::deserialize(MapAccessDeserializer::new(map)).map(RoleSetting::from)
    }
}

/// The access a request gets. `legacy` means no preset or role is configured anywhere, so
/// Krabb keeps its behavior from before presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Grant {
    pub role: Role,
    pub preset: Preset,
    pub visibility: Visibility,
    pub files: Option<Files>,
    pub elevation: Elevation,
    pub legacy: bool,
}

/// The highest list that names the sender, directly or through one of its platform roles.
pub(crate) fn listed(policy: &ChannelConfig, sender: &str, roles: &[String]) -> Option<Role> {
    let named = |list: &[String]| list.iter().any(|value| value == sender || roles.contains(value));

    if policy.owner.as_deref() == Some(sender) {
        Some(Role::Owner)
    } else if named(&policy.admin) {
        Some(Role::Admin)
    } else if named(&policy.member) {
        Some(Role::Member)
    } else {
        None
    }
}

pub(crate) fn sender(policy: &ChannelConfig, event: &serde_json::Value) -> Option<Role> {
    let sender = event_id(&event["sender"]).unwrap_or_default();

    let roles =
        event["roles"].as_array().into_iter().flatten().filter_map(event_id).collect::<Vec<_>>();

    listed(policy, &sender, &roles)
}

/// Layers built-in role defaults, `[roles]`, then the channel's or client's own `preset` and
/// `roles`; the most specific setting wins. A `preset` is the owner's preset and caps the other
/// roles' presets from the layers below it, and explicit `files` or `elevation` beat the
/// preset's own.
pub(crate) fn resolve(config: &Config, origin: &Origin) -> Grant {
    let (role, scope) = match origin.source() {
        Principal::Client(id) => {
            let role = if client::known(&id) { Role::Owner } else { Role::Member };

            let scope = config.clients.get(&id).map(|client| (client.preset, &client.roles));

            (role, scope)
        }

        Principal::Channel(id) => {
            let scope = config.channels.get(&id);
            let sender = origin.sender.as_deref().unwrap_or_default();

            let role = scope
                .and_then(|channel| listed(channel, sender, &origin.roles))
                .unwrap_or(Role::Guest);

            (role, scope.map(|channel| (channel.preset, &channel.roles)))
        }

        Principal::Agent(_) => (Role::Member, None),
    };

    let global = config.roles.get(&role).copied().unwrap_or_default();
    let local = scope.and_then(|(_, roles)| roles.get(&role).copied()).unwrap_or_default();
    let named = global.preset.unwrap_or(role.preset());

    let preset = local
        .preset
        .or_else(|| scope.and_then(|(preset, _)| preset).map(|base| capped(role, named, base)))
        .or(global.preset)
        .or_else(|| config.preset.map(|base| capped(role, role.preset(), base)))
        .unwrap_or(role.preset());

    Grant {
        role,
        preset,
        visibility: local.visibility.or(global.visibility).unwrap_or(role.visibility()),
        files: local.files.or(global.files).or(preset.files()),
        elevation: local.elevation.or(global.elevation).unwrap_or(preset.elevation()),
        legacy: legacy(config),
    }
}

fn capped(role: Role, below: Preset, base: Preset) -> Preset {
    if role == Role::Owner { base } else { below.min(base) }
}

fn legacy(config: &Config) -> bool {
    config.preset.is_none()
        && config.roles.is_empty()
        && config
            .channels
            .values()
            .all(|channel| channel.preset.is_none() && channel.roles.is_empty())
        && config.clients.values().all(|client| client.preset.is_none() && client.roles.is_empty())
}

#[cfg(test)]
mod tests;
