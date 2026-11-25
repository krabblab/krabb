//! External programs a plugin declares it needs, such as `signal-cli` or a container engine.

use super::*;

const HELP_LIMIT: usize = 300;
const REQUIREMENT_LIMIT: usize = 8;

/// One program a plugin runs. `setting` names a value that overrides `program`; with no
/// `program`, the requirement applies only once that setting is set.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Requirement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
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

        let help = requirement.help.trim();

        if help.is_empty() || help.len() > HELP_LIMIT || help.chars().any(char::is_control) {
            return Err(format!(
                "Requirement help must be one line of visible text up to {HELP_LIMIT} bytes."
            ));
        }
    }

    Ok(())
}

/// The help of the first requirement of each installed plugin that this environment lacks.
pub(crate) fn missing(root: &Path, lock: &Lock) -> BTreeMap<String, String> {
    let path = std::env::var("PATH").unwrap_or_default();

    lock.plugins
        .keys()
        .filter_map(|id| {
            let manifest = read_manifest(&root.join("plugins").join(id))?;
            let help = lacking(root, &manifest.requires, &path)?;

            Some((id.clone(), help))
        })
        .collect()
}

fn lacking(root: &Path, requirements: &[Requirement], path: &str) -> Option<String> {
    requirements.iter().find_map(|requirement| {
        let configured = requirement.setting.as_ref().and_then(|setting| value(root, setting));
        let program = configured.or_else(|| requirement.program.clone())?;

        found(&program, path).is_none().then(|| requirement.help.trim().to_owned())
    })
}

/// A setting's value from the environment or the saved store, as the plugin would see it.
fn value(root: &Path, setting: &str) -> Option<String> {
    let names = [setting.to_owned()];

    credentials::resolve(&names, root)
        .ok()
        .and_then(|values| values.get(setting).cloned())
        .or_else(|| std::env::var(setting).ok())
        .filter(|value| !value.trim().is_empty())
}

/// Finds PROGRAM, either as a path or as a file name on PATH, and returns where it is.
pub(crate) fn found(program: &str, path: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);

    if candidate.components().count() > 1 {
        return executable(candidate);
    }

    let names = if cfg!(windows) && candidate.extension().is_none() {
        vec![format!("{program}.exe"), format!("{program}.cmd")]
    } else {
        vec![program.to_owned()]
    };

    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
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
