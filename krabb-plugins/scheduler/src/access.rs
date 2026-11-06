use krabb_core::scope::{Access, Owner, Principal};
use serde_json::Value;

fn owner(value: &Value) -> Option<Owner> {
    let channel = value["channel"].as_str()?;

    let source = if channel == "tui" {
        Principal::Client("tui".into())
    } else {
        Principal::try_from(format!("channels.{channel}")).ok()?
    };

    let conversation = value["session"].as_str()?.to_owned();

    (!conversation.is_empty()).then_some(Owner { source, conversation })
}

pub(super) fn validate(caller: &Value) -> krabb_core::Result<()> {
    if ["channel", "chat", "session"]
        .iter()
        .any(|key| caller[*key].as_str().is_none_or(|value| value.is_empty() || value.len() > 4096))
        || !(caller["thread"].is_null() || caller["thread"].is_string())
    {
        return Err(krabb_core::Error::Denied("Schedule caller is invalid.".into()));
    }

    if let Some(value) = caller.get("access") {
        let access: Access = serde_json::from_value(value.clone())
            .map_err(|_| krabb_core::Error::Denied("Schedule access is invalid.".into()))?;
        access.validate().map_err(krabb_core::Error::Denied)?;

        if owner(caller).as_ref() != Some(&access.owner) {
            return Err(krabb_core::Error::Denied(
                "Schedule access owner does not match its caller.".into(),
            ));
        }
    }

    Ok(())
}

pub(super) fn authorize(origin: &Value, caller: Option<&Value>) -> krabb_core::Result<()> {
    let Some(caller) = caller else {
        return Ok(());
    };

    validate(caller)?;

    let owns = ["channel", "chat", "session", "thread"]
        .iter()
        .all(|field| caller[*field] == origin[*field]);
    let shared = caller
        .get("access")
        .and_then(|value| serde_json::from_value::<Access>(value.clone()).ok())
        .zip(owner(origin))
        .is_some_and(|(access, owner)| access.allows(&owner));

    if owns || shared {
        Ok(())
    } else {
        Err(krabb_core::Error::Denied("Scheduled task belongs to another conversation.".into()))
    }
}
