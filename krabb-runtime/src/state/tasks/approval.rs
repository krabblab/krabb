use super::*;
use krabb_core::task::Run;
use serde_json::Value;

const TEXT_LIMIT: usize = 4 * 1024;
const ARGS_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Approval {
    pub id: String,
    pub tool: String,
    pub args: Value,
    pub text: String,
    pub generation: u64,
    #[serde(default)]
    pub decision: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub started: bool,
}

impl Approval {
    pub fn validate(&self) -> std::io::Result<()> {
        let size = serde_json::to_vec(&self.args).map_err(std::io::Error::other)?.len();

        if !valid(&self.id)
            || self.tool.is_empty()
            || self.tool.len() > 128
            || self.text.trim().is_empty()
            || self.text.len() > TEXT_LIMIT
            || size > ARGS_LIMIT
            || !self.args.is_object()
        {
            return Err(std::io::Error::other("Invalid pending approval."));
        }

        Ok(())
    }

    fn notice(&self) -> String {
        format!(
            "Approval needed before running {}: {}\nReply \"approve\" or \"deny\" in this \
                conversation. Nothing runs until you decide.",
            self.tool, self.text
        )
    }
}

impl Store {
    pub fn request_approval(
        &mut self,
        run: &Run,
        call: u64,
        tool: &str,
        args: &Value,
        text: &str,
    ) -> std::io::Result<()> {
        self.save_approval(run, call, tool, args, text, true)
    }

    pub fn propose_approval(
        &mut self,
        run: &Run,
        call: u64,
        tool: &str,
        args: &Value,
        text: &str,
    ) -> std::io::Result<()> {
        self.save_approval(run, call, tool, args, text, false)
    }

    fn save_approval(
        &mut self,
        run: &Run,
        call: u64,
        tool: &str,
        args: &Value,
        text: &str,
        blocked: bool,
    ) -> std::io::Result<()> {
        self.check_task_step(&run.id, run.generation)?;
        let task = &self.tasks[&run.id];

        let refusal = if !task.tools {
            Some(
                "This conversation has no tool access, so actions that need approval cannot \
                 run here.\nThe owner can allow them with `tools = true` and an approval mode \
                 other than off in this conversation's section of config.toml.",
            )
        } else if task.approval.is_some() {
            Some("Another approval is already waiting, so wait for that decision first.")
        } else if task.uncertain {
            Some("The task stopped at an uncertain step and cannot request approval.")
        } else {
            None
        };

        if let Some(refusal) = refusal {
            return Err(std::io::Error::other(refusal));
        }

        let approval = Approval {
            id: format!("{}-approval-{}-{call}", run.id, run.generation),
            tool: tool.into(),
            args: args.clone(),
            text: crate::clip(text.into(), TEXT_LIMIT),
            generation: run.generation,
            decision: None,
            started: false,
        };

        approval.validate()?;
        self.change(|store| {
            let task = store.tasks.get_mut(&run.id).unwrap();
            task.revision = task.revision.saturating_add(1);
            task.updated = now();
            task.approval = Some(approval);

            if blocked {
                task.status = TaskStatus::Blocked;
                Store::approval_notice(task);
            }
        })
    }

    pub fn withdraw_approval(&mut self, run: &Run, call: u64) -> std::io::Result<()> {
        let id = format!("{}-approval-{}-{call}", run.id, run.generation);

        if self
            .tasks
            .get(&run.id)
            .is_none_or(|task| task.approval.as_ref().is_none_or(|approval| approval.id != id))
        {
            return Ok(());
        }

        self.change(|store| {
            let task = store.tasks.get_mut(&run.id).unwrap();
            task.approval = None;
            task.revision = task.revision.saturating_add(1);
        })
    }

    pub fn escalate_approval(&mut self, run: &Run) -> std::io::Result<()> {
        self.check_task_step(&run.id, run.generation)?;

        if self.tasks[&run.id].approval.as_ref().is_none_or(|approval| approval.decision.is_some())
        {
            return Err(std::io::Error::other("The approval is superseded."));
        }

        self.change(|store| {
            let task = store.tasks.get_mut(&run.id).unwrap();
            task.status = TaskStatus::Blocked;
            task.revision = task.revision.saturating_add(1);
            task.updated = now();
            Store::approval_notice(task);
        })
    }

    pub fn decided_approval(&self, run: &Run) -> Option<Approval> {
        self.tasks
            .get(&run.id)
            .and_then(|task| task.approval.clone())
            .filter(|approval| approval.decision.is_some())
    }

    pub fn settle_approval(
        &mut self,
        run: &Run,
        id: &str,
        message: Message,
    ) -> std::io::Result<()> {
        self.check_task_step(&run.id, run.generation)?;

        if self.tasks[&run.id].approval.as_ref().is_none_or(|approval| approval.id != id) {
            return Err(std::io::Error::other("The approval is superseded."));
        }

        self.change(|store| {
            let task = store.tasks.get_mut(&run.id).unwrap();
            task.approval = None;
            task.revision = task.revision.saturating_add(1);

            if let Some(journal) = store.sessions.get_mut(&run.id) {
                journal.messages.push(message);
                compact_messages(&mut journal.messages);
            }
        })
    }

    /// Fences an approved action before it runs so that no later step can run it again.
    pub fn start_approval(&mut self, run: &Run, id: &str) -> std::io::Result<()> {
        self.check_task_step(&run.id, run.generation)?;

        if !self.tasks[&run.id]
            .approval
            .as_ref()
            .is_some_and(|approval| approval.id == id && approval.decision == Some(true))
        {
            return Err(std::io::Error::other("The approval is superseded."));
        }

        self.change(|store| {
            let task = store.tasks.get_mut(&run.id).unwrap();
            task.approval.as_mut().unwrap().started = true;
            task.revision = task.revision.saturating_add(1);
        })
    }

    /// Retires a decided approval after its action ran and handed the task off.
    pub fn retire_approval(&mut self, id: &str, approval: &str) -> std::io::Result<()> {
        let decided = self
            .tasks
            .get(id)
            .and_then(|task| task.approval.as_ref())
            .is_some_and(|saved| saved.id == approval && saved.decision.is_some());

        if !decided {
            return Ok(());
        }

        self.change(|store| {
            let task = store.tasks.get_mut(id).unwrap();
            task.approval = None;
            task.revision = task.revision.saturating_add(1);
        })
    }

    pub(super) fn approval_notice(task: &mut Task) {
        if let Some(approval) =
            task.approval.as_ref().filter(|approval| approval.decision.is_none())
        {
            task.notice = Some(Notice {
                id: format!("{}-{}", approval.id, task.revision),
                text: approval.notice(),
                model: false,
            });
        }
    }
}
