use super::Findings;
use crate::command::reason::Effect;

const READS: &[&str] = &["dir", "type", "findstr", "where", "ver", "echo"];
const DELETES: &[&str] = &["del", "erase", "rd", "rmdir"];

const WRITES: &[&str] = &[
    "copy", "move", "xcopy", "robocopy", "ren", "rename", "md", "mkdir", "mklink", "attrib",
    "icacls", "takeown",
];

pub(super) fn evaluate(name: &str, args: &[&str], found: &mut Findings) -> bool {
    let effect = if READS.contains(&name) {
        Effect::Reads
    } else if DELETES.contains(&name) {
        Effect::DeletesFiles
    } else if WRITES.contains(&name) {
        Effect::ChangesFiles
    } else if name == "net" {
        return net(args, found);
    } else {
        return false;
    };

    found.acts(name, effect);
    true
}

fn net(args: &[&str], found: &mut Findings) -> bool {
    let action = args.first().map(|arg| arg.to_ascii_lowercase());

    let (command, effect) = match action.as_deref() {
        Some("start" | "stop" | "pause" | "continue") if args.len() > 1 => {
            ("net start", Effect::ManagesServices)
        }

        Some("start") => ("net start", Effect::QueriesServices),
        Some("use" | "view") => ("net use", Effect::UsesNetwork),
        _ => ("net", Effect::Unlisted),
    };

    found.acts(command, effect);
    true
}
