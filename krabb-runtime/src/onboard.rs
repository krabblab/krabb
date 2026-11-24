//! Guides a first run through composable steps: home, provider, hatch, channel, and service.
//!
//! Each step calls the command it stands for, so `init` adds questions and order, not logic.

use std::path::{Path, PathBuf};

use crate::style::{accent, dim};

use crate::cli::{
    HatchArgs, InitArgs, PluginCommand, PluginInstall, ServiceCommand, ServiceInstall,
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

/// How many listed models a choice shows, so a long catalog stays readable.
const CHOICES: usize = 20;

/// Why a typed model name was refused, matching the rule `/model` applies.
const MODEL: &str = "Model names are one line of 1–256 visible characters without spaces.";

/// How long a provider may take to list its models before setup asks for a name instead.
const LISTING: std::time::Duration = std::time::Duration::from_secs(30);

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

    /// Numbers the next question and styles it as a heading.
    fn heading(&mut self, question: &str) -> String {
        self.number += 1;

        format!("{}\n", accent(&format!("{}. {question}", self.number), self.color))
    }

    /// Asks a numbered question with a vertical picker, or `None` when the person stops.
    fn ask(&mut self, question: &str, options: &[&str]) -> std::io::Result<Option<usize>> {
        let heading = self.heading(question);

        eprintln!("{heading}");

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
            && let Some((current, model)) = configured(&self.root)?
        {
            if !self.named(Step::Provider) {
                self.done(&summary(&current, &model));
                self.overridden(&current, &model)?;
                return Ok(Flow::Next);
            }

            let question = format!("Change the {current} provider your Krabb thinks with?");

            match self.ask(&question, &["Keep it", "Change its model", "Choose another"])? {
                None => return Ok(Flow::Stop),
                Some(0) => return Ok(Flow::Next),
                Some(1) => return self.select(&current).await,

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

        self.select(offer.id).await
    }

    /// Asks for PROVIDER's model, saves both, and says where they apply and when.
    async fn select(&mut self, provider: &str) -> Result<Flow, Error> {
        let config = crate::load_config_at(&self.root)?;
        let (current, model) = global(&config);
        let kept = (current == provider && model != "default").then_some(model);

        let Some(model) = self.model(&config, provider, kept.as_deref()).await? else {
            return Ok(Flow::Stop);
        };

        crate::save_setting(&self.root, "provider", provider.into())?;
        crate::save_setting(&self.root, "model", model.as_str().into())?;

        let applied =
            crate::control_at("config.selection.reload", serde_json::json!({}), &self.root)
                .await?
                .is_some();

        self.done(&summary(provider, &model));
        self.note(&format!(
            "Saved in {} for every conversation without a choice of its own",
            self.root.join("config.toml").display()
        ));

        self.overridden(provider, &model)?;

        self.note(if applied {
            "No restart needed; the running daemon uses it for new turns"
        } else {
            "No restart needed; it applies when the daemon starts"
        });

        Ok(Flow::Next)
    }

    /// Notes when the environment or `clients.cli` makes the CLI use another provider or model.
    fn overridden(&self, provider: &str, model: &str) -> Result<(), Error> {
        let config = crate::load_config_at(&self.root)?;
        let (used, used_model) = crate::selection::client(&config, "cli", (None, None));

        if (used.as_str(), used_model.as_str()) != (provider, model) {
            self.note(&format!(
                "KRABB_MODEL_PLUGIN, KRABB_MODEL, or clients.cli make the CLI use {used} with \
                 {used_model} instead"
            ));
        }

        Ok(())
    }

    /// The model to use: one from the provider's list, a typed name, or KEPT on Enter.
    async fn model(
        &mut self,
        config: &crate::Config,
        provider: &str,
        kept: Option<&str>,
    ) -> Result<Option<String>, Error> {
        let keep = kept.unwrap_or("default").to_owned();

        if !self.interactive {
            return Ok(Some(keep));
        }

        self.note(&format!("Looking up the {provider} models"));

        let models = listed(&self.root, config, provider).await;
        let mut heading = None;

        if !models.is_empty() {
            let first = kept.map_or_else(
                || "Use its default model".to_owned(),
                |model| format!("Keep {model}"),
            );

            let shown = models
                .iter()
                .filter(|model| Some(model.as_str()) != kept)
                .take(CHOICES)
                .collect::<Vec<_>>();

            let mut labels = vec![first.as_str()];

            labels.extend(shown.iter().map(|model| model.as_str()));
            labels.push("Type another model");

            match self.ask("Which model should your Krabb use?", &labels)? {
                None => return Ok(None),
                Some(0) => return Ok(Some(keep)),
                Some(index) if index <= shown.len() => return Ok(Some(shown[index - 1].clone())),

                Some(_) => heading = Some(String::new()),
            }
        }

        let hint = kept.map_or_else(
            || format!("Press Enter to use the {provider} default model"),
            |model| format!("Press Enter to keep {model}"),
        );

        let heading = heading.unwrap_or_else(|| self.heading("Which model should your Krabb use?"));

        let mut error = None;

        loop {
            let prompt = match &error {
                Some(error) => format!("{error}\nTry again, or press Esc to stop.\n"),
                None => format!("{heading}{}\n", dim(&hint, self.color)),
            };

            match crate::setup::line(&prompt) {
                Ok(typed) => match typed_model(&typed, &keep) {
                    Ok(model) => return Ok(Some(model)),
                    Err(problem) => error = Some(problem),
                },

                Err(problem) if problem.to_string() == crate::setup::CANCELLED => return Ok(None),
                Err(problem) if crate::setup::rejected(&problem) => error = Some(MODEL.into()),
                Err(problem) => return Err(problem),
            }
        }
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
                quiet: true,
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

        let ready = ready_provider(&self.root)?.is_some();
        let terminal = crate::load_lock_at(&self.root)?.plugins.contains_key("tui");

        println!("{}", closing(ready, terminal));
        Ok(())
    }
}

/// How the guide ends: whether the Krabb can answer, then the one command to run next.
fn closing(ready: bool, terminal: bool) -> &'static str {
    match (ready, terminal) {
        (true, true) => "Your Krabb is ready.\nChat with it in the terminal with `krabb tui`",
        (true, false) => "Your Krabb is ready.\nCheck every part with `krabb status`",

        (false, _) => {
            "Your Krabb is set up, and it needs a provider to answer.\n\
                       Add one with `krabb init provider`"
        }
    }
}

fn summary(provider: &str, model: &str) -> String {
    if model == "default" {
        format!("Your Krabb thinks with the {provider} provider and its default model.")
    } else {
        format!("Your Krabb thinks with the {provider} provider and the {model} model.")
    }
}

/// The model a typed answer names: KEEP for an empty answer, or a valid model name.
fn typed_model(typed: &str, keep: &str) -> Result<String, String> {
    let typed = typed.trim();

    if typed.is_empty() {
        return Ok(keep.to_owned());
    }

    if usable(typed) { Ok(typed.to_owned()) } else { Err(MODEL.into()) }
}

/// Whether NAME is safe to show and save as a model, as `/model` requires.
fn usable(name: &str) -> bool {
    crate::selection::validate(None, Some(name)).is_ok() && !name.contains(char::is_whitespace)
}

/// Model ids PROVIDER lists through its `models` command, or none when it cannot list them.
/// A running daemon answers with the copy it already runs; otherwise the plugin starts briefly.
async fn listed(root: &Path, config: &crate::Config, provider: &str) -> Vec<String> {
    let params = serde_json::json!({"plugin": provider});

    // A running daemon that has the plugin answers; only without one does a copy start here.
    match tokio::time::timeout(LISTING, crate::control_at("model.list", params, root)).await {
        Ok(Ok(Some(value))) => return value["text"].as_str().map(model_ids).unwrap_or_default(),

        Ok(Ok(None)) => {}

        Ok(Err(error)) if error.to_string() == crate::ipc::MODEL_NOT_RUNNING => {}

        _ => return Vec::new(),
    }

    let Some(path) = crate::binary_at(provider, root) else {
        return Vec::new();
    };

    let capability = Some(crate::Capability::Model);

    let Ok(mut process) = crate::launch(root, path, provider, capability, false, config).await
    else {
        return Vec::new();
    };

    let mut models = Vec::new();

    if process.hello.commands.iter().any(|command| command.name == provider) {
        let request = krabb_core::types::Request::call(
            1,
            "command",
            serde_json::json!({"name": provider, "args": ["models"]}),
        );

        if let Ok(Ok(response)) = tokio::time::timeout(LISTING, process.call(request)).await
            && let Some(text) = response.result.as_ref().and_then(|value| value["text"].as_str())
        {
            models = model_ids(text);
        }
    }

    let _ = process.stop().await;

    models
}

/// Reads the indented `id - name` lines a provider's model list prints, keeping usable ids.
fn model_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("  "))
        .filter_map(|line| line.split_once(" - "))
        .map(|(id, _)| id.trim())
        .filter(|id| usable(id))
        .map(str::to_owned)
        .collect()
}

/// The provider and model the top-level configuration selects, without environment overrides.
fn global(config: &crate::Config) -> (String, String) {
    crate::selection::resolve(config, (None, None), (None, None), (None, None))
}

/// The provider and model saved in the configuration, when that provider is installed.
fn configured(root: &Path) -> Result<Option<(String, String)>, Error> {
    let config = crate::load_config_at(root)?;
    let (provider, model) = global(&config);

    Ok(crate::model_plugin_at(root, &provider).is_ok().then_some((provider, model)))
}

/// The selected provider when it is installed and ready to answer.
pub(crate) fn ready_provider(root: &Path) -> Result<Option<String>, Error> {
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
