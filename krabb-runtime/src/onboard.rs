//! Guides a first run through composable steps: home, provider, hatch, channel, and service.
//!
//! Each step calls the command it stands for, so `init` adds questions and order, not logic.

use std::path::{Path, PathBuf};

use crate::style::{accent, dim};

use crate::cli::{
    ConfigCommand, ConfigSet, HatchArgs, InitArgs, PluginCommand, PluginInstall, ServiceCommand,
    ServiceInstall,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// One part of setting up a Krabb, run in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub(crate) enum Step {
    Home,
    Provider,
    Hatch,
    Channel,
    Service,
}

const ALL: [Step; 5] = [Step::Home, Step::Provider, Step::Hatch, Step::Channel, Step::Service];

/// An official plugin a step offers, with the label people choose it by and the key a flag
/// names it with.
struct Offer {
    label: &'static str,
    key: &'static str,
    id: &'static str,
    defaults: &'static [(&'static str, &'static str)],
}

const fn offer(label: &'static str, id: &'static str) -> Offer {
    Offer { label, key: id, id, defaults: &[] }
}

const PROVIDERS: [Offer; 7] = [
    offer("OpenAI", "openai"),
    offer("Anthropic", "anthropic"),
    offer("Gemini", "gemini"),
    offer("OpenRouter", "openrouter"),
    offer("Codex", "codex"),
    Offer {
        label: "Ollama Cloud",
        key: "ollama",
        id: "ollama",
        defaults: &[("KRABB_OLLAMA_HOST", "https://ollama.com")],
    },
    Offer {
        label: "Ollama Local",
        key: "ollama-local",
        id: "ollama",
        defaults: &[("KRABB_OLLAMA_HOST", "http://127.0.0.1:11434")],
    },
];

/// Places to talk to a Krabb: the terminal client first, then chat channels.
const PLACES: [Offer; 6] = [
    offer("Terminal", "tui"),
    offer("Telegram", "telegram"),
    offer("Discord", "discord"),
    offer("Slack", "slack"),
    offer("WhatsApp", "whatsapp"),
    offer("Signal", "signal"),
];

const YES_LATER: [&str; 2] = ["Yes", "Not now"];

/// Runs `krabb init`: the reset or plain home creation as before, or the guided steps.
pub(crate) async fn run(args: InitArgs, json: bool) -> Result<(), Error> {
    let interactive = !json && crate::agent::interactive();
    let guided = !args.steps.is_empty() || args.provider.is_some() || args.channel.is_some();

    if args.force || (!guided && !interactive) {
        return crate::home::init(args.force, args.yes, json);
    }

    crate::home::validate_init_flags(args.force, args.yes)?;

    if json {
        return Err("The guided steps talk to people, so run them without --json.".into());
    }

    let steps = plan(&args, interactive)?;

    let mut guide = Guide {
        root: crate::home(),
        color: crate::style::colors(),
        number: 0,
        named: args.steps.clone(),
        interactive,
    };

    if interactive {
        println!("Let's set up your Krabb.");
    }

    for step in steps {
        let flow = match step {
            Step::Home => guide.home()?,
            Step::Provider => guide.provider(args.provider.as_deref()).await?,
            Step::Hatch => guide.hatch().await?,
            Step::Channel => guide.channel(args.channel.as_deref()).await?,
            Step::Service => guide.service()?,
        };

        if let Flow::Stop = flow {
            println!();
            println!("Setup stopped.");
            println!("Continue any time with `krabb init`");
            return Ok(());
        }
    }

    guide.finish()
}

/// Which steps to run: named ones, those a flag answers, or all of them in a terminal.
fn plan(args: &InitArgs, interactive: bool) -> Result<Vec<Step>, String> {
    let mut steps = args.steps.clone();

    if args.provider.is_some() {
        steps.push(Step::Provider);
    }

    if args.channel.is_some() {
        steps.push(Step::Channel);
    }

    if steps.is_empty() && interactive {
        steps.extend(ALL);
    }

    if steps.is_empty() {
        steps.push(Step::Home);
    }

    steps.sort();
    steps.dedup();

    if let Some(id) = &args.provider {
        find(&PROVIDERS, id)?;
    }

    if let Some(id) = args.channel.as_deref().filter(|id| *id != "none") {
        find(&PLACES, id)?;
    }

    if !interactive {
        if steps.contains(&Step::Provider) && args.provider.is_none() {
            return Err("Choose a provider with --provider, such as --provider openai.".into());
        }

        if steps.contains(&Step::Channel) && args.channel.is_none() {
            return Err("Choose where to talk with --channel, such as --channel telegram.".into());
        }

        if steps.contains(&Step::Hatch) {
            return Err("Hatching asks questions, so run `krabb hatch --yes` with your wishes \
                        instead."
                .into());
        }
    }

    Ok(steps)
}

