use super::*;

use serde_json::json;

const GROUP: &str = "
[channels.discord]
allow = ['42']
owner = 'ada'
admin = ['bob', 'moderators']
member = ['cy']
";

fn config(text: &str) -> Config {
    toml::from_str(text).unwrap()
}

fn origin(integration: &str, sender: Option<&str>, roles: &[&str]) -> Origin {
    Origin {
        source: None,
        integration: integration.into(),
        chat: "42".into(),
        session: "session".into(),
        thread: None,
        private: Some(false),
        sender: sender.map(str::to_owned),
        roles: roles.iter().map(|role| (*role).to_owned()).collect(),
    }
}

fn remote(id: &str) -> Origin {
    Origin { source: Some(Principal::Client(id.into())), ..origin(id, None, &[]) }
}

fn agent() -> Origin {
    Origin { source: Some(Principal::Agent("peer".into())), ..origin("peer", None, &[]) }
}

fn setting(
    preset: Option<Preset>,
    visibility: Option<Visibility>,
    files: Option<Files>,
    elevation: Option<Elevation>,
) -> RoleSetting {
    RoleSetting { preset, visibility, files, elevation }
}

#[test]
fn role_settings_accept_the_short_and_the_table_form() {
    let config = config(
        "
preset = 'standard'

[roles]
member = 'standard'
guest = { preset = 'loose', files = 'system', elevation = 'full' }

[channels.discord]
preset = 'hardened'

[channels.discord.roles]
admin = { preset = 'loose', visibility = 'integration' }
owner = { elevation = 'services' }

[clients.tui]
preset = 'loose'
roles = { owner = 'standard' }
",
    );

    assert_eq!(config.preset, Some(Preset::Standard));
    assert_eq!(config.roles[&Role::Member], setting(Some(Preset::Standard), None, None, None));

    assert_eq!(
        config.roles[&Role::Guest],
        setting(Some(Preset::Loose), None, Some(Files::System), Some(Elevation::Full))
    );

    let channel = &config.channels["discord"];

    assert_eq!(channel.preset, Some(Preset::Hardened));

    assert_eq!(
        channel.roles[&Role::Admin],
        setting(Some(Preset::Loose), Some(Visibility::Integration), None, None)
    );

    assert_eq!(channel.roles[&Role::Owner], setting(None, None, None, Some(Elevation::Services)));

    let client = &config.clients["tui"];

    assert_eq!(client.preset, Some(Preset::Loose));
    assert_eq!(client.roles[&Role::Owner], setting(Some(Preset::Standard), None, None, None));
}

#[test]
fn role_settings_round_trip_in_their_shortest_form() {
    let text = "
preset = 'loose'

[roles]
member = 'standard'
guest = { visibility = 'all' }

[channels.discord]
preset = 'hardened'
roles = { admin = { preset = 'loose', files = 'home' } }

[clients.acp]
roles = { member = 'chat' }
";

    let parsed = config(text);
    let written = toml::to_string(&parsed).unwrap();
    let reparsed = config(&written);

    assert_eq!(reparsed.preset, parsed.preset);
    assert_eq!(reparsed.roles, parsed.roles);
    assert_eq!(reparsed.channels["discord"].preset, parsed.channels["discord"].preset);
    assert_eq!(reparsed.channels["discord"].roles, parsed.channels["discord"].roles);
    assert_eq!(reparsed.clients["acp"].roles, parsed.clients["acp"].roles);
    assert!(written.contains("member = \"standard\""), "{written}");
    assert!(written.contains("visibility = \"all\""), "{written}");
    assert_eq!(toml::to_string(&reparsed).unwrap(), written);
}

#[test]
fn untouched_configs_serialize_without_access_keys() {
    let default = toml::to_string(&Config::default()).unwrap();

    for text in ["", GROUP, "[clients.tui]\ntools = true\n", default.as_str()] {
        let written = toml::to_string(&config(text)).unwrap();

        assert!(!written.contains("preset"), "{written}");
        assert!(!written.contains("roles"), "{written}");
        assert_eq!(toml::to_string(&config(&written)).unwrap(), written);
    }

    assert_eq!(toml::to_string(&config(&default)).unwrap(), default);
}

