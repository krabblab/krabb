mod destinations;
mod runs;
pub(super) use destinations::{destination, parse, schema, update};
pub(super) use runs::{destinations, publish};

use super::*;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Origin {
    pub channel: String,
    pub chat: String,
    pub session: String,
    pub thread: Option<String>,
    pub private: Option<bool>,
    pub sender: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

impl Origin {
    pub fn scope(&self) -> String {
        conversation_scope(&self.channel, &self.chat, self.thread.as_deref())
    }
}

pub fn origin(context: &Value) -> Result<Option<Origin>, PluginError> {
    if context["origin"].is_null() || context["origin"]["channel"] == "cli" {
        return Ok(None);
    }

    let mut value = context["origin"].clone();

    if value["channel"] == "tui" && value["chat"].is_null() {
        value["chat"] = value["session"].clone();
    }

    let origin: Origin =
        serde_json::from_value(value).map_err(|_| "PAUSED: The schedule's origin is invalid.")?;

    if !valid(&origin.channel) || !valid(&origin.session) || origin.chat.is_empty() {
        return Err("PAUSED: The schedule's origin is invalid.".into());
    }

    Ok(Some(origin))
}

pub fn policy(config: &Config, context: &Value, workspace: &Path) -> Result<(), PluginError> {
    destinations(context)?;

    if worktree::isolated(workspace) {
        return Err("PAUSED: Scheduled execution cannot enforce worktree isolation.".into());
    }

    let origin = origin(context)?;
    let Some(origin) = origin.filter(|origin| origin.channel != "cli") else {
        if context["shell"] == true && !config.tui_shell() {
            return Err("PAUSED: Shell access for this schedule is now disabled.".into());
        }

        return Ok(());
    };

    let owner = sharing::principal(&origin.channel);

    if sharing::isolated(config, &owner) {
        return Err("PAUSED: The originating integration now requires worktree isolation.".into());
    }

    let (tools, shell) = if origin.channel == "tui" {
        let local = config.clients.get("tui").cloned().unwrap_or_default();
        (local.tools, config.tui_shell())
    } else {
        let mut local = config
            .channels
            .get(&origin.channel)
            .cloned()
            .ok_or("PAUSED: The originating channel is no longer configured.")?;
        local.mention = None;
        local.topic.clear();

        let event = serde_json::json!({
            "private":origin.private.unwrap_or(false), "sender":origin.sender,
            "roles":origin.roles, "thread":origin.thread
        });

        if !allowed(&local, &event, &origin.chat) {
            return Err(
                "PAUSED: The schedule's originating conversation is no longer authorized.".into()
            );
        }

        (local.tools, local.shell)
    };

    if !sharing::approval(config, &owner, config.approval_mode()).enabled() {
        return Err("PAUSED: The originating approval policy now blocks scheduled actions.".into());
    }

    if (context["tools"] == true && !tools) || (context["shell"] == true && !(tools && shell)) {
        return Err("PAUSED: The schedule's captured capabilities are no longer authorized.".into());
    }

    Ok(())
}

pub async fn reconcile(
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
    stop: &Stop,
    call: &mut u64,
) {
    let pending: Vec<_> = match sessions.lock() {
        Ok(store) => {
            store.runs.values().filter(|run| !run.reported).map(|run| (run.job, run.id)).collect()
        }

        Err(_) => return,
    };

    if pending.is_empty() {
        return;
    }

    let Some((_, scheduler)) = plugins.find(Capability::Scheduler).await else {
        return;
    };

    let start = (*call as usize) % pending.len();

    for (job, run) in pending.iter().cycle().skip(start).take(pending.len().min(16)) {
        let request = *call;
        *call = call.saturating_add(1);
        let Some(Ok(response)) = scheduler_call(
            &scheduler,
            request,
            "receipt",
            serde_json::json!({
                "id":job, "run_id":run
            }),
            stop,
        )
        .await
        else {
            return;
        };

        if response.error.is_some() {
            return;
        }

        if response.result.is_some_and(|value| value["reported"] == true)
            && let Ok(mut store) = sessions.lock()
            && let Err(error) = store.report_run(*job, *run)
        {
            warn!(schedule = job, %error, "Schedule receipt could not be reconciled.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_rechecks_origin_policy_without_granting_new_capabilities() {
        let mut config: Config = toml::from_str(
            r#"
            approval = "auto"
            [channels.telegram]
            allow = ["room"]
            tools = true
        "#,
        )
        .unwrap();
        let context = serde_json::json!({
            "origin":{"channel":"telegram","chat":"room","session":"telegram-room","private":false},
            "tools":true,"shell":false
        });

        let workspace = Path::new("workspace");

        policy(&config, &context, workspace).unwrap();
        config.channels.get_mut("telegram").unwrap().tools = false;

        assert!(policy(&config, &context, workspace).is_err());
        config.visibility = vec![sharing::Rule {
            data: vec![sharing::Data::Permissions],
            resources: vec![krabb_core::visibility::Resource::Global],
        }];
        config.tools = true;

        assert!(policy(&config, &context, workspace).is_err());
        config.channels.get_mut("telegram").unwrap().tools = true;

        policy(&config, &context, workspace).unwrap();
        config.channels.get_mut("telegram").unwrap().worktree = true;

        assert!(policy(&config, &context, workspace).is_err());
        config.channels.get_mut("telegram").unwrap().worktree = false;
        config.channels.get_mut("telegram").unwrap().allow.clear();

        assert!(policy(&config, &context, workspace).is_err());
        assert!(policy(&config, &context, Path::new("workspace/.krabb/worktrees/chat")).is_err());
    }
}
