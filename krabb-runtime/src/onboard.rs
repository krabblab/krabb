//! Guides a first run through composable steps: home, provider, hatch, channel, and service.
//!
//! Each step calls the command it stands for, so `init` adds questions and order, not logic.

use std::path::{Path, PathBuf};

use crate::style::{accent, dim};

use crate::cli::{HatchArgs, InitArgs, PluginCommand, PluginInstall, ServiceAction};

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

/// Plugins a place installs with it, without a question, so its first chat can already act.
const TERMINAL: [Offer; 1] = [offer("Tools", "tools")];

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
        spaced: true,
        named: args.steps.clone(),
        interactive,
        started: false,
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

        match flow {
            Flow::Next => {}

            Flow::Stop => {
                guide.gap();
                println!("Setup stopped.");
                println!("Continue any time with `krabb init`.");
                return Ok(());
            }

            Flow::Blocked { why, next } => {
                guide.gap();
                return Err(blocked(why, next).into());
            }
        }
    }

    guide.finish().await
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

/// Whether the guide goes on, the person stopped it, or a step cannot finish yet. Only a blocked
/// step ends it with an error, so scripts see that setup is incomplete.
enum Flow {
    Next,
    Stop,
    Blocked { why: &'static str, next: &'static str },
}

/// What Codex needs before it can answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codex {
    Ready,
    Install,
    SignIn,
    Unsigned,
}

const INSTALL_CODEX: Flow = Flow::Blocked {
    why: "the Codex CLI is not installed",
    next: "Install it from https://developers.openai.com/codex/cli, then run `krabb init` again.",
};

const SIGN_IN_CODEX: Flow = Flow::Blocked {
    why: "Codex is not signed in",
    next: "Sign in with `krabb codex login`, then run `krabb init` again.",
};

/// What Codex needs given whether its CLI is installed, whether it is signed in, and whether a
/// person can sign in here.
fn codex_need(cli: bool, signed_in: bool, interactive: bool) -> Codex {
    match (cli, signed_in, interactive) {
        (false, _, _) => Codex::Install,
        (true, true, _) => Codex::Ready,
        (true, false, true) => Codex::SignIn,
        (true, false, false) => Codex::Unsigned,
    }
}

/// Why hatching must wait for Codex, with the fix, or none when Codex can answer.
fn hatch_wait(need: Codex) -> Option<&'static str> {
    match need {
        Codex::Ready => None,

        Codex::Install => Some(
            "Hatching needs the Codex CLI, so install it from \
             https://developers.openai.com/codex/cli, then run `krabb init hatch`.",
        ),

        Codex::SignIn | Codex::Unsigned => Some(
            "Hatching needs Codex signed in, so run `krabb codex login`, then `krabb init hatch`.",
        ),
    }
}

/// The error a blocked step ends the guide with: why it stopped, then the one next action.
fn blocked(why: &str, next: &str) -> String {
    format!("Setup stopped, because {why}.\n{next}")
}

/// The Codex CLI this shell runs, from `KRABB_CODEX_BINARY` or the search path.
fn codex_binary() -> Option<PathBuf> {
    if let Some(binary) = std::env::var_os("KRABB_CODEX_BINARY") {
        return Some(binary.into());
    }

    crate::service::codex_binary_from_path(&std::env::var("PATH").unwrap_or_default())
}

/// Whether the Codex CLI is installed and signed in, without asking anything.
fn codex_state() -> (bool, bool) {
    let cli = codex_binary().is_some();

    (cli, cli && crate::codex_auth())
}

struct Guide {
    root: PathBuf,
    color: bool,
    /// Whether the last line printed was blank or a section title, so a question needs no gap.
    spaced: bool,
    named: Vec<Step>,
    interactive: bool,
    /// Whether the service step started the service, which may not report running at once.
    started: bool,
}

impl Guide {
    fn section(&mut self, title: &str) {
        println!("\n{}", accent(title, self.color));
        self.spaced = true;
    }

    fn done(&mut self, text: &str) {
        println!("{} {text}", accent("✓", self.color));
        self.spaced = false;
    }

    fn note(&mut self, text: &str) {
        println!("{}", dim(text, self.color));
        self.spaced = false;
    }

    /// Separates a finished step from what comes next with one blank line.
    fn gap(&mut self) {
        if !self.spaced {
            println!();
            self.spaced = true;
        }
    }

    /// Prints the gap before a question and returns the question's line.
    fn question(&mut self, text: &str) -> String {
        self.gap();
        self.spaced = false;

        format!("{text}\n")
    }