fn find<'a>(offers: &'a [Offer], key: &str) -> Result<&'a Offer, String> {
    offers.iter().find(|offer| offer.key == key).ok_or_else(|| {
        let keys = offers.iter().map(|offer| offer.key).collect::<Vec<_>>();

        format!("Unknown choice {key}, so use one of {}.", keys.join(", "))
    })
}

/// Whether the person stopped the guide, which ends it without an error.
enum Flow {
    Next,
    Stop,
}

struct Guide {
    root: PathBuf,
    color: bool,
    number: usize,
    named: Vec<Step>,
    interactive: bool,
}

impl Guide {
    fn section(&self, title: &str) {
        println!("\n{}\n", accent(title, self.color));
    }

    fn done(&self, text: &str) {
        println!("{} {text}", accent("✓", self.color));
    }

    fn note(&self, text: &str) {
        println!("{}", dim(text, self.color));
    }

    /// Asks a numbered question with a vertical picker, or `None` when the person stops.
    fn ask(&mut self, question: &str, options: &[&str]) -> std::io::Result<Option<usize>> {
        self.number += 1;
        eprintln!("{}\n", accent(&format!("{}. {question}", self.number), self.color));

        let answer = crate::choice::pick(options, 0)?;

        eprintln!();
        Ok(answer)
    }

    /// Whether the person named STEP, so a finished step offers a change instead of a skip.
    fn named(&self, step: Step) -> bool {
        self.interactive && self.named.contains(&step)
    }

    fn require_home(&self) -> Result<(), Error> {
        if crate::home::regular_file_present(&self.root.join("config.toml")) {
            Ok(())
        } else {
            Err("Krabb is not initialized.\nRun `krabb init home` first.".into())
        }
    }

    fn home(&self) -> Result<Flow, Error> {
        self.section("Home");

        if crate::home::regular_file_present(&self.root.join("config.toml")) {
            self.done(&format!("Home is ready at {}.", self.root.display()));
        } else {
            crate::home::init_at(&self.root)?;
            self.done(&format!("Created home at {}.", self.root.display()));
        }

        Ok(Flow::Next)
    }

    async fn provider(&mut self, flag: Option<&str>) -> Result<Flow, Error> {
        self.require_home()?;
        self.section("Intelligence");

        if flag.is_none()
            && let Some(current) = ready_provider(&self.root)?
        {
            if !self.named(Step::Provider) {
                self.done(&format!("Your Krabb thinks with the {current} provider."));
                return Ok(Flow::Next);
            }

            let question = format!("Change the {current} provider your Krabb thinks with?");

            match self.ask(&question, &["Keep it", "Choose another"])? {
                None => return Ok(Flow::Stop),
                Some(0) => return Ok(Flow::Next),

                Some(_) => {}
            }
        }

        let offer = match flag {
            Some(id) => find(&PROVIDERS, id)?,

            None => {
                let mut labels = PROVIDERS.iter().map(|offer| offer.label).collect::<Vec<_>>();

                labels.push("Skip");

                match self.ask("Which provider should your Krabb think with?", &labels)? {
                    None => return Ok(Flow::Stop),

                    Some(index) if index == PROVIDERS.len() => {
                        self.note("Add a provider later with `krabb init provider`");
                        return Ok(Flow::Next);
                    }

                    Some(index) => &PROVIDERS[index],
                }
            }
        };

        if let Flow::Stop = self.install(offer, "provider").await? {
            return Ok(Flow::Stop);
        }

        let provider = toml::Value::String(offer.id.into());

        for assignment in [format!("provider={provider}"), "model=\"default\"".to_owned()] {
            let set = ConfigSet { assignment, force: false };

            crate::config_command(ConfigCommand::Set(set), &self.root, false).await?;
        }

        self.done(&format!("Your Krabb thinks with the {} provider.", offer.id));
        Ok(Flow::Next)
    }

    async fn hatch(&mut self) -> Result<Flow, Error> {
        self.require_home()?;
        self.section("Personality");

        if ready_provider(&self.root)?.is_none() {
            self.note("Hatching needs a provider, so add one with `krabb init provider`");
            return Ok(Flow::Next);
        }

        if crate::hatch::hatched(&crate::hatch::read(&self.root.join("data/system"))?)
            && !self.named(Step::Hatch)
        {
            self.done("Your Krabb has a personality.");
            return Ok(Flow::Next);
        }

        match self.ask("Give your Krabb a personality now?", &YES_LATER)? {
            None => Ok(Flow::Stop),

            Some(0) => {
                crate::hatch::run(HatchArgs::default(), &self.root, false).await?;
                Ok(Flow::Next)
            }

            Some(_) => {
                self.note("Give it one later with `krabb hatch`");
                Ok(Flow::Next)
            }
        }
    }

