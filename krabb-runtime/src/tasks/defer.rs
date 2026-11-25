use krabb_core::task::defer::When;

pub(super) fn parse(value: &str) -> Result<When, String> {
    let (at, timezone) = match value.parse::<jiff::Timestamp>() {
        Ok(timestamp) => instant(value, timestamp)?,

        Err(_) => {
            let (datetime, zone) = value
                .rsplit_once(' ')
                .ok_or("Include a date, time, and explicit timezone or UTC offset.")?;

            (krabb_date::parse_exact_datetime(datetime.trim(), zone.trim())?, zone.trim().into())
        }
    };

    let when = When { at, timezone, expression: None };

    when.validate().map_err(|error| error.to_string())?;
    Ok(when)
}

/// Reads a timestamp with its own UTC offset, keeping its zone annotation or offset as the
/// timezone.
fn instant(value: &str, timestamp: jiff::Timestamp) -> Result<(u64, String), String> {
    if timestamp.subsec_nanosecond() != 0 {
        return Err("Use a whole-second resume time.".into());
    }

    let at = u64::try_from(timestamp.as_second())
        .map_err(|_| "Choose a future resume time.".to_owned())?;

    if let Some((_, zone)) = value.strip_suffix(']').and_then(|rest| rest.rsplit_once('[')) {
        return Ok((at, zone.trim_start_matches('!').into()));
    }

    if value.ends_with(['Z', 'z']) {
        return Ok((at, "UTC".into()));
    }

    let offset = value
        .rfind(['+', '-'])
        .filter(|at| *at > 9)
        .ok_or("Include an explicit UTC offset or named timezone.")?;

    Ok((at, value[offset..].to_owned()))
}

pub(super) fn describe(when: &When) -> String {
    let at = krabb_date::format_datetime_in_timezone(when.at, &when.timezone);

    match &when.expression {
        Some(expression) => format!("{at} (from \"{expression}\")"),
        None => at,
    }
}
