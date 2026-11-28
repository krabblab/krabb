use super::*;

fn config(text: &str) -> Config {
    toml::from_str(text).unwrap()
}

#[test]
fn settings_land_in_the_right_section_and_unknown_places_fail() {
    let config = config("[channels.telegram]\nowner = 'ada'\n");

    assert_eq!(key(&config, "global", Role::Admin).unwrap(), "roles.admin");
    assert_eq!(key(&config, "telegram", Role::Member).unwrap(), "channels.telegram.roles.member");
    assert_eq!(key(&config, "tui", Role::Owner).unwrap(), "clients.tui.roles.owner");
    assert!(
        key(&config, "carrier-pigeon", Role::Guest).unwrap_err().to_string().contains("No channel")
    );
}

#[test]
fn more_than_the_default_states_its_risk_once() {
    assert_eq!(risk(Role::Admin, Preset::Standard, None), None);
    assert_eq!(risk(Role::Member, Preset::Hardened, Some(Elevation::None)), None);

    let loose = risk(Role::Guest, Preset::Loose, None).unwrap();
    let root = risk(Role::Owner, Preset::Standard, Some(Elevation::Full)).unwrap();

    assert!(loose.contains("guest") && loose.contains("default (chat)"));
    assert!(root.contains("full admin rights"));
}

#[test]
fn the_list_shows_every_role_in_every_place() {
    let config = config("preset = 'standard'\n[channels.telegram.roles]\nadmin = 'loose'\n");
    let text = listing(&config, &places(&config));

    assert!(
        text.starts_with("Access by role\nglobal\n  owner   standard  admin none      sees chat")
    );
    assert!(text.contains("channels.telegram\n  owner"));
    assert!(text.contains("  admin   loose     admin packages  sees chat"));
    assert!(text.contains("  guest   chat"));

    let legacy = Config::default();

    assert!(listing(&legacy, &places(&legacy)).contains("krabb init preset"));
}

#[test]
fn names_are_checked_before_anything_is_saved() {
    assert!(
        parse::<Role>("root", "role", "owner")
            .unwrap_err()
            .to_string()
            .contains("Unknown role root")
    );
    assert_eq!(parse::<Preset>("loose", "preset", "").unwrap(), Preset::Loose);
}

#[test]
fn a_set_role_is_saved_and_read_back() {
    let root = crate::tests::test_root("access-set");

    crate::home::init_at(&root).unwrap();

    let set = |role: &str, preset: &str, elevation: Option<&str>| Command::Set {
        place: "global".into(),
        role: role.into(),
        preset: preset.into(),
        elevation: elevation.map(str::to_owned),
        yes: true,
    };

    run(set("member", "standard", None), &root, true).unwrap();
    run(set("admin", "standard", Some("services")), &root, true).unwrap();

    let config = crate::load_config_at(&root).unwrap();

    assert_eq!(layered(&config, Role::Member, None).preset, Preset::Standard);
    assert_eq!(layered(&config, Role::Admin, None).elevation, Elevation::Services);

    let _ = std::fs::remove_dir_all(root);
}
