use super::*;

pub(super) fn validate(
    task: &Task,
    config: &Config,
    clients: &clients::Registry,
) -> Result<(), PluginError> {
    if task.workspace.canonicalize().ok().as_ref() != Some(&task.workspace) {
        return Err(crate::failure::blocked(
            "The captured workspace is missing or has changed its location.",
        ));
    }

    let principal = task.origin.source();
    let approval = sharing::approval(config, &principal, config.approval_mode());

    if !approval.enabled() && (!task.foreground || task.tools || task.shell) {
        return Err(crate::failure::blocked(
            "The originating approval policy no longer permits background work.",
        ));
    }

    let (tools, shell, isolated) = match &principal {
        sharing::Principal::Client(id) => {
            if !client::known(id) && !config.clients.contains_key(id) {
                return Err(crate::failure::blocked(
                    "The originating client is no longer configured.",
                ));
            }

            let policy = config.clients.get(id).cloned().unwrap_or_default();
            let live = clients.get(id, config)?;
            (
                live.tools.load(std::sync::atomic::Ordering::Acquire),
                live.shell.load(std::sync::atomic::Ordering::Acquire),
                policy.worktree,
            )
        }

        sharing::Principal::Channel(id) => {
            let mut policy = config.channels.get(id).cloned().ok_or_else(|| {
                crate::failure::blocked("The originating channel is no longer configured.")
            })?;
            policy.mention = None;
            policy.topic.clear();
            let event = serde_json::json!({
                "private":task.origin.private, "sender":task.origin.sender,
                "roles":task.origin.roles, "thread":task.origin.thread,
            });

            if !allowed(&policy, &event, &task.origin.chat) {
                return Err(crate::failure::blocked(
                    "The originating conversation is no longer authorized.",
                ));
            }

            (
                tools_allowed(&policy, &event, &task.origin.chat, true),
                channel_shell_allowed(&policy, &event, &task.origin.chat, true, config.shell),
                policy.worktree,
            )
        }

        sharing::Principal::Agent(_) => {
            return Err(crate::failure::blocked("This task has no user endpoint."));
        }
    };

    if task.tools && !tools
        || task.shell && !(tools && shell)
        || (isolated || sharing::isolated(config, &principal))
            && !worktree::isolated(&task.workspace)
    {
        return Err(crate::failure::blocked(
            "The task's captured capabilities are no longer authorized.",
        ));
    }

    Ok(())
}
