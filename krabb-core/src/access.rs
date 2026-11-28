use std::fmt;

use serde::{Deserialize, Serialize};

/// Who sent a request, from IDs the platform verifies; later variants outrank earlier ones.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Guest,
    Member,
    Admin,
    Owner,
}

/// What a role may do without asking; later variants allow more.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Chat,
    Hardened,
    Standard,
    Loose,
}

/// Which conversations' data a role may see.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Chat,
    Integration,
    All,
}

/// How far the file tools and the shell reach.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Files {
    Folder,
    Workspace,
    Home,
    System,
}

/// Which admin rights approved commands may use.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Elevation {
    None,
    Packages,
    Services,
    Full,
}

/// A class of tool call that a preset decides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    Read,
    Fetch,
    /// A write inside the role's file reach.
    Write,
    /// A write outside the role's file reach.
    WriteOutside,
    Delete,
    /// A shell command the host recognizes as read-only.
    Inspect,
    Shell,
    Install,
    Elevate,
    Settings,
    Message,
    Schedule,
}

impl Action {
    pub const ALL: [Self; 12] = [
        Self::Read,
        Self::Fetch,
        Self::Write,
        Self::WriteOutside,
        Self::Delete,
        Self::Inspect,
        Self::Shell,
        Self::Install,
        Self::Elevate,
        Self::Settings,
        Self::Message,
        Self::Schedule,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verdict {
    Run,
    Ask,
    Deny,
}

impl Role {
    pub const ALL: [Self; 4] = [Self::Guest, Self::Member, Self::Admin, Self::Owner];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Member => "member",
            Self::Admin => "admin",
            Self::Owner => "owner",
        }
    }

    pub const fn preset(self) -> Preset {
        match self {
            Self::Guest => Preset::Chat,
            Self::Member => Preset::Hardened,
            Self::Admin | Self::Owner => Preset::Standard,
        }
    }

    pub const fn visibility(self) -> Visibility {
        Visibility::Chat
    }
}

impl Preset {
    pub const ALL: [Self; 4] = [Self::Chat, Self::Hardened, Self::Standard, Self::Loose];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Hardened => "hardened",
            Self::Standard => "standard",
            Self::Loose => "loose",
        }
    }

    pub const fn decide(self, action: Action) -> Verdict {
        match (self, action) {
            (Self::Chat, _) => Verdict::Deny,
            (_, Action::Install | Action::Elevate) => Verdict::Ask,
            (Self::Loose, _) => Verdict::Run,
            (_, Action::Read | Action::Fetch) => Verdict::Run,
            (Self::Standard, Action::Write | Action::Inspect) => Verdict::Run,
            _ => Verdict::Ask,
        }
    }

    pub const fn files(self) -> Option<Files> {
        match self {
            Self::Chat => None,
            Self::Hardened => Some(Files::Folder),
            Self::Standard => Some(Files::Workspace),
            Self::Loose => Some(Files::Home),
        }
    }

    pub const fn elevation(self) -> Elevation {
        match self {
            Self::Loose => Elevation::Packages,
            Self::Chat | Self::Hardened | Self::Standard => Elevation::None,
        }
    }
}

impl Visibility {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Integration => "integration",
            Self::All => "all",
        }
    }
}

impl Files {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Workspace => "workspace",
            Self::Home => "home",
            Self::System => "system",
        }
    }
}

impl Elevation {
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Packages => "packages",
            Self::Services => "services",
            Self::Full => "full",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl fmt::Display for Files {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl fmt::Display for Elevation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use Action::*;
    use Verdict::{Ask, Deny, Run};

    #[test]
    fn every_preset_decides_every_action_as_the_table_says() {
        let table = [
            (Read, [Deny, Run, Run, Run]),
            (Fetch, [Deny, Run, Run, Run]),
            (Write, [Deny, Ask, Run, Run]),
            (WriteOutside, [Deny, Ask, Ask, Run]),
            (Delete, [Deny, Ask, Ask, Run]),
            (Inspect, [Deny, Ask, Run, Run]),
            (Shell, [Deny, Ask, Ask, Run]),
            (Install, [Deny, Ask, Ask, Ask]),
            (Elevate, [Deny, Ask, Ask, Ask]),
            (Settings, [Deny, Ask, Ask, Run]),
            (Message, [Deny, Ask, Ask, Run]),
            (Schedule, [Deny, Ask, Ask, Run]),
        ];

        assert_eq!(table.map(|(action, _)| action), Action::ALL);

        for (action, verdicts) in table {
            for (preset, verdict) in Preset::ALL.into_iter().zip(verdicts) {
                assert_eq!(preset.decide(action), verdict, "{preset} {action:?}");
            }
        }
    }

    #[test]
    fn presets_imply_their_file_reach_and_elevation() {
        let table = [
            (Preset::Chat, None, Elevation::None),
            (Preset::Hardened, Some(Files::Folder), Elevation::None),
            (Preset::Standard, Some(Files::Workspace), Elevation::None),
            (Preset::Loose, Some(Files::Home), Elevation::Packages),
        ];

        for (preset, files, elevation) in table {
            assert_eq!(preset.files(), files, "{preset}");
            assert_eq!(preset.elevation(), elevation, "{preset}");
        }
    }

    #[test]
    fn roles_default_to_the_epic_presets_and_chat_visibility() {
        let presets = [Preset::Chat, Preset::Hardened, Preset::Standard, Preset::Standard];

        for (role, preset) in Role::ALL.into_iter().zip(presets) {
            assert_eq!(role.preset(), preset, "{role}");
            assert_eq!(role.visibility(), Visibility::Chat, "{role}");
        }

        assert!(Role::Guest < Role::Member && Role::Member < Role::Admin);
        assert!(Role::Admin < Role::Owner);
        assert!(Preset::Chat < Preset::Hardened && Preset::Standard < Preset::Loose);
    }

    #[test]
    fn names_display_and_serialize_in_lowercase() {
        let mut names = Vec::new();

        names.extend(Role::ALL.map(|value| (value.to_string(), serde_json::json!(value))));
        names.extend(Preset::ALL.map(|value| (value.to_string(), serde_json::json!(value))));

        names.extend(
            [Visibility::Chat, Visibility::Integration, Visibility::All]
                .map(|value| (value.to_string(), serde_json::json!(value))),
        );

        names.extend(
            [Files::Folder, Files::Workspace, Files::Home, Files::System]
                .map(|value| (value.to_string(), serde_json::json!(value))),
        );

        names.extend(
            [Elevation::None, Elevation::Packages, Elevation::Services, Elevation::Full]
                .map(|value| (value.to_string(), serde_json::json!(value))),
        );

        let expected = [
            "guest",
            "member",
            "admin",
            "owner",
            "chat",
            "hardened",
            "standard",
            "loose",
            "chat",
            "integration",
            "all",
            "folder",
            "workspace",
            "home",
            "system",
            "none",
            "packages",
            "services",
            "full",
        ];

        assert_eq!(names.len(), expected.len());

        for ((display, json), name) in names.into_iter().zip(expected) {
            assert_eq!(display, name);
            assert_eq!(json, name);
        }

        assert_eq!(serde_json::from_str::<Files>("\"home\"").unwrap(), Files::Home);
        assert!(serde_json::from_str::<Preset>("\"Loose\"").is_err());
        assert!(serde_json::from_str::<Role>("\"root\"").is_err());
    }
}
