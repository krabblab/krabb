use super::*;

pub(super) fn validate(
    job: &Job,
    config: &Config,
    clients: &clients::Registry,
) -> Result<(), PluginError> {
    if job.workspace.canonicalize().ok().as_ref() != Some(&job.workspace) {
        return Err(crate::failure::blocked(
            "The captured workspace is missing or has changed its location.",
        ));
    }

    let principal = job.origin.source();
    let approval = sharing::approval(config, &principal, config.approval_mode());

    if !approval.enabled() && (!job.foreground || job.tools || job.shell) {
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
                "private":job.origin.private, "sender":job.origin.sender,
                "roles":job.origin.roles, "thread":job.origin.thread,
            });

            if !allowed(&policy, &event, &job.origin.chat) {
                return Err(crate::failure::blocked(
                    "The originating conversation is no longer authorized.",
                ));
            }

            (
                tools_allowed(&policy, &event, &job.origin.chat, true),
                channel_shell_allowed(&policy, &event, &job.origin.chat, true, config.shell),
                policy.worktree,
            )
        }

        sharing::Principal::Agent(_) => {
            return Err(crate::failure::blocked("This job has no user endpoint."));
        }
    };

    if job.tools && !tools
        || job.shell && !(tools && shell)
        || (isolated || sharing::isolated(config, &principal))
            && !worktree::isolated(&job.workspace)
    {
        return Err(crate::failure::blocked(
            "The job's captured capabilities are no longer authorized.",
        ));
    }

    Ok(())
}
