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

pub(super) struct Spec<'a> {
    pub flags: &'a str,
    pub values: &'a str,
    pub long_flags: &'a [&'a str],
    pub long_values: &'a [&'a str],
}

#[derive(Debug, Default)]
pub(super) struct Walk {
    pub end: usize,
    pub seen: Vec<String>,
}

impl Walk {
    pub fn has(&self, options: &[&str]) -> bool {
        self.seen.iter().any(|seen| options.contains(&seen.as_str()))
    }
}

pub(super) fn walk(args: &[&str], spec: &Spec<'_>) -> Result<Walk, String> {
    let mut walk = Walk::default();

    while let Some(&arg) = args.get(walk.end) {
        if arg == "--" {
            walk.end += 1;
            break;
        }

        if !arg.starts_with('-') || arg == "-" {
            break;
        }

        walk.end += 1;

        if let Some(name) = arg.strip_prefix("--") {
            let (name, attached) = name.split_once('=').map_or((name, false), |(n, _)| (n, true));
            let option = resolve(name, spec).ok_or_else(|| format!("--{name}"))?;

            if spec.long_values.contains(&option) && !attached {
                walk.end += 1;
            }

            walk.seen.push(format!("--{option}"));
            continue;
        }

        for (at, flag) in arg.char_indices().skip(1) {
            walk.seen.push(format!("-{flag}"));

            if spec.values.contains(flag) {
                walk.end += usize::from(at + flag.len_utf8() == arg.len());
                break;
            }

            if !spec.flags.contains(flag) {
                return Err(format!("-{flag}"));
            }
        }
    }

    Ok(walk)
}

fn resolve<'a>(name: &str, spec: &Spec<'a>) -> Option<&'a str> {
    let options = spec.long_flags.iter().chain(spec.long_values);

    if let Some(exact) = options.clone().find(|option| **option == name) {
        return Some(exact);
    }

    let mut matches = options.filter(|option| !name.is_empty() && option.starts_with(name));
    let found = matches.next()?;

    matches.next().is_none().then_some(*found)
}