#[test]
fn unknown_roles_and_values_fail_clearly() {
    let cases = [
        ("preset = 'open'", "unknown variant `open`"),
        ("[roles]\nroot = 'loose'", "unknown variant `root`"),
        ("[roles]\nmember = 'open'", "unknown variant `open`"),
        ("[roles]\nmember = { files = 'disk' }", "unknown variant `disk`"),
        ("[roles]\nmember = { elevation = 'sudo' }", "unknown variant `sudo`"),
        ("[roles]\nmember = { visibility = 'world' }", "unknown variant `world`"),
        ("[roles]\nmember = { data = 'all' }", "unknown field `data`"),
        ("[roles]\nmember = 3", "a preset name or a table"),
        ("[channels.discord]\npreset = 'Loose'", "unknown variant `Loose`"),
        ("[channels.discord.roles]\nguest = 'sudo'", "unknown variant `sudo`"),
        ("[clients.tui.roles]\nnobody = 'chat'", "unknown variant `nobody`"),
    ];

    for (text, expected) in cases {
        let message = toml::from_str::<Config>(text).unwrap_err().to_string();

        assert!(message.contains(expected), "{text}: {message}");
    }
}

#[test]
fn local_clients_are_owners_and_remote_callers_are_members() {
    let config = config("");

    let cases = [
        (origin(client::TUI, None, &[]), Role::Owner, Preset::Standard),
        (origin(client::CLI, None, &[]), Role::Owner, Preset::Standard),
        (remote("acp"), Role::Member, Preset::Hardened),
        (remote("ag-ui"), Role::Member, Preset::Hardened),
        (agent(), Role::Member, Preset::Hardened),
    ];

    for (origin, role, preset) in cases {
        let grant = resolve(&config, &origin);

        assert_eq!((grant.role, grant.preset), (role, preset), "{}", origin.integration);
        assert!(grant.legacy);
    }
}

#[test]
fn channel_senders_take_the_highest_list_that_names_them() {
    let config = config(GROUP);

    let cases = [
        (Some("ada"), &[][..], Role::Owner),
        (Some("bob"), &[], Role::Admin),
        (Some("zed"), &["moderators"][..], Role::Admin),
        (Some("cy"), &[], Role::Member),
        (Some("zed"), &[], Role::Guest),
        (None, &[], Role::Guest),
    ];

    for (sender, roles, role) in cases {
        assert_eq!(resolve(&config, &origin("discord", sender, roles)).role, role, "{sender:?}");
    }

    assert_eq!(resolve(&config, &origin("slack", Some("ada"), &[])).role, Role::Guest);
}

#[test]
fn built_in_defaults_need_no_configuration() {
    let config = config(GROUP);

    let cases = [
        ("ada", Preset::Standard, Some(Files::Workspace)),
        ("bob", Preset::Standard, Some(Files::Workspace)),
        ("cy", Preset::Hardened, Some(Files::Folder)),
        ("zed", Preset::Chat, None),
    ];

    for (sender, preset, files) in cases {
        let grant = resolve(&config, &origin("discord", Some(sender), &[]));

        assert_eq!(grant.preset, preset, "{sender}");
        assert_eq!(grant.files, files, "{sender}");
        assert_eq!(grant.visibility, Visibility::Chat, "{sender}");
        assert_eq!(grant.elevation, Elevation::None, "{sender}");
        assert!(grant.legacy, "{sender}");
    }
}

#[test]
fn the_most_specific_setting_wins() {
    let cases = [
        ("preset = 'loose'", "", "ada", Preset::Loose),
        ("preset = 'loose'", "", "bob", Preset::Standard),
        ("preset = 'hardened'", "", "bob", Preset::Hardened),
        ("preset = 'hardened'", "", "zed", Preset::Chat),
        ("[roles]\nmember = 'loose'", "", "cy", Preset::Loose),
        ("[roles]\nguest = 'standard'", "", "zed", Preset::Standard),
        ("preset = 'hardened'\n[roles]\nowner = 'loose'", "", "ada", Preset::Loose),
        ("[roles]\nmember = 'loose'", "preset = 'hardened'", "cy", Preset::Hardened),
        ("[roles]\nowner = 'loose'", "preset = 'hardened'", "ada", Preset::Hardened),
        ("preset = 'hardened'", "preset = 'loose'", "bob", Preset::Standard),
        ("preset = 'hardened'", "preset = 'loose'", "ada", Preset::Loose),
        ("", "preset = 'hardened'\nroles = { member = 'loose' }", "cy", Preset::Loose),
        ("[roles]\nadmin = 'chat'", "roles = { member = 'loose' }", "bob", Preset::Chat),
    ];

    for (global, local, sender, preset) in cases {
        let config = config(&format!("{global}\n{GROUP}{local}\n"));
        let grant = resolve(&config, &origin("discord", Some(sender), &[]));

        assert_eq!(grant.preset, preset, "{global} | {local} | {sender}");
        assert!(!grant.legacy, "{global} | {local}");
    }
}

