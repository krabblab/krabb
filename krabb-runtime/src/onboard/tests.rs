use super::*;
use std::collections::BTreeSet;

fn args(flags: &[&str]) -> InitArgs {
    use clap::Parser;

    let arguments = ["krabb", "init"].iter().chain(flags);

    match crate::cli::Cli::try_parse_from(arguments).unwrap().command {
        crate::cli::Command::Init(args) => args,
        command => panic!("parsed {command:?}"),
    }
}

fn guide(root: PathBuf) -> Guide {
    Guide { root, color: false, number: 0, named: Vec::new(), interactive: false }
}

#[test]
fn steps_run_in_order_from_names_flags_or_the_terminal() {
    assert_eq!(plan(&args(&[]), true).unwrap(), ALL);
    assert_eq!(plan(&args(&[]), false).unwrap(), [Step::Home]);

    let named = plan(&args(&["service", "home", "home"]), false).unwrap();

    assert_eq!(named, [Step::Home, Step::Service]);

    let flagged = plan(&args(&["home", "--channel", "none", "--provider", "ollama"]), false);

    assert_eq!(flagged.unwrap(), [Step::Home, Step::Provider, Step::Channel]);
    assert_eq!(plan(&args(&["--provider", "openai"]), true).unwrap(), [Step::Provider]);
}

#[test]
fn without_a_terminal_each_question_needs_its_flag() {
    let provider = plan(&args(&["provider"]), false).unwrap_err();
    let channel = plan(&args(&["channel"]), false).unwrap_err();
    let hatch = plan(&args(&["hatch"]), false).unwrap_err();

    assert!(provider.contains("--provider openai"));
    assert!(channel.contains("--channel telegram"));
    assert!(hatch.contains("`krabb hatch --yes`"));
    assert!(plan(&args(&["provider", "hatch"]), true).is_ok());

    let unknown = plan(&args(&["--provider", "skynet"]), true).unwrap_err();

    assert!(unknown.starts_with("Unknown choice skynet, so use one of openai, anthropic"));
    assert!(plan(&args(&["--channel", "carrier-pigeon"]), true).is_err());

    let local = find(&PROVIDERS, "ollama-local").unwrap();

    assert_eq!((local.label, local.id), ("Ollama Local", "ollama"));
    assert_eq!(local.defaults, [("KRABB_OLLAMA_HOST", "http://127.0.0.1:11434")]);
    assert_eq!(find(&PROVIDERS, "ollama").unwrap().label, "Ollama Cloud");
}

#[test]
fn reset_stays_a_separate_flag() {
    use clap::Parser;

    for flags in [["init", "--force", "home"], ["init", "--force", "--provider=openai"]] {
        assert!(crate::cli::Cli::try_parse_from(["krabb"].iter().chain(&flags)).is_err());
    }

    assert!(args(&["--force", "--yes"]).force);
}

/// Every official provider, channel, and client appears in the guide, so new plugins
/// cannot be left out of the choices.
#[test]
fn choices_cover_every_official_provider_channel_and_client() {
    let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("../krabb-plugins");

    let folders = |group: &str| {
        std::fs::read_dir(plugins.join(group))
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.path().join("krabb-plugin.toml").is_file())
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect::<BTreeSet<_>>()
    };

    let providers = PROVIDERS.iter().map(|offer| offer.id.to_owned()).collect::<BTreeSet<_>>();
    let places = PLACES.iter().map(|offer| offer.id.to_owned()).collect::<BTreeSet<_>>();
    let mut talk = folders("channels");

    talk.extend(folders("clients"));

    assert_eq!(providers, folders("intelligence"));
    assert_eq!(places, talk);
}

