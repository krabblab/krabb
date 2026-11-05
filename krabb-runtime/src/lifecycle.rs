use super::*;

pub(super) async fn run(
    root: &Path,
    id: &str,
    attach: bool,
    json: bool,
) -> Result<(), PluginError> {
    let value = change(root, id, attach).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if attach {
        println!("Attached plugin {id}.");
    } else {
        println!(
            "Detached plugin {id}. It remains installed; reattach with `krabb plugin attach {id}`."
        );
    }

    Ok(())
}

pub(super) async fn change(root: &Path, id: &str, attach: bool) -> Result<Value, PluginError> {
    if !valid(id) {
        return Err(format!("Invalid plugin ID: {id}.").into());
    }

    if !load_lock_at(root)?.plugins.contains_key(id) {
        return Err(format!("Plugin is not installed: {id}.").into());
    }

    let method = if attach { "plugin.attach" } else { "plugin.detach" };

    if let Some(value) = control_at(method, serde_json::json!({"id": id}), root).await? {
        return Ok(value);
    }

    let _offline = offline_lock(root)?;

    if attach {
        return Err("The daemon is not running. Start it before attaching a plugin.".into());
    }

    Ok(serde_json::json!({"id": id, "unloaded": false}))
}