#[test]
fn explicit_files_elevation_and_visibility_beat_the_preset() {
    let config = config(&format!(
        "
[roles]
member = {{ preset = 'loose', files = 'workspace', visibility = 'integration' }}
guest = {{ preset = 'loose', files = 'system', elevation = 'full' }}
{GROUP}
[channels.discord.roles]
member = {{ elevation = 'services' }}
admin = 'loose'
"
    ));

    let grant = |sender| resolve(&config, &origin("discord", Some(sender), &[]));

    let member = grant("cy");

    assert_eq!(member.preset, Preset::Loose);
    assert_eq!(member.files, Some(Files::Workspace));
    assert_eq!(member.elevation, Elevation::Services);
    assert_eq!(member.visibility, Visibility::Integration);

    let guest = grant("zed");

    assert_eq!((guest.files, guest.elevation), (Some(Files::System), Elevation::Full));

    let admin = grant("bob");

    assert_eq!((admin.files, admin.elevation), (Some(Files::Home), Elevation::Packages));
    assert_eq!(admin.visibility, Visibility::Chat);
}

#[test]
fn clients_override_roles_in_their_own_table() {
    let config = config(
        "
[roles]
owner = 'hardened'
member = 'standard'

[clients.tui]
preset = 'loose'

[clients.acp]
preset = 'hardened'
roles = { member = { visibility = 'integration' } }
",
    );

    assert_eq!(resolve(&config, &origin(client::TUI, None, &[])).preset, Preset::Loose);
    assert_eq!(resolve(&config, &origin(client::CLI, None, &[])).preset, Preset::Hardened);

    let caller = resolve(&config, &remote("acp"));

    assert_eq!(caller.preset, Preset::Hardened);
    assert_eq!(caller.visibility, Visibility::Integration);
    assert_eq!(resolve(&config, &agent()).preset, Preset::Standard);
}

#[test]
fn any_access_setting_anywhere_leaves_legacy_mode() {
    let cases = [
        ("", true),
        (GROUP, true),
        ("preset = 'standard'", false),
        ("[roles]\nguest = 'chat'", false),
        ("[channels.discord]\npreset = 'loose'", false),
        ("[channels.discord.roles]\nguest = 'chat'", false),
        ("[clients.tui]\npreset = 'loose'", false),
        ("[clients.acp.roles]\nmember = { files = 'folder' }", false),
    ];

    for (text, legacy) in cases {
        let config = config(text);

        assert_eq!(resolve(&config, &origin(client::TUI, None, &[])).legacy, legacy, "{text}");
    }
}

mod before {
    use super::super::{ChannelConfig, event_id, mentions, same_id};