#[test]
fn the_home_step_creates_or_keeps_the_home() {
    let root = crate::tests::test_root("onboard-home");
    let guide = guide(root.clone());

    let error = guide.require_home().unwrap_err().to_string();

    assert_eq!(error, "Krabb is not initialized.\nCreate it with `krabb init home`");

    guide.home().unwrap();

    assert!(root.join("config.toml").is_file());

    std::fs::write(root.join("data/system/KRAB.md"), "# Personality Instructions\nKept.\n")
        .unwrap();

    guide.home().unwrap();

    let krab = std::fs::read_to_string(root.join("data/system/KRAB.md")).unwrap();

    assert!(krab.ends_with("Kept.\n"));
    assert!(guide.require_home().is_ok());
    assert!(installed(&root, "channel").unwrap().is_empty());
    assert_eq!(ready_provider(&root).unwrap(), None);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn steps_after_home_refuse_a_missing_home() {
    let root = crate::tests::test_root("onboard-missing");
    let mut guide = guide(root.clone());

    for error in [
        guide.provider(Some("openai")).await.err(),
        guide.channel(Some("none")).await.err(),
        guide.hatch().await.err(),
        guide.service().err(),
    ] {
        assert!(error.unwrap().to_string().contains("`krabb init home`"));
    }
}

#[tokio::test]
async fn choosing_no_channel_installs_nothing() {
    let root = crate::tests::test_root("onboard-none");

    let mut guide = guide(root.clone());

    guide.home().unwrap();

    assert!(matches!(guide.channel(Some("none")).await.unwrap(), Flow::Next));
    assert!(matches!(guide.hatch().await.unwrap(), Flow::Next));
    assert!(!root.join("plugins/tui").exists());

    guide.finish().unwrap();

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_guide_ends_with_one_next_action() {
    assert_eq!(
        closing(true, true),
        "Your Krabb is ready.\nChat with it in the terminal with `krabb tui`"
    );

    assert_eq!(closing(true, false), "Your Krabb is ready.\nCheck every part with `krabb status`");

    assert!(closing(false, true).ends_with("Add one with `krabb init provider`"));
}

#[test]
fn model_lists_yield_ids_and_summaries_name_the_choice() {
    let listing = "Codex models (4):\n  gpt-5.5 - GPT-5.5 (default)\n  gpt-5.5-mini - Mini\n  \
                   odd entry - spaced\n  bad\u{1b}[2J - escape\n  login\nMore models are available.";

    assert_eq!(model_ids(listing), ["gpt-5.5", "gpt-5.5-mini"]);
    assert_eq!(typed_model("  ", "kept"), Ok("kept".into()));
    assert_eq!(typed_model(" gpt-typed ", "kept"), Ok("gpt-typed".into()));

    for refused in ["gpt 5", "bad\u{1b}", &"x".repeat(257)] {
        assert_eq!(typed_model(refused, "kept"), Err(MODEL.into()), "{refused:?}");
    }

    assert!(model_ids("No Codex models are available for this account.").is_empty());

    assert_eq!(
        summary("openai", "default"),
        "Your Krabb thinks with the openai provider and its default model."
    );

    assert_eq!(
        summary("codex", "gpt-5.5"),
        "Your Krabb thinks with the codex provider and the gpt-5.5 model."
    );
}

#[tokio::test]
async fn repeat_setup_keeps_the_model_of_the_same_provider() {
    let root = crate::tests::test_root("onboard-model");
    let mut guide = guide(root.clone());

    guide.home().unwrap();
    crate::save_setting(&root, "provider", "openai".into()).unwrap();
    crate::save_setting(&root, "model", "gpt-chosen".into()).unwrap();

    assert!(matches!(guide.select("openai").await.unwrap(), Flow::Next));

    let config = crate::load_config_at(&root).unwrap();

    assert_eq!(config.provider.as_deref(), Some("openai"));
    assert_eq!(config.model.as_deref(), Some("gpt-chosen"));

    assert!(matches!(guide.select("anthropic").await.unwrap(), Flow::Next));

    let config = crate::load_config_at(&root).unwrap();

    assert_eq!(config.provider.as_deref(), Some("anthropic"));
    assert_eq!(config.model.as_deref(), Some("default"));
    assert!(listed(&root, &config, "anthropic").await.is_empty());

    let rest = std::fs::read_to_string(root.join("config.toml"))
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("provider") && !line.starts_with("model"))
        .collect::<Vec<_>>()
        .join("\n");

    std::fs::write(root.join("config.toml"), format!("model = \"gpt-only\"\n{rest}\n")).unwrap();

    assert!(matches!(guide.select("codex").await.unwrap(), Flow::Next));
    assert_eq!(crate::load_config_at(&root).unwrap().model.as_deref(), Some("gpt-only"));

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn providers_with_a_models_command_offer_their_models() {
    use std::os::unix::fs::PermissionsExt;

    let root = crate::tests::test_root("onboard-listed");
    let source = root.join("source");
    let binary = source.join("bin").join(crate::plugin_name("lister"));

    crate::init_at(&root).unwrap();
    std::fs::create_dir_all(source.join("bin")).unwrap();

    std::fs::write(
        source.join("krabb-plugin.toml"),
        "id='lister'\nversion='0.1.0'\nprotocol={major=0,minor=1}\ncapabilities=['model']\n\
         [[commands]]\nname='lister'\ndescription='List models.'\ninteractive=false\n",
    )
    .unwrap();

    let hello = r#"{"jsonrpc":"2.0","id":1,"result":{"protocol":{"major":0,"minor":1},"id":"lister","version":"0.1.0","capabilities":["model"],"commands":[{"name":"lister","description":"List models.","interactive":false}]}}"#;

    let reply =
        r#"{"jsonrpc":"2.0","id":1,"result":{"text":"Models (2):\n  m-one - One\n  m-two - Two"}}"#;

    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*hello*) printf '%s\\n' \
             '{hello}' ;;\n*models*) printf '%s\\n' '{reply}' ;;\n*shutdown*) printf '%s\\n' \
             '{{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{{\"ok\":true}}}}'; exit 0 ;;\nesac\n\
             done\n"
        ),
    )
    .unwrap();

    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

    crate::link_at(
        crate::Source {
            id: "lister".into(),
            source: Some(source.display().to_string()),
            revision: None,
            version: None,
            yes: true,
        },
        false,
        &root,
    )
    .unwrap();

    let config = crate::load_config_at(&root).unwrap();

    assert_eq!(listed(&root, &config, "lister").await, ["m-one", "m-two"]);

    crate::save_setting(&root, "provider", "lister".into()).unwrap();
    crate::save_setting(&root, "model", "m-two".into()).unwrap();

    assert_eq!(configured(&root).unwrap(), Some(("lister".into(), "m-two".into())));

    let _ = std::fs::remove_dir_all(root);
}
