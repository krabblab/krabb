use super::Findings;
use crate::command::reason::{Effect, Reason};

const ALIASES: &[(&str, &str)] = &[
    ("ls", "get-childitem"),
    ("dir", "get-childitem"),
    ("gci", "get-childitem"),
    ("cat", "get-content"),
    ("type", "get-content"),
    ("gc", "get-content"),
    ("sls", "select-string"),
    ("pwd", "get-location"),
    ("gl", "get-location"),
    ("gi", "get-item"),
    ("echo", "write-output"),
    ("write", "write-output"),
    ("measure", "measure-object"),
    ("select", "select-object"),
    ("sort", "sort-object"),
    ("rm", "remove-item"),
    ("del", "remove-item"),
    ("erase", "remove-item"),
    ("rd", "remove-item"),
    ("rmdir", "remove-item"),
    ("ri", "remove-item"),
    ("cp", "copy-item"),
    ("copy", "copy-item"),
    ("cpi", "copy-item"),
    ("mv", "move-item"),
    ("move", "move-item"),
    ("mi", "move-item"),
    ("ren", "rename-item"),
    ("rni", "rename-item"),
    ("ni", "new-item"),
    ("mkdir", "new-item"),
    ("md", "new-item"),
    ("sc", "set-content"),
    ("ac", "add-content"),
    ("clc", "clear-content"),
    ("tee", "tee-object"),
    ("saps", "start-process"),
    ("start", "start-process"),
];

const READS: &[&str] = &[
    "get-childitem",
    "get-content",
    "select-string",
    "test-path",
    "get-location",
    "get-item",
    "get-date",
    "get-filehash",
    "resolve-path",
    "write-output",
    "measure-object",
    "select-object",
    "sort-object",
    "findstr",
];

const VARIABLES: &[&str] =
    &["outvariable", "errorvariable", "warningvariable", "informationvariable", "pipelinevariable"];

const SHORT_VARIABLES: &[&str] = &["ov", "ev", "wv", "iv", "pv"];

const WRITE_VERBS: &[&str] =
    &["set-", "new-", "add-", "copy-", "move-", "rename-", "clear-", "export-"];

const WRITE_CMDLETS: &[&str] = &["out-file", "tee-object"];

const SERVICE_VERBS: &[&str] =
    &["start-", "stop-", "restart-", "suspend-", "resume-", "set-", "new-", "remove-"];

pub(super) fn evaluate(name: &str, args: &[&str], found: &mut Findings) -> bool {
    let name = ALIASES.iter().find(|(alias, _)| *alias == name).map_or(name, |(_, cmdlet)| cmdlet);

    let effect = if READS.contains(&name) {
        if args.iter().any(|arg| sets_variable(arg)) {
            found.acts(name, Effect::SetsVariable);
            return true;
        }

        Effect::Reads
    } else if name == "start-process" {
        let elevated = args.windows(2).any(|pair| {
            pair[0].eq_ignore_ascii_case("-verb") && pair[1].eq_ignore_ascii_case("runas")
        });

        if elevated {
            found.push(Reason::Elevated(name.to_owned()));
        }

        Effect::RunsPrograms
    } else if name == "get-service" {
        Effect::QueriesServices
    } else if name.ends_with("-service") && SERVICE_VERBS.iter().any(|verb| name.starts_with(verb))
    {
        Effect::ManagesServices
    } else if name.starts_with("remove-") {
        Effect::DeletesFiles
    } else if WRITE_CMDLETS.contains(&name) || WRITE_VERBS.iter().any(|verb| name.starts_with(verb))
    {
        Effect::ChangesFiles
    } else {
        return false;
    };

    found.acts(name, effect);
    true
}

fn sets_variable(arg: &str) -> bool {
    let Some(parameter) = arg.strip_prefix('-') else {
        return false;
    };

    let parameter = parameter.split_once(':').map_or(parameter, |(name, _)| name);
    let parameter = parameter.to_ascii_lowercase();

    !parameter.is_empty()
        && (SHORT_VARIABLES.contains(&parameter.as_str())
            || VARIABLES.iter().any(|long| long.starts_with(&parameter)))
}
