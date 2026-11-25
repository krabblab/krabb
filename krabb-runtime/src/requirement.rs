//! External programs a plugin declares it needs, such as `signal-cli` or a container engine.

use super::*;

const HELP_LIMIT: usize = 300;
const REQUIREMENT_LIMIT: usize = 8;

/// One program a plugin runs. `setting` names an environment variable whose value overrides
/// `program`; with no `program`, the requirement applies only once that variable is set, and
/// `off` lists its values that need no program. Unknown keys are ignored for later releases.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Requirement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub off: Vec<String>,
    pub help: String,
}

pub(crate) fn validate(requirements: &[Requirement]) -> Result<(), String> {
    if requirements.len() > REQUIREMENT_LIMIT {
        return Err(format!("Plugins may declare at most {REQUIREMENT_LIMIT} requirements."));
    }

    for requirement in requirements {
        if requirement.program.is_none() && requirement.setting.is_none() {
            return Err("Each requirement needs a program or a setting.".into());
        }

        if requirement.program.as_deref().is_some_and(|program| !program_name(program)) {
            return Err("Required program names must be plain file names.".into());
        }

        if requirement.setting.as_deref().is_some_and(|setting| !setting.starts_with("KRABB_")) {
            return Err("Requirement settings must be KRABB_ environment variable names.".into());
        }

        if requirement.off.len() > REQUIREMENT_LIMIT
            || requirement.off.iter().any(|value| !program_name(value))
        {
            return Err("Requirement `off` values must be plain words.".into());
        }

        if !visible(&requirement.help) {
            return Err(format!(
                "Requirement help must be one line of visible text up to {HELP_LIMIT} bytes."
            ));
        }
    }

    Ok(())
}

/// Whether HELP is one line a terminal shows as written: no control, bidirectional, or
/// zero-width characters that could rewrite or reorder the screen.
pub(crate) fn visible(help: &str) -> bool {
    let help = help.trim();

    !help.is_empty() && help.len() <= HELP_LIMIT && !help.chars().any(hidden)
}

pub(crate) fn hidden(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// The help of the first requirement of each installed plugin that this environment lacks.
pub(crate) fn missing(root: &Path, lock: &Lock) -> BTreeMap<String, String> {
    let path = std::env::var("PATH").unwrap_or_default();

    lock.plugins
        .keys()
        .filter_map(|id| {
            let manifest = read_manifest(&root.join("plugins").join(id))?;
            let help = unmet(&manifest.requires, &path, setting_value)?;

            Some((id.clone(), help))
        })
        .collect()
}

fn unmet(
    requirements: &[Requirement],
    path: &str,
    setting: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    requirements.iter().find_map(|requirement| {
        let configured = requirement.setting.as_deref().and_then(&setting);

        if configured.as_ref().is_some_and(|value| requirement.off.contains(value)) {
            return None;
        }

        let program = configured.or_else(|| requirement.program.clone())?;

        locate(&program, path).is_none().then(|| requirement.help.trim().to_owned())
    })
}

/// The host passes these variables to plugins only from its own environment, never from the
/// saved store, so the check reads them the same way.
fn setting_value(setting: &str) -> Option<String> {
    std::env::var(setting)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Finds PROGRAM as a plugin would start it: an absolute path, or a file name on PATH.
/// Windows starts a bare name only as an `.exe`, so the lookup adds only that extension.
pub(crate) fn locate(program: &str, path: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);

    if candidate.is_absolute() {
        return executable(candidate);
    }

    if candidate.components().count() > 1 {
        return None;
    }

    let name = if cfg!(windows) && !program.to_ascii_lowercase().ends_with(".exe") {
        format!("{program}.exe")
    } else {
        program.to_owned()
    };

    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(&name))
        .find_map(|candidate| executable(&candidate))
}

fn executable(candidate: &Path) -> Option<PathBuf> {
    let metadata = std::fs::metadata(candidate).ok()?;

    if !metadata.is_file() {
        return None;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }

    candidate.canonicalize().ok()
}

fn program_name(program: &str) -> bool {
    !program.is_empty()
        && program.len() <= 64
        && program.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        && !program.starts_with('.')
}

#[cfg(test)]
mod tests;