    fn listed(policy: &ChannelConfig, event: &serde_json::Value) -> bool {
        let sender = event_id(&event["sender"]).unwrap_or_default();

        let roles = event["roles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(event_id)
            .collect::<Vec<_>>();

        policy.owner.as_deref() == Some(sender.as_str())
            || policy
                .admin
                .iter()
                .any(|value| value == &sender || roles.iter().any(|role| role == value))
            || policy
                .member
                .iter()
                .any(|value| value == &sender || roles.iter().any(|role| role == value))
    }

    pub fn allowed(policy: &ChannelConfig, event: &serde_json::Value, chat: &str) -> bool {
        let private = event["private"] == true;

        if !private && policy.allow.is_empty() {
            return false;
        }

        if !private && !policy.allow.iter().any(|value| value == chat) {
            return false;
        }

        if let Some(marker) = policy.mention.as_deref()
            && event["addressed"] != true
            && !event["text"].as_str().is_some_and(|text| mentions(text, marker))
        {
            return false;
        }

        if !policy.topic.is_empty()
            && !policy.topic.iter().any(|value| same_id(&event["topic"], value))
        {
            return false;
        }

        if !policy.thread.is_empty()
            && !policy.thread.iter().any(|value| same_id(&event["thread"], value))
        {
            return false;
        }

        let restricted =
            policy.owner.is_some() || !policy.admin.is_empty() || !policy.member.is_empty();

        !restricted || listed(policy, event)
    }

    pub fn tools_allowed(
        policy: &ChannelConfig,
        event: &serde_json::Value,
        chat: &str,
        approvals: bool,
    ) -> bool {
        if !approvals || policy.tools != Some(true) || !allowed(policy, event, chat) {
            return false;
        }

        event["private"] != true
            || policy.allow.iter().any(|value| value == chat)
            || listed(policy, event)
    }

    pub fn schedule_allowed(policy: &ChannelConfig, event: &serde_json::Value, chat: &str) -> bool {
        if policy.tools != Some(true) || !allowed(policy, event, chat) {
            return false;
        }

        event["private"] != true
            || policy.allow.iter().any(|value| value == chat)
            || listed(policy, event)
    }
}

fn events() -> Vec<serde_json::Value> {
    let senders = [json!("ada"), json!("bob"), json!("cy"), json!("zed"), json!(7), json!(null)];
    let roles = [json!([]), json!(["moderators"]), json!(["cy", 3])];
    let places = [json!({}), json!({"topic": "7"}), json!({"thread": 9})];
    let texts = [json!(null), json!("hi @krabb"), json!("hi")];
    let mut events = Vec::new();

    for sender in &senders {
        for role in &roles {
            for place in &places {
                for text in &texts {
                    for (private, addressed) in [(false, false), (false, true), (true, false)] {
                        let mut event = json!({
                            "sender": sender,
                            "roles": role,
                            "private": private,
                            "text": text,
                            "addressed": addressed,
                        });

                        for (key, value) in place.as_object().into_iter().flatten() {
                            event[key] = value.clone();
                        }

                        events.push(event);
                    }
                }
            }
        }
    }

    events
}

#[test]
fn shared_role_matching_keeps_every_channel_decision() {
    let policies = [
        "",
        "allow = ['42']",
        "allow = ['42']\ntools = true",
        "allow = ['42']\ntools = true\nshell = true",
        "allow = ['42']\ntools = true\nowner = 'ada'",
        "allow = ['42']\ntools = true\nowner = 'ada'\nadmin = ['bob', 'moderators']",
        "allow = ['42']\ntools = true\nmember = ['cy']",
        "allow = ['42']\ntools = true\nshell = true\nowner = 'ada'\nadmin = ['bob']\nmember = ['cy']",
        "allow = ['42']\ntools = true\nmention = '@krabb'\nowner = 'ada'",
        "allow = ['42']\ntools = true\ntopic = ['7']\nmember = ['cy']",
        "allow = ['42']\ntools = true\nthread = ['9']\nadmin = ['bob']",
        "tools = true\nshell = true\nowner = 'ada'\nmember = ['cy']",
    ];

    let events = events();
    let mut checked = 0;

    for text in policies {
        let policy: ChannelConfig = toml::from_str(text).unwrap();

        for event in &events {
            for chat in ["42", "43"] {
                let talks = before::allowed(&policy, event, chat);
                let schedules = before::schedule_allowed(&policy, event, chat);

                assert_eq!(allowed(&policy, event, chat), talks, "{text} {event}");
                assert_eq!(schedule_allowed(&policy, event, chat), schedules, "{text} {event}");

                for approvals in [false, true] {
                    let tools = before::tools_allowed(&policy, event, chat, approvals);

                    assert_eq!(tools_allowed(&policy, event, chat, approvals), tools, "{event}");

                    for enabled in [false, true] {
                        let shell = enabled || (policy.shell == Some(true) && tools);

                        assert_eq!(
                            channel_shell_allowed(&policy, event, chat, approvals, enabled),
                            shell,
                            "{text} {event}"
                        );
                    }
                }

                checked += 1;
            }
        }
    }

    assert_eq!(checked, 12 * 6 * 3 * 3 * 3 * 3 * 2);
}

#[test]
fn today_decisions_for_each_role_and_chat_kind() {
    let policy: ChannelConfig = toml::from_str(
        "allow = ['42']\ntools = true\nmention = 'krabb'\ntopic = ['7']\nowner = 'ada'\n\
         admin = ['bob']\nmember = ['cy']",
    )
    .unwrap();

    let cases = [
        ("ada", false, "krabb", "7", true, true),
        ("bob", false, "krabb", "7", true, true),
        ("cy", false, "krabb", "7", true, true),
        ("zed", false, "krabb", "7", false, false),
        ("ada", true, "krabb", "7", true, true),
        ("cy", true, "krabb", "7", true, true),
        ("zed", true, "krabb", "7", false, false),
        ("ada", false, "hello", "7", false, false),
        ("ada", false, "krabb", "8", false, false),
    ];

    for (sender, private, text, topic, talks, tools) in cases {
        let event = json!({"sender": sender, "private": private, "text": text, "topic": topic});

        assert_eq!(allowed(&policy, &event, "42"), talks, "{event}");
        assert_eq!(tools_allowed(&policy, &event, "42", true), tools, "{event}");
        assert_eq!(schedule_allowed(&policy, &event, "42"), tools, "{event}");
        assert!(!tools_allowed(&policy, &event, "42", false), "{event}");
        assert!(!allowed(&policy, &event, "43") || private, "{event}");
    }
}

#[test]
fn presets_classify_tool_calls_and_shell_commands() {
    let shell = |command: &str| action("shell_run", &json!({"command": command}));

    assert_eq!(action("file_read", &json!({"path": "a"})), Action::Read);
    assert_eq!(action("web_fetch", &json!({"url": "https://example.com"})), Action::Fetch);
    assert_eq!(action("file_write", &json!({"path": "a", "content": "b"})), Action::Write);
    assert_eq!(action("message_send", &json!({})), Action::Message);
    assert_eq!(action(settings::TOOL, &json!({})), Action::Settings);
    assert_eq!(action("schedule_propose", &json!({})), Action::Schedule);
    assert_eq!(action("mcp_unknown_tool", &json!({})), Action::Shell);
    assert_eq!(shell("ls -la"), Action::Inspect);
    assert_eq!(shell("rm -rf build"), Action::Delete);
    assert_eq!(shell("apt install htop"), Action::Install);
    assert_eq!(shell("sudo ls"), Action::Elevate);
    assert_eq!(shell("touch a"), Action::Shell);
    assert_eq!(shell("git status"), Action::Shell);
}

#[test]
fn the_gate_follows_the_preset_and_an_approval_mode_of_off_narrows_it() {
    let legacy = config("");
    let presets = config("preset = 'standard'");
    let owner = resolve(&presets, &remote(krabb_core::client::TUI));
    let write = json!({"path": "a", "content": "b"});

    assert_eq!(gate(&legacy, &[], &owner, "tui", "file_write", &write, ApprovalMode::Prompt), None);
    assert_eq!(owner.preset, Preset::Standard);

    let run = gate(&presets, &[], &owner, "tui", "file_write", &write, ApprovalMode::Prompt);
    let ask = gate(
        &presets,
        &[],
        &owner,
        "tui",
        "shell_run",
        &json!({"command": "rm x"}),
        ApprovalMode::Auto,
    );
    let off = gate(&presets, &[], &owner, "tui", "file_write", &write, ApprovalMode::Off);
    let read =
        gate(&presets, &[], &owner, "tui", "file_read", &json!({"path": "a"}), ApprovalMode::Off);
    let settings =
        gate(&presets, &[], &owner, "tui", settings::TOOL, &json!({}), ApprovalMode::Auto);

    assert_eq!(run, Some(Verdict::Run));
    assert_eq!(ask, Some(Verdict::Ask));
    assert_eq!(off, Some(Verdict::Deny));
    assert_eq!(read, Some(Verdict::Run));
    assert_eq!(settings, Some(Verdict::Ask));

    let guest = Grant { preset: Preset::Chat, ..owner };

    assert_eq!(
        gate(&presets, &[], &guest, "tui", "file_read", &json!({}), ApprovalMode::Auto),
        Some(Verdict::Deny)
    );
}

#[test]
fn a_preset_turns_tools_on_for_capable_roles_and_false_still_narrows() {
    let text = format!("preset = 'standard'\n{GROUP}");
    let presets = config(&text);
    let policy = &presets.channels["discord"];
    let event = |sender: &str| json!({"sender": sender, "chat": "42", "private": false});

    let owner = effective(&presets, "discord", policy, &event("ada"));
    let stranger = effective(&presets, "discord", policy, &event("eve"));

    assert_eq!((owner.tools, owner.shell), (Some(true), Some(true)));
    assert_eq!((stranger.tools, stranger.shell), (Some(false), Some(false)));

    let narrowed = config(&format!("{text}tools = true\nshell = false\n"));
    let narrowed = effective(&narrowed, "discord", &narrowed.channels["discord"], &event("ada"));

    assert_eq!((narrowed.tools, narrowed.shell), (Some(true), Some(false)));

    let legacy = config(GROUP);
    let unchanged = effective(&legacy, "discord", &legacy.channels["discord"], &event("ada"));

    assert_eq!((unchanged.tools, unchanged.shell), (None, None));
}

#[test]
fn clients_follow_the_preset_unless_set() {
    let presets = config("preset = 'standard'");
    let legacy = config("");

    assert!(client(&presets, None, false));
    assert!(!client(&presets, Some(false), true));
    assert!(!client(&legacy, None, false));
    assert!(client(&legacy, None, true));
    assert!(client(&legacy, Some(true), false));
}

#[test]
fn only_the_requesters_role_or_a_higher_one_approves() {
    assert!(approves(Role::Owner, Some(Role::Admin)));
    assert!(approves(Role::Admin, Some(Role::Admin)));
    assert!(!approves(Role::Member, Some(Role::Admin)));
    assert!(!approves(Role::Guest, Some(Role::Member)));
    assert!(approves(Role::Guest, None));
}

#[test]
fn admin_commands_need_their_level_and_always_ask_within_it() {
    let shell = |command: &str| json!({"command": command});

    assert_eq!(needs("sudo apt install htop"), Some(Elevation::Packages));
    assert_eq!(needs("sudo systemctl restart nginx"), Some(Elevation::Services));
    assert_eq!(needs("sudo rm -rf /var/cache/x"), Some(Elevation::Full));
    assert_eq!(needs("ls -la"), None);
    assert_eq!(needs("apt list --installed"), None);

    let expected = if superuser() { Some(Elevation::Packages) } else { None };

    assert_eq!(needs("apt install htop"), expected);

    let presets = config("preset = 'standard'");
    let owner = resolve(&presets, &remote(krabb_core::client::TUI));
    let install = shell("sudo apt install htop");

    assert_eq!(owner.elevation, Elevation::None);
    assert_eq!(
        gate(&presets, &[], &owner, "tui", "shell_run", &install, ApprovalMode::Auto),
        Some(Verdict::Deny)
    );

    let text = refusal(&owner, "shell_run", &install);

    assert!(text.contains("`packages` admin level") && text.ends_with("`sudo apt install htop`"));

    let packages = Grant { elevation: Elevation::Packages, ..owner };
    let loose = Grant { preset: Preset::Loose, elevation: Elevation::Full, ..owner };

    assert_eq!(
        gate(&presets, &[], &packages, "tui", "shell_run", &install, ApprovalMode::Auto),
        Some(Verdict::Ask)
    );
    assert_eq!(
        gate(&presets, &[], &loose, "tui", "shell_run", &install, ApprovalMode::Auto),
        Some(Verdict::Ask)
    );

    let services = shell("sudo systemctl restart nginx");

    assert_eq!(
        gate(&presets, &[], &packages, "tui", "shell_run", &services, ApprovalMode::Auto),
        Some(Verdict::Deny)
    );
    assert_eq!(
        gate(&presets, &[], &loose, "tui", "shell_run", &services, ApprovalMode::Auto),
        Some(Verdict::Ask)
    );
    assert!(refusal(&owner, "file_read", &json!({})).starts_with("The standard preset"));
}
