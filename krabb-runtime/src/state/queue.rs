use super::*;

impl Store {
    /// Removes a queued message before its turn starts, and returns whether it was waiting.
    pub fn unqueue(&mut self, id: &str, request: &str) -> std::io::Result<bool> {
        let Some(session) = self.sessions.get(id) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Session was not found.",
            ));
        };

        if !session.queued.iter().any(|message| message.id == request) {
            return Ok(false);
        }

        self.change(|store| {
            let Some(session) = store.sessions.get_mut(id) else {
                return;
            };

            session.queued.retain(|message| message.id != request);
            session.queue_roles.remove(request);
            session.updated = now();
        })?;

        Ok(true)
    }

    /// Reserves an idle session for OWNER and moves the queue's first message from SENDER into
    /// the conversation in one saved change, so a crash leaves it either waiting or started.
    pub fn reserve_next(
        &mut self,
        id: &str,
        owner: String,
        sender: &str,
    ) -> std::io::Result<Option<Message>> {
        let Some(session) = self.sessions.get(id) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Session was not found.",
            ));
        };

        if session.is_work() {
            return Err(std::io::Error::other(
                "Internal work sessions cannot reserve interactive turns.",
            ));
        }

        let idle = !session.reservation_active
            && session.inflight.is_none()
            && session.status != "working";

        let Some(next) = session.queued.first().filter(|message| {
            idle && message.role == Role::User && message.sender.as_deref() == Some(sender)
        }) else {
            return Ok(None);
        };

        let message = next.clone();

        self.change(|store| {
            let Some(session) = store.sessions.get_mut(id) else {
                return;
            };

            session.queued.remove(0);
            session.queue_roles.remove(&message.id);

            if !session.messages.iter().any(|item| item.id == message.id) {
                session.messages.push(message.clone());

                compact_messages(&mut session.messages);
            }

            session.status = "working".into();
            session.reservation_until = Some(now().saturating_add(TUI_RESERVATION_TTL));
            session.reservation_owner = Some(owner);
            session.updated = now();
        })?;

        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (PathBuf, Store) {
        let root = std::env::temp_dir().join(format!(
            "krabb-state-queue-{name}-{}-{}",
            std::process::id(),
            now()
        ));

        let _ = std::fs::remove_dir_all(&root);
        let mut store = Store::load(root.join("sessions.json")).unwrap();

        store.create("main", "fixture").unwrap();
        (root, store)
    }

    fn typed(id: &str, sender: &str) -> Message {
        Message {
            id: id.into(),
            session: "main".into(),
            role: Role::User,
            sender: Some(sender.into()),
            content: vec![Content::Text { text: format!("text {id}") }],
        }
    }

    #[test]
    fn starts_queued_messages_in_order_and_only_when_idle() {
        let (root, mut store) = store("order");

        for id in ["1", "2", "3"] {
            store.queue_with_roles("main", typed(id, "tui"), Vec::new()).unwrap();
        }

        let error = store.reserve("main", "typed".into()).unwrap_err();

        assert_eq!(error.to_string(), krabb_core::types::SESSION_BUSY);

        let first = store.reserve_next("main", "a".into(), "tui").unwrap().unwrap();

        assert_eq!(first.id, "1");
        assert!(store.reserve_next("main", "b".into(), "tui").unwrap().is_none());

        store.set_status("main", "idle").unwrap();

        let second = store.reserve_next("main", "b".into(), "tui").unwrap().unwrap();
        let recovered = Store::load(root.join("sessions.json")).unwrap();
        let session = &recovered.sessions["main"];

        assert_eq!(second.id, "2");
        assert_eq!(session.queued.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), ["3"]);

        assert_eq!(
            session.messages.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["1", "2"]
        );

        assert_eq!(session.reservation_owner.as_deref(), Some("b"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancels_only_messages_that_have_not_started() {
        let (root, mut store) = store("cancel");

        store.queue_with_roles("main", typed("1", "tui"), Vec::new()).unwrap();
        store.queue_with_roles("main", typed("2", "tui"), Vec::new()).unwrap();
        store.reserve_next("main", "a".into(), "tui").unwrap();

        assert!(!store.unqueue("main", "1").unwrap());
        assert!(store.unqueue("main", "2").unwrap());
        assert!(!store.unqueue("main", "2").unwrap());
        assert!(store.unqueue("missing", "2").is_err());

        assert!(
            Store::load(root.join("sessions.json")).unwrap().sessions["main"].queued.is_empty()
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn leaves_other_senders_and_full_queues_untouched() {
        let (root, mut store) = store("limit");

        store.queue_with_roles("main", typed("telegram", "7"), Vec::new()).unwrap();

        assert!(store.reserve_next("main", "a".into(), "tui").unwrap().is_none());

        for index in 1..LIMIT {
            store.queue_with_roles("main", typed(&index.to_string(), "tui"), Vec::new()).unwrap();
        }

        let error = store.queue_with_roles("main", typed("over", "tui"), Vec::new()).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(error.to_string(), krabb_core::types::QUEUE_FULL);
        assert_eq!(store.sessions["main"].queued.len(), LIMIT);

        let _ = std::fs::remove_dir_all(root);
    }
}
