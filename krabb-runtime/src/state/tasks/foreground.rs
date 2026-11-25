use super::*;
use krabb_core::task::Run;

impl Store {
    pub fn admit_foreground(&mut self, task: Task, history: Vec<Message>) -> std::io::Result<Run> {
        self.admit_request(task, history, None)
    }

    pub fn queue_foreground(
        &mut self,
        task: Task,
        history: Vec<Message>,
        input: Message,
    ) -> std::io::Result<Run> {
        self.admit_request(task, history, Some(input))
    }

    fn admit_request(
        &mut self,
        mut task: Task,
        history: Vec<Message>,
        input: Option<Message>,
    ) -> std::io::Result<Run> {
        if self.sessions.contains_key(&task.id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Task ID already exists.",
            ));
        }

        let user = history.iter().rev().find(|message| message.role == Role::User);

        if input.as_ref().is_some_and(|input| user != Some(input)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "The queued input does not match its saved request context.",
            ));
        }

        task.request = user.map(|message| message.id.clone());

        let original = task
            .request
            .as_ref()
            .map(|request| {
                let message = input
                    .as_ref()
                    .or_else(|| {
                        self.sessions.get(&task.origin.session).and_then(|session| {
                            session.messages.iter().rev().find(|message| {
                                message.id == *request && message.role == Role::User
                            })
                        })
                    })
                    .or_else(|| history.iter().rev().find(|message| message.role == Role::User))
                    .ok_or_else(|| std::io::Error::other("The original request is missing."))?;
                Ok::<_, std::io::Error>(message.clone())
            })
            .transpose()?;

        task.input = original.as_ref().map(crate::state::Input::capture).transpose()?;

        task.foreground = true;

        if task.request.as_ref().is_some_and(|request| {
            self.tasks.values().any(|existing| {
                existing.origin.session == task.origin.session
                    && existing.request.as_ref() == Some(request)
            })
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "This request already has an accepted task.",
            ));
        }

        let saved = self
            .sessions
            .get(&task.origin.session)
            .map(Session::context)
            .unwrap_or_default()
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();

        let through =
            input.as_ref().map(|input| input.id.clone()).or_else(|| saved.last().cloned());

        let mut extras = history
            .iter()
            .filter(|message| {
                !saved.contains(&message.id)
                    && Some(&message.id) != task.request.as_ref()
                    && !message.id.starts_with(crate::state::SYSTEM_CONTEXT)
            })
            .cloned()
            .collect::<Vec<_>>();

        extras.sort_by_key(|message| message.role != Role::System);

        let queued = input.is_some();

        task.status = if queued { TaskStatus::Queued } else { TaskStatus::Running };

        task.step = if queued { 0 } else { 1 };

        task.revision = if queued { 0 } else { 1 };

        let run = Run { id: task.id.clone(), generation: task.step };

        let retire = if self.tasks.len() >= LIMIT {
            Some(
                self.tasks
                    .values()
                    .filter(|task| !task.protected())
                    .min_by_key(|task| task.updated)
                    .ok_or_else(|| std::io::Error::other("Task capacity is full."))?
                    .id
                    .clone(),
            )
        } else {
            None
        };

        if retire.is_none() && self.work_session_count() >= WORK_SESSION_LIMIT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Task journal capacity is full.",
            ));
        }

        if let Some(input) = &input {
            let source = self.session(&input.session)?;

            if source.is_work() || source.messages.iter().any(|message| message.id == input.id) {
                return Err(std::io::Error::other("The request identity is already in use."));
            }
        }

        self.change(|store| {
            if let Some(input) = input {
                let source = store.sessions.get_mut(&input.session).unwrap();
                source.messages.push(input);
                compact_messages(&mut source.messages);
            }

            if let Some(retire) = retire {
                store.retire_task(&retire);
            }

            let mut session =
                Session::new(task.id.clone(), task.model.clone(), SessionKind::Foreground, true);
            session.origin = Some(task.origin.clone());
            session.original = original.map(|mut message| {
                message.session = task.id.clone();
                message
            });

            session.reference = Some(crate::state::Reference { through });
            session.messages = extras
                .into_iter()
                .map(|mut message| {
                    message.session = task.id.clone();
                    message
                })
                .collect();
            compact_messages(&mut session.messages);
            store.sessions.insert(task.id.clone(), session);
            store.tasks.insert(task.id.clone(), task);
        })?;

        Ok(run)
    }

    pub fn interrupt_foreground(&mut self, session: &str) -> std::io::Result<()> {
        if !self.sessions.contains_key(session) {
            return Err(std::io::Error::other("Conversation not found."));
        }

        self.change(|store| {
            let source = store.sessions.get_mut(session).unwrap();
            source.status = "cancelled".into();

            for task in store.tasks.values_mut().filter(|task| {
                task.foreground
                    && task.origin.session == session
                    && task.status == TaskStatus::Running
            }) {
                task.status = TaskStatus::Cancelling;
                task.restart = false;
                task.revision = task.revision.saturating_add(1);
                task.notice = None;
            }
        })
    }

    pub fn promote_foreground(&mut self, run: &Run, instruction: &str) -> std::io::Result<()> {
        self.check_task_step(&run.id, run.generation)?;

        if instruction.trim().is_empty() || instruction.len() > crate::TOOL_ARGS_LIMIT {
            return Err(std::io::Error::other("A bounded, self-contained goal is required."));
        }

        self.change(|store| {
            let task = store.tasks.get_mut(&run.id).unwrap();
            task.foreground = false;
            task.unsafe_step = false;
            task.instruction = instruction.into();

            task.status = TaskStatus::Queued;
            task.revision = task.revision.saturating_add(1);
            task.next = now();
            let origin = task.origin.session.clone();
            host_receipt(
                store,
                &origin,
                crate::routing::NAME,
                &format!("{}-handoff", run.id),
                "The background goal is saved. Its result will arrive automatically.",
            );
            host_receipt(
                store,
                &run.id,
                crate::routing::NAME,
                &format!("{}-handoff", run.id),
                "The background goal is saved. Its result will arrive automatically.",
            );

            if let Some(session) = store.sessions.get_mut(&run.id) {
                session.kind = SessionKind::Task;
            }
        })
    }

    pub fn foreground_result(&mut self, run: &Run, text: &str) -> std::io::Result<()> {
        self.check_task_step(&run.id, run.generation)?;

        self.checkpoint_task(
            &run.id,
            "The request has been answered.",
            TaskStatus::Reporting,
            now(),
            Some(text),
        )
    }

    pub fn check_foreground_reply(
        &self,
        task_id: Option<&str>,
        session: &str,
        text: &str,
    ) -> std::io::Result<()> {
        let Some(id) = task_id else { return Ok(()) };

        let task = self.task(id)?;

        if task.origin.session != session
            || (task.status == TaskStatus::Reporting && task.result.as_deref() != Some(text))
        {
            return Err(std::io::Error::other("The foreground result does not match its task."));
        }

        Ok(())
    }

    pub(in crate::state) fn finish_foreground_reply(&mut self, id: &str) {
        if let Some(task) = self.tasks.get_mut(id)
            && task.status == TaskStatus::Reporting
        {
            task.status = TaskStatus::Completed;
            task.notice = None;
            task.revision = task.revision.saturating_add(1);
        }
    }

    pub fn client_foreground_reply(
        &mut self,
        task: Option<&str>,
        message: Message,
    ) -> std::io::Result<()> {
        let text = message
            .content
            .iter()
            .map(krabb_core::types::Content::render)
            .collect::<Vec<_>>()
            .join("\n");

        if !self.sessions.contains_key(&message.session) {
            return Err(std::io::Error::other("The conversation no longer exists."));
        }

        self.check_foreground_reply(task, &message.session, &text)?;

        self.change(|store| {
            if let Some(task) = task {
                store.finish_foreground_reply(task);
            }

            if let Some(session) = store.sessions.get_mut(&message.session) {
                session.messages.push(message);
                compact_messages(&mut session.messages);
                session.updated = now();
                session.status = "idle".into();
            }
        })
    }

    pub fn phase_turn(
        &mut self,
        session: &str,
        run: Option<&Run>,
        phase: &str,
    ) -> std::io::Result<()> {
        if let Some(run) = run {
            self.check_task_step(&run.id, run.generation)?;
        }

        self.phase_with_task(session, phase, run.map(|run| run.id.as_str()))
    }
}
