use super::*;
use krabb_core::types::Content;

impl Store {
    pub fn prepare_queued_context(&mut self, id: &str) -> std::io::Result<()> {
        let job = self.job(id)?;

        if !job.foreground || job.step != 1 {
            return Ok(());
        }

        let source = self.session(&job.origin.session)?;

        if self.session(id)?.reference.is_some() {
            let through = source.context().last().map(|message| message.id.clone());

            return self.change(|store| {
                store.sessions.get_mut(id).unwrap().reference =
                    Some(crate::state::Reference { through });
            });
        }

        let Some(input) = source
            .messages
            .iter()
            .find(|message| Some(&message.id) == job.request.as_ref())
            .cloned()
        else {
            return Ok(());
        };

        let pending = self.pending_requests(job);

        let mut history = source
            .context()
            .into_iter()
            .filter(|message| {
                message.role != Role::User
                    || (!pending.contains(&message.id) && Some(&message.id) != job.request.as_ref())
            })
            .collect::<Vec<_>>();

        history.push(input);

        for message in &mut history {
            message.session = id.into();
        }

        self.change(|store| {
            let journal = store.sessions.get_mut(id).unwrap();
            journal.messages = history;
            journal.compacted_context = None;
            journal.compacted_through = None;
            journal.context_usage = None;
            compact_messages(&mut journal.messages);
        })
    }

    fn pending_requests(&self, job: &Job) -> BTreeSet<String> {
        self.jobs
            .values()
            .filter(|other| {
                other.id != job.id
                    && other.foreground
                    && other.origin.session == job.origin.session
                    && !other.status.terminal()
            })
            .filter_map(|other| other.request.clone())
            .collect()
    }

    pub fn foreground_history(&mut self, id: &str) -> std::io::Result<Vec<Message>> {
        let session = self
            .sessions
            .get(id)
            .ok_or_else(|| std::io::Error::other("Task context not found."))?;

        let marker = format!("{id}-generation-{}", self.jobs.get(id).map_or(0, |job| job.step));

        let (base, own) = if session.reference.is_some() && session.compacted_context.is_none() {
            (self.reference(id), session.messages.clone())
        } else {
            (Vec::new(), session.context())
        };

        let (base, _) = settle(id, &format!("{marker}-reference"), base);
        let (own, receipts) = settle(id, &marker, own);

        if receipts != 0 {
            self.change(|store| {
                let session = store.sessions.get_mut(id).unwrap();
                session.messages = own.clone();
                session.compacted_context = None;
                session.compacted_through = None;
                session.context_usage = None;
                compact_messages(&mut session.messages);
            })?;
        }

        Ok(join(base, own))
    }

    pub(crate) fn resolved_context(&self, id: &str) -> std::io::Result<Vec<Message>> {
        let session = self.session(id)?;

        if session.reference.is_none() || session.compacted_context.is_some() {
            return Ok(session.context());
        }

        Ok(join(self.reference(id), session.messages.clone()))
    }

    fn reference(&self, id: &str) -> Vec<Message> {
        let (Some(job), Some(journal)) = (self.jobs.get(id), self.sessions.get(id)) else {
            return Vec::new();
        };

        let (Some(source), Some(reference)) =
            (self.sessions.get(&job.origin.session), journal.reference.as_ref())
        else {
            return Vec::new();
        };

        let mut base = source.context();

        let request = base
            .iter()
            .chain(&journal.original)
            .find(|message| message.role == Role::User && Some(&message.id) == job.request.as_ref())
            .cloned();

        match &reference.through {
            Some(through) => {
                if let Some(end) = base.iter().position(|message| &message.id == through) {
                    base.truncate(end + 1);
                }
            }

            None => base.clear(),
        }

        let own = journal.messages.iter().map(|message| &message.id).collect::<BTreeSet<_>>();
        let pending = self.pending_requests(job);

        base.retain(|message| {
            !own.contains(&message.id)
                && (message.role != Role::User
                    || (!pending.contains(&message.id)
                        && Some(&message.id) != job.request.as_ref()))
        });

        base.extend(request);

        for message in &mut base {
            message.session = id.into();
        }

        base
    }

    pub(in crate::state) fn reference_journals(&mut self) -> bool {
        let mut referenced = Vec::new();

        for (id, session) in &self.sessions {
            let Some(job) = self.jobs.get(id) else { continue };

            if session.kind != SessionKind::Foreground
                || session.reference.is_some()
                || session.compacted_context.is_some()
            {
                continue;
            }

            let Some(request) = job.request.as_ref() else { continue };

            let Some(source) = self.sessions.get(&job.origin.session) else { continue };

            let Some(end) = session
                .messages
                .iter()
                .position(|message| message.role == Role::User && &message.id == request)
            else {
                continue;
            };

            let saved = source.messages.iter().map(|message| &message.id).collect::<BTreeSet<_>>();

            let (system, conversation): (Vec<_>, Vec<_>) =
                session.messages[..=end].iter().partition(|message| message.role == Role::System);

            if source.compacted_context.is_some()
                || conversation.iter().any(|message| !saved.contains(&message.id))
            {
                continue;
            }

            let through = conversation
                .iter()
                .filter(|message| &message.id != request)
                .max_by_key(|message| {
                    source.messages.iter().position(|saved| saved.id == message.id)
                })
                .map(|message| message.id.clone())
                .or_else(|| Some(request.clone()));

            let messages = system
                .into_iter()
                .filter(|message| !message.id.starts_with(super::super::SYSTEM_CONTEXT))
                .chain(&session.messages[end + 1..])
                .cloned()
                .collect::<Vec<_>>();

            referenced.push((id.clone(), messages, through));
        }

        let changed = !referenced.is_empty();

        for (id, messages, through) in referenced {
            let session = self.sessions.get_mut(&id).unwrap();
            session.messages = messages;
            session.reference = Some(crate::state::Reference { through });
        }

        changed
    }
}

fn join(base: Vec<Message>, mut own: Vec<Message>) -> Vec<Message> {
    let rest = own.split_off(own.iter().take_while(|message| message.role == Role::System).count());

    own.into_iter().chain(base).chain(rest).collect()
}

fn settle(id: &str, marker: &str, history: Vec<Message>) -> (Vec<Message>, usize) {
    let mut settled = Vec::new();

    let mut pending = Vec::new();
    let mut receipts = 0;

    for message in history {
        if matches!(message.role, Role::User | Role::Assistant) {
            close(id, marker, &mut settled, &mut pending, &mut receipts);
        }

        if message.role == Role::Assistant {
            for content in &message.content {
                if let Content::ToolCall { name, .. } = content {
                    pending.push(name.clone());
                }
            }
        } else if message.role == Role::Tool
            && let Some(position) =
                pending.iter().position(|name| Some(name.as_str()) == message.sender.as_deref())
        {
            pending.remove(position);
        }

        settled.push(message);
    }

    close(id, marker, &mut settled, &mut pending, &mut receipts);

    (settled, receipts)
}

fn close(
    id: &str,
    marker: &str,
    messages: &mut Vec<Message>,
    pending: &mut Vec<String>,
    count: &mut usize,
) {
    for name in pending.drain(..) {
        *count += 1;
        messages.push(Message {
            id: format!("{marker}-interrupted-tool-{count}"),
            session: id.into(),
            role: Role::Tool,
            sender: Some(name),
            content: vec![Content::Text {
                text: "Interrupted before a durable result was received. Do not assume this \
                    operation succeeded. Only known-safe reads may be retried automatically."
                    .into(),
            }],
        });
    }
}
