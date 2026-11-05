use super::*;

#[derive(Debug, Default, Args)]
pub(super) struct Drop {
    #[arg(required_unless_present = "all", conflicts_with = "all", help = "Delivery identifier.")]
    pub id: Option<String>,
    #[arg(long, help = "Drop every matching pending or uncertain delivery.")]
    pub all: bool,
    #[arg(long, requires = "all", help = "Limit --all to this channel plugin.")]
    pub channel: Option<String>,
    #[arg(short, long, help = "Skip the confirmation prompt.")]
    pub yes: bool,
}

pub(super) async fn drop(args: Drop, root: &Path, json: bool) -> Result<(), PluginError> {
    let prompt = if let Some(id) = &args.id {
        format!("Drop delivery {id} without retrying it?")
    } else if let Some(channel) = &args.channel {
        format!("Drop all pending and uncertain deliveries for {channel} without retrying them?")
    } else {
        "Drop all pending and uncertain deliveries across every channel without retrying them?"
            .into()
    };

    if !confirm_action(args.yes, json, &prompt)? {
        print_cancelled("Drop", json);
        return Ok(());
    }

    let params = serde_json::json!({
        "id": args.id, "all": args.all, "channel": args.channel, "yes": true,
    });

    let value = if let Some(value) = control_at("delivery.drop", params.clone(), root).await? {
        value
    } else {
        let _offline = offline_lock(root)?;
        apply(&mut sessions_at(root)?, &params)?
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if let Some(id) = value["id"].as_str() {
        println!("Dropped delivery {id}.");
    } else {
        println!("Dropped {} deliveries.", value["count"]);
    }

    Ok(())
}

pub(super) fn apply(store: &mut state::Store, params: &Value) -> std::io::Result<Value> {
    let invalid = |message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
    let id = params["id"].as_str().filter(|id| !id.is_empty());
    let all = params["all"] == true;
    let channel = params["channel"].as_str();

    if params["yes"] != true {
        return Err(invalid("Delivery drop requires confirmation."));
    }

    if id.is_some() == all
        || (!params["id"].is_null() && id.is_none())
        || (!params["all"].is_null() && !params["all"].is_boolean())
        || (!params["channel"].is_null() && channel.is_none())
        || channel.is_some_and(|channel| !all || !valid(channel))
    {
        return Err(invalid("Choose a delivery id or all=true, with an optional channel for all."));
    }

    if let Some(id) = id {
        store.drop_delivery(id)?;
        return Ok(serde_json::json!({"id": id, "status": "dropped"}));
    }

    let count = store.drop_deliveries(channel)?;

    Ok(serde_json::json!({"count": count, "channel": channel, "status": "dropped"}))
}
