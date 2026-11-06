use super::*;
use krabb_core::schedule::{Destination, validate_destinations};

pub fn schema() -> Value {
    serde_json::json!({
        "type":"array", "maxItems":krabb_core::schedule::DESTINATION_LIMIT,
        "items":{
            "type":"object", "additionalProperties":false,
            "properties":{
                "channel":{"type":"string"}, "chat":{"type":"string"},
                "private":{"type":"boolean"}, "thread":{"type":"string"}
            },
            "required":["channel","chat","private"]
        }
    })
}

pub fn parse(value: &Value) -> Result<Vec<Destination>, PluginError> {
    let destinations: Vec<Destination> = serde_json::from_value(value.clone())?;

    validate_destinations(&destinations)?;

    Ok(destinations)
}

pub fn destination(value: &str) -> Result<Destination, PluginError> {
    value.parse::<Destination>().map_err(Into::into)
}

pub fn update(args: &[String]) -> Result<Vec<Destination>, PluginError> {
    if args == ["--clear"] {
        return Ok(Vec::new());
    }

    if args.is_empty() || !args.len().is_multiple_of(2) {
        return Err("Use --to <destination> (repeatable), or --clear.".into());
    }

    let mut destinations = Vec::new();

    for pair in args.chunks_exact(2) {
        if pair[0] != "--to" {
            return Err("Use --to <destination> (repeatable), or --clear.".into());
        }

        destinations.push(destination(&pair[1])?);
    }

    validate_destinations(&destinations)?;

    Ok(destinations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_capture_preserves_multiple_destinations_without_inferred_legacy_send() {
        let home = crate::tests::test_root("cli-destinations");

        crate::init_at(&home).unwrap();

        let args = [
            "--after",
            "5m",
            "--to",
            "telegram:private:123",
            "--to",
            "tui:group:tui-default",
            "Write to the default TUI chat",
        ]
        .map(str::to_owned);

        let (params, _) = crate::parse_schedule_add(&args, &home).unwrap();

        assert_eq!(params["context"]["destinations"].as_array().unwrap().len(), 2);
        assert!(params["context"]["message_destination"].is_null());

        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn destinations_are_explicit_bounded_and_unambiguous() {
        let args =
            ["--to", "telegram:private:123", "--to", "discord:group:456:789"].map(str::to_owned);

        let targets = update(&args).unwrap();

        assert_eq!(targets.len(), 2);
        assert!(targets[0].private);
        assert_eq!(targets[1].thread.as_deref(), Some("789"));
        assert!(update(&["--clear".into()]).unwrap().is_empty());
        assert!(update(&[]).is_err());
        assert!(update(&["--clear".into(), "--to".into()]).is_err());
        assert!(destination("telegram:123").is_err());
        assert!(destination("tui:private:tui-default").is_err());
        assert!(parse(&serde_json::json!([targets[0], targets[0]])).is_err());
        assert!(parse(&serde_json::json!([{"channel":"telegram","chat":"1"}])).is_err());
    }
}
