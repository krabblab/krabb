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

    assert_eq!(error, "Krabb is not initialized.\nRun `krabb init home` first.");

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
