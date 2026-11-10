use super::*;
use krabb_core::reply::Reply;
use serde_json::Value;

impl Session {
    pub fn resolve_reply(&self, mut reply: Reply) -> Reply {
        if reply.text.is_none()
            && let Some(saved) = self.references.iter().rev().find(|saved| saved.id == reply.id)
        {
            reply.text.clone_from(&saved.text);
            reply.sender = reply.sender.or_else(|| saved.sender.clone());
        }

        if reply.text.is_none() {
            reply.text = Some("[Referenced message unavailable.]".into());
        }

        reply
    }

    fn remember_reply(&mut self, reply: Reply) {
        self.references.retain(|saved| saved.id != reply.id);

        self.references.push(reply);

        let excess = self.references.len().saturating_sub(128);

        self.references.drain(..excess);
    }
}

impl Store {
    pub fn remember_reply(&mut self, session: &str, reply: Reply) -> std::io::Result<()> {
        let Some(reply) = reply.normalized() else { return Ok(()) };

        self.change(|store| {
            if let Some(session) = store.sessions.get_mut(session) {
                session.remember_reply(reply);
            }
        })
    }

    pub fn delivered(&mut self, id: &str, result: &Value) -> std::io::Result<()> {
        let mut references: Vec<Reply> = result["references"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .take(128)
            .filter_map(|value| serde_json::from_value::<Reply>(value.clone()).ok())
            .filter_map(Reply::normalized)
            .collect();

        references.reverse();

        self.change(|store| {
            if let Some(index) = store.outbox.iter().position(|delivery| delivery.id == id) {
                let delivery = store.outbox.remove(index);

                for session in store.sessions.values_mut().filter(|session| {
                    session.channel.as_deref() == Some(&delivery.channel)
                        && session.chat.as_deref() == Some(&delivery.chat)
                        && session.thread == delivery.thread
                }) {
                    for reply in &references {
                        session.remember_reply(reply.clone());
                    }
                }

                store.receipt(id, Receipt::Delivered);
            }
        })
    }
}
