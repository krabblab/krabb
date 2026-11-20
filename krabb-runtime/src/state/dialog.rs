use super::*;

const CONTROL_LIMIT: usize = 64;

/// A retained `/task` command receipt, kept outside the conversation history.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub id: String,
    pub sender: String,
    pub input: String,
    pub reply: String,
}

impl Store {
    pub fn control_reply(
        &self,
        session: &str,
        id: &str,
        sender: &str,
        input: &str,
    ) -> std::io::Result<Option<String>> {
        let session = self.session(session)?;

        let Some(control) = session.commands.iter().find(|control| control.id == id) else {
            return Ok(None);
        };

        if control.sender != sender || control.input != input {
            return Err(std::io::Error::other(
                "This message ID was already used with different content.",
            ));
        }

        Ok(Some(control.reply.clone()))
    }

    pub fn record_control(
        &mut self,
        session: &str,
        id: &str,
        sender: &str,
        input: &str,
        reply: &str,
    ) -> std::io::Result<()> {
        let control = Control {
            id: id.into(),
            sender: sender.into(),
            input: input.into(),
            reply: reply.into(),
        };

        self.commit_control(session, control, |_| {})
    }

    /// Drops history from message `from` and records the command in one save, so a replayed
    /// command finds its receipt instead of rewinding another exchange.
    pub fn rewind(&mut self, session: &str, from: usize, control: Control) -> std::io::Result<()> {
        self.commit_control(session, control, |current| {
            let removed = current.messages.split_off(from.min(current.messages.len()));
            let compacted = current.compacted_through.as_ref();

            // The summary still covers what remains, so only the removed exchange leaves it.
            if removed.iter().any(|message| Some(&message.id) == compacted) {
                let last = current.messages.last().map(|message| message.id.clone());
                let context = current.compacted_context.take().unwrap_or_default();

                current.compacted_context = last.as_ref().map(|_| {
                    context
                        .into_iter()
                        .filter(|kept| removed.iter().all(|message| message.id != kept.id))
                        .collect()
                });

                current.compacted_through = last;
            }

            current.context_usage = None;
        })
    }

    /// Applies `edit` and records `control` in one save, restoring the session if it fails.
    fn commit_control(
        &mut self,
        session: &str,
        control: Control,
        edit: impl FnOnce(&mut Session),
    ) -> std::io::Result<()> {
        let previous = self.session(session)?.clone();

        let current = self.sessions.get_mut(session).unwrap();

        edit(current);

        if current.commands.len() >= CONTROL_LIMIT {
            current.commands.remove(0);
        }

        current.commands.push(control);
        current.updated = now();

        if let Err(error) = self.save_session_only(session) {
            self.sessions.insert(session.into(), previous);
            return Err(error);
        }

        Ok(())
    }

    fn save_session_only(&self, id: &str) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };

        if !session_catalog(path) {
            return self.save();
        }

        let bytes = load_file(path, BYTE_LIMIT)?
            .ok_or_else(|| std::io::Error::other("Conversation catalog is missing."))?;

        let mut index: SessionIndex =
            serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;

        let session = &self.sessions[id];
        let bytes = serde_json::to_vec(session).map_err(std::io::Error::other)?;
        let limit = BYTE_LIMIT;

        #[cfg(test)]
        let limit = self.record_limit.unwrap_or(limit);

        if bytes.len() as u64 > limit {
            return Err(std::io::Error::other("Conversation record is full."));
        }

        let key = session_record_key(&bytes);

        let directory =
            record_directory(path, id, session.isolated.as_ref(), index.journals.contains(id))?;

        save_record(directory.join(format!("{key}.json")), bytes)?;
        index.sessions.insert(id.into(), key.clone());
        save_json(path, &index)?;

        let _ = cleanup_session_records(&directory, &BTreeMap::from([(id.into(), key)]));

        Ok(())
    }
}