    async fn channel(&mut self, flag: Option<&str>) -> Result<Flow, Error> {
        self.require_home()?;
        self.section("Conversations");

        let mut places = installed(&self.root, "channel")?;

        places.extend(installed(&self.root, "client")?);

        if flag.is_none() && !places.is_empty() && !self.named(Step::Channel) {
            self.done(&format!("Your Krabb talks with you through {}.", places.join(", ")));
            return Ok(Flow::Next);
        }

        let offer = match flag {
            Some("none") => return Ok(Flow::Next),
            Some(id) => find(&PLACES, id)?,

            None => {
                let labels = PLACES.iter().map(|offer| offer.label).collect::<Vec<_>>();

                match self.ask("Where will you talk to your Krabb?", &labels)? {
                    None => return Ok(Flow::Stop),
                    Some(index) => &PLACES[index],
                }
            }
        };

        let kind = if offer.id == "tui" { "client" } else { "channel" };

        self.install(offer, kind).await
    }

    fn service(&mut self) -> Result<Flow, Error> {
        self.require_home()?;
        self.section("Service");

        let installed = crate::service::service_path().is_file();

        if installed && !self.named(Step::Service) {
            self.done("Your Krabb runs as a background service.");
            return Ok(Flow::Next);
        }

        let (question, options) = if installed {
            ("Replace the background service that is already installed?", ["Keep it", "Replace it"])
        } else {
            ("Keep your Krabb running in the background?", YES_LATER)
        };

        let write = if self.interactive {
            match self.ask(question, &options)? {
                None => return Ok(Flow::Stop),
                Some(0) => !installed,

                Some(_) if !installed => {
                    self.note("Start it later with `krabb init service`");
                    return Ok(Flow::Next);
                }

                Some(_) => true,
            }
        } else {
            !installed
        };

        if write {
            let install = ServiceInstall { force: installed };

            crate::service::service(ServiceCommand::Install(install), false)?;
        }

        crate::service::service(ServiceCommand::Start, false)?;
        self.done("Your Krabb runs as a background service.");
        Ok(Flow::Next)
    }

    /// Installs OFFER unless it already is, then asks for its secrets and settings. Cancelling
    /// those questions stops the guide, as cancelling a choice does.
    async fn install(&mut self, offer: &Offer, kind: &str) -> Result<Flow, Error> {
        if crate::load_lock_at(&self.root)?.plugins.contains_key(offer.id) {
            self.done(&format!("The {} {kind} is already installed.", offer.id));
        } else {
            let install = PluginInstall {
                ids: vec![offer.id.into()],
                source: None,
                revision: None,
                link: false,
                unstable: false,
                yes: true,
            };

            crate::plugins::plugin(PluginCommand::Install(install), false).await?;
            self.done(&format!("Installed {} {kind}.", offer.id));
        }

        if !crate::setup::needed(&self.root, offer.id)? {
            return Ok(Flow::Next);
        }

        let later = format!("Set it up later with `krabb plugin setup {}`", offer.id);

        if !self.interactive {
            self.note(&later);
            return Ok(Flow::Next);
        }

        let defaults = offer
            .defaults
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();

        let options = crate::setup::Options::new(offer.id, defaults, self.number);

        match crate::setup::run(options, &self.root, false).await {
            Ok(number) => self.number = number,

            Err(error) if error.to_string() == crate::setup::CANCELLED => {
                self.note(&later);
                return Ok(Flow::Stop);
            }

            Err(error) => return Err(error),
        }

        Ok(Flow::Next)
    }

    fn finish(&self) -> Result<(), Error> {
        println!();

        if ready_provider(&self.root)?.is_some() {
            println!("Your Krabb is ready.");
        } else {
            println!("Your Krabb is set up, and it needs a provider to answer.");
        }

        if crate::load_lock_at(&self.root)?.plugins.contains_key("tui") {
            println!("Chat with it in the terminal with `krabb tui`");
        }

        println!("Check every part with `krabb status`");
        Ok(())
    }
}

/// The selected provider when it is installed and ready to answer.
fn ready_provider(root: &Path) -> Result<Option<String>, Error> {
    let config = crate::load_config_at(root)?;
    let (provider, _) = crate::selection::client(&config, "cli", (None, None));

    Ok(crate::model_plugin_at(root, &provider).is_ok().then_some(provider))
}

/// Ids of installed plugins with CAPABILITY.
fn installed(root: &Path, capability: &str) -> Result<Vec<String>, Error> {
    let lock = crate::load_lock_at(root)?;

    Ok(lock
        .plugins
        .iter()
        .filter(|(_, entry)| entry.capabilities.iter().any(|item| item == capability))
        .map(|(id, _)| id.clone())
        .collect())
}

#[cfg(test)]
mod tests;
