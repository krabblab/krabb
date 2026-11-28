pub(super) fn long(arg: &str, option: &str) -> bool {
    let Some(name) = arg.strip_prefix("--") else {
        return false;
    };

    let name = name.split_once('=').map_or(name, |(name, _)| name);

    !name.is_empty() && option.starts_with(name)
}

pub(super) fn takes_value(arg: &str, short: &[&str], long_values: &[&str]) -> bool {
    if short.contains(&arg) {
        return true;
    }

    !arg.contains('=') && long_values.iter().any(|option| long(arg, option))
}

pub(super) fn cluster(arg: &str) -> Option<&str> {
    arg.strip_prefix('-').filter(|flags| !flags.is_empty() && !flags.starts_with('-'))
}