    /// Asks a question with a vertical picker, or `None` when the person stops.
    fn ask(&mut self, question: &str, options: &[&str]) -> std::io::Result<Option<usize>> {
        let heading = self.question(question);

        eprint!("{heading}");

        let answer = crate::choice::pick(options, 0)?;

        eprintln!();
        self.spaced = true;
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
            Err("Krabb is not initialized.\nCreate it with `krabb init home`.".into())
        }
    }

    fn home(&mut self) -> Result<Flow, Error> {
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
                return self.sign_in(&current).await;
            }

            let question = format!("Change the {current} provider your Krabb thinks with?");

            match self.ask(&question, &["Keep it", "Change its model", "Choose another"])? {
                None => return Ok(Flow::Stop),
                Some(0) => return self.sign_in(&current).await,

                Some(1) => {
                    return match self.sign_in(&current).await? {
                        Flow::Next => self.select(&current).await,
                        flow => Ok(flow),
                    };
                }

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
                        self.note("Add a provider later with `krabb init provider`.");
                        return Ok(Flow::Next);
                    }

                    Some(index) => &PROVIDERS[index],
                }
            }
        };

        match self.install(offer, "provider").await? {
            Flow::Next => {}
            flow => return Ok(flow),
        }

        match self.sign_in(offer.id).await? {
            Flow::Next => self.select(offer.id).await,
            flow => Ok(flow),
        }
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
            "Saved in {} for every conversation without a choice of its own.",
            self.root.join("config.toml").display()
        ));

        self.overridden(provider, &model)?;

        self.note(if applied {
            "No restart is needed, because the running daemon uses it for new turns."
        } else {
            "No restart is needed, because it applies when the daemon starts."
        });

        Ok(Flow::Next)
    }

    /// Notes when the environment or `clients.cli` makes the CLI use another provider or model.
    fn overridden(&mut self, provider: &str, model: &str) -> Result<(), Error> {
        let config = crate::load_config_at(&self.root)?;
        let (used, used_model) = crate::selection::client(&config, "cli", (None, None));

        if (used.as_str(), used_model.as_str()) != (provider, model) {
            self.note(&format!(
                "KRABB_MODEL_PLUGIN, KRABB_MODEL, or clients.cli make the CLI use {used} with \
                 {used_model} instead."
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

        self.gap();
        self.note(&format!("Looking up the {provider} models."));
        self.spaced = true;

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
            || format!("Press Enter to use the {provider} default model."),
            |model| format!("Press Enter to keep {model}."),
        );

        let heading =
            heading.unwrap_or_else(|| self.question("Which model should your Krabb use?"));

        let mut error = None;

        self.spaced = false;

        loop {
            let prompt = match &error {
                Some(error) => format!("{error}\nTry again, or press Esc to stop.\n"),
                None => format!("{heading}{}\n", dim(&hint, self.color)),
            };

            match crate::setup::line(&prompt) {
                Ok(typed) => match typed_model(&typed, &keep) {
                    Ok(model) => {
                        eprintln!();
                        self.spaced = true;
                        return Ok(Some(model));
                    }

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

        let Some(provider) = ready_provider(&self.root)? else {
            self.note("Hatching needs a provider, so add one with `krabb init provider`.");
            return Ok(Flow::Next);
        };

        if provider == "codex" {
            let (cli, signed_in) = codex_state();

            if let Some(wait) = hatch_wait(codex_need(cli, signed_in, false)) {
                self.note(wait);
                return Ok(Flow::Next);
            }
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
                self.spaced = false;
                Ok(Flow::Next)
            }

            Some(_) => {
                self.note("Give it one later with `krabb hatch`.");
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

            for place in &places {
                self.add_companions(place).await?;
            }

            return Ok(Flow::Next);
        }

        let offer = match flag {
            Some("none") => {
                self.note("Add a place to talk later with `krabb init channel`.");
                return Ok(Flow::Next);
            }

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

        let flow = self.install(offer, kind).await?;

        if !matches!(flow, Flow::Next) {
            return Ok(flow);
        }

        self.add_companions(offer.id).await?;
        Ok(Flow::Next)
    }

    /// Installs the plugins that come with the place ID when they are missing, so an earlier
    /// home that already talks through the TUI also gets its workspace tools. A failed install,
    /// such as offline, leaves a note with its fix instead of stopping the guide.
    async fn add_companions(&mut self, id: &str) -> Result<(), Error> {
        for companion in companions(id) {
            if crate::load_lock_at(&self.root)?.plugins.contains_key(companion.id) {
                continue;
            }

            match fetch(companion.id).await {
                Ok(()) => self.done(&format!("Installed {} plugin.", companion.id)),
                Err(error) => self.note(&companion_failed(companion.id, &error.to_string())),
            }
        }

        Ok(())
    }

    fn service(&mut self) -> Result<Flow, Error> {
        self.require_home()?;
        self.section("Service");

        let path = crate::service::service_path();
        let installed = path.is_file();

        let (question, options) = if installed {
            ("Replace the background service that is already installed?", ["Keep it", "Replace it"])
        } else {
            ("Keep your Krabb running in the background?", YES_LATER)
        };

        let write = if installed && !self.named(Step::Service) {
            false
        } else if self.interactive {
            match self.ask(question, &options)? {
                None => return Ok(Flow::Stop),
                Some(0) => !installed,

                Some(_) if !installed => {
                    self.note("The background service stays off.");
                    return Ok(Flow::Next);
                }

                Some(_) => true,
            }
        } else {
            !installed
        };

        let patched = installed && !write && self.find_codex(&path)?;

        if write {
            crate::service::install_at(&path, installed)?;
        }

        let silent = crate::service::ServiceReport::Silent;

        for action in actions(installed && (write || patched)) {
            crate::service::service_action_with(&path, *action, silent)?;
        }

        self.started = true;

        if patched {
            self.done("Updated the background service so it finds Codex.");
        }

        self.done("Your Krabb runs as a background service.");
        Ok(Flow::Next)
    }

    /// Adds the Codex CLI this shell runs to the service at PATH when Codex is the provider and
    /// the service, installed before Codex, could not find it. Returns whether it did.
    fn find_codex(&self, path: &Path) -> Result<bool, Error> {
        if ready_provider(&self.root)?.as_deref() != Some("codex") {
            return Ok(false);
        }

        let Some(binary) = codex_binary() else {
            return Ok(false);
        };

        let Some(environment) = crate::definition::environment(path) else {
            return Ok(false);
        };

        let Some(environment) = crate::definition::with_codex(&environment, &binary) else {
            return Ok(false);
        };

        crate::definition::rewrite(path, &environment)
    }

    /// Installs OFFER unless it already is, then asks for its secrets and settings. Cancelling
    /// those questions stops the guide, as cancelling a choice does.
    async fn install(&mut self, offer: &Offer, kind: &str) -> Result<Flow, Error> {
        if crate::load_lock_at(&self.root)?.plugins.contains_key(offer.id) {
            self.done(&format!("The {} {kind} is already installed.", offer.id));
        } else {
            fetch(offer.id).await?;
            self.done(&format!("Installed {} {kind}.", offer.id));
        }

        self.configure(offer).await
    }

    /// Makes sure PROVIDER can answer before the guide goes on. Only Codex needs more than its
    /// plugin: its CLI, signed in with the person's ChatGPT account.
    async fn sign_in(&mut self, provider: &str) -> Result<Flow, Error> {
        if provider != "codex" {
            return Ok(Flow::Next);
        }

        let (cli, signed_in) = codex_state();

        match codex_need(cli, signed_in, self.interactive) {
            Codex::Ready => return Ok(Flow::Next),
            Codex::Install => return Ok(INSTALL_CODEX),
            Codex::Unsigned => return Ok(SIGN_IN_CODEX),

            Codex::SignIn => {}
        }

        self.gap();
        self.note("Sign in to Codex with your ChatGPT account.");

        let login = vec!["codex".to_owned(), "login".to_owned()];
        let finished = crate::external_command_at(login, false, self.root.clone()).await.is_ok();

        self.spaced = false;
        self.gap();

        if !finished || !crate::codex_auth() {
            return Ok(SIGN_IN_CODEX);
        }

        self.done("Codex is signed in.");
        Ok(Flow::Next)
    }

    /// Asks for OFFER's secrets and settings when it has any left to set.
    /// Codex has only optional settings, which `krabb plugin setup codex` changes later.
    async fn configure(&mut self, offer: &Offer) -> Result<Flow, Error> {
        if offer.id == "codex" || !crate::setup::needed(&self.root, offer.id)? {
            return Ok(Flow::Next);
        }

        let later = format!("Set it up later with `krabb plugin setup {}`.", offer.id);

        if !self.interactive {
            self.note(&later);
            return Ok(Flow::Next);
        }

        let defaults = offer
            .defaults
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();

        let options = crate::setup::Options::new(offer.id, defaults, true);

        self.gap();

        match crate::setup::run(options, &self.root, false).await {
            Ok(()) => self.spaced = false,

            Err(error) if error.to_string() == crate::setup::CANCELLED => {
                self.note(&later);
                return Ok(Flow::Stop);
            }

            Err(error) => return Err(error),
        }

        Ok(Flow::Next)
    }

    async fn finish(&mut self) -> Result<(), Error> {
        self.gap();

        let Some(provider) = ready_provider(&self.root)? else {
            println!("{}", closing(Ending::Provider));
            return Ok(());
        };

        let credentials = crate::setup::path(&self.root);
        let key = crate::credentials::key_path(&self.root);

        if !crate::ready_at(&provider, Some(&credentials), &key) {
            println!("{}", closing(Ending::Setup(&provider)));
            return Ok(());
        }

        let running = self.started
            || (crate::service_path().is_file()
                && matches!(crate::service_manager_state(), Ok(crate::ServiceState::Active)));

        let status = serde_json::json!({});

        if !running
            && !crate::control_at("status", status, &self.root)
                .await
                .is_ok_and(|answer| answer.is_some())
        {
            println!("{}", closing(Ending::Service));

            return Ok(());
        }

        if running
            && self.interactive
            && let Some(problem) = self.blocker(&provider).await
        {
            println!("{problem}");
            return Ok(());
        }

        let terminal = crate::load_lock_at(&self.root)?.plugins.contains_key("tui");

        println!("{}", closing(Ending::Ready { terminal }));

        Ok(())
    }

    /// Why the running Krabb cannot answer with PROVIDER yet, with the one next step, or none
    /// when the service answers and the provider loaded. Init writes and starts the service just
    /// before this, so a slow start gets the same wait `krabb tui` gives it, even while the
    /// service manager does not report it running yet.
    async fn blocker(&self, provider: &str) -> Option<String> {
        if let Err(error) =
            crate::require_tui_daemon(&self.root, true, crate::DAEMON_STARTUP_CHECKS).await
        {
            return Some(format!("Your Krabb is set up, but it does not answer yet.\n{error}"));
        }

        let status = crate::status::status_at(&self.root).await.ok();

        let states = status.as_ref().and_then(|status| status["plugin_states"].as_array());
        let state = states.and_then(|states| states.iter().find(|state| state["id"] == provider));

        if state.is_some_and(|state| state["state"] == "ready") {
            return None;
        }

        // Only the provider's own state counts here, not the first problem anywhere.
        let next = state
            .and_then(|state| {
                crate::status::next_action(&serde_json::json!({"plugin_states": [state]}))
            })
            .unwrap_or_else(|| "Check every part with `krabb status`.".into());

        Some(format!("Your Krabb is set up, but the {provider} provider is not ready.\n{next}"))
    }
}

/// The plugins installed with the place ID, beyond the place itself.
fn companions(id: &str) -> &'static [Offer] {
    if id == "tui" { &TERMINAL } else { &[] }
}

/// Why the companion plugin ID was not installed, from ERROR's first line, and its fix.
fn companion_failed(id: &str, error: &str) -> String {
    let reason = error.lines().next().unwrap_or_default().trim().trim_end_matches('.');

    let fix = format!("Install it later with `krabb plugin install {id}`.");

    if reason.is_empty() {
        format!("Could not install the {id} plugin.\n{fix}")
    } else {
        format!("Could not install the {id} plugin: {reason}.\n{fix}")
    }
}

/// The service actions that apply the service step: a stop first when the definition of an
/// installed service changed, because only a fresh start reads it, then a start, which does
/// nothing to a service that already runs.
fn actions(changed: bool) -> &'static [ServiceAction] {
    if changed { &[ServiceAction::Stop, ServiceAction::Start] } else { &[ServiceAction::Start] }
}

/// Installs the official plugin ID quietly, through the same path as `krabb plugin install`.
async fn fetch(id: &str) -> Result<(), Error> {
    let install = PluginInstall {
        ids: vec![id.into()],
        source: None,
        revision: None,
        link: false,
        unstable: false,
        yes: true,
        quiet: true,
    };

    crate::plugins::plugin(PluginCommand::Install(install), false).await?;
    Ok(())
}

/// What still keeps the Krabb from answering when the guide ends, checked in this order.
#[derive(Clone, Copy, Debug)]
enum Ending<'a> {
    Provider,
    Setup(&'a str),
    Service,
    Ready { terminal: bool },
}

/// How the guide ends: whether the Krabb can answer, then the one command to run next.
fn closing(ending: Ending<'_>) -> String {
    match ending {
        Ending::Ready { terminal: true } => {
            "Your Krabb is ready.\nChat with it in the terminal with `krabb tui`.".into()
        }

        Ending::Ready { terminal: false } => {
            "Your Krabb is ready.\nCheck every part with `krabb status`.".into()
        }

        Ending::Provider => "Your Krabb is set up, and it needs a provider to answer.\nAdd one \
                             with `krabb init provider`."
            .into(),

        Ending::Setup("codex") => "Your Krabb is set up, and Codex needs a sign-in to answer.\n\
                                   Sign in with `krabb codex login`."
            .into(),

        Ending::Setup(id) => format!(
            "Your Krabb is set up, and the {id} provider needs setup to answer.\nSet it up with \
             `krabb plugin setup {id}`."
        ),

        Ending::Service => "Your Krabb is set up, and it needs the background service to \
                            answer.\nStart it with `krabb init service`."
            .into(),
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
