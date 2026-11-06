use std::{
    collections::BTreeMap,
    io,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ring::hmac;
use serde::Serialize;
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::sync::{oneshot, watch};

const LIMIT: usize = 32;
const LIFE: Duration = Duration::from_secs(300);
const ID_BYTES: usize = 12;
const MAC_BYTES: usize = 16;
const TARGET_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Target {
    pub channel: String,
    pub chat: String,
    pub thread: Option<String>,
    pub session: String,
    pub tool: String,
    pub args: Value,
}

pub(crate) struct Challenge {
    pub approve: String,
    pub deny: String,
    pub answer: oneshot::Receiver<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct View {
    pub id: String,
    pub target: Target,
    pub expires: u64,
}

struct Pending {
    target: Target,
    workspace: Option<String>,
    expires: u64,
    answer: oneshot::Sender<bool>,
    completion: watch::Sender<()>,
}

pub(crate) struct Gate {
    key: [u8; 32],
    pending: BTreeMap<String, Pending>,
    decisions: Vec<(Target, Option<String>, bool, u64)>,
}

impl Gate {
    pub fn new() -> io::Result<Self> {
        let mut key = [0_u8; 32];
        getrandom::fill(&mut key).map_err(io::Error::other)?;
        Ok(Self { key, pending: BTreeMap::new(), decisions: Vec::new() })
    }

    #[cfg(test)]
    pub fn issue(&mut self, target: Target) -> io::Result<Challenge> {
        self.issue_in(target, None)
    }

    pub fn issue_in(&mut self, target: Target, workspace: Option<String>) -> io::Result<Challenge> {
        let now = now()?;
        let target_size = serde_json::to_vec(&target).map_err(io::Error::other)?.len();

        if target_size > TARGET_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Approval request exceeds the size limit.",
            ));
        }

        self.pending.retain(|_, pending| pending.expires > now);

        if self.pending.len() >= LIMIT {
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "Approval capacity is full."));
        }

        let mut nonce = [0_u8; ID_BYTES];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let id = hex(&nonce);
        let expires = now.saturating_add(LIFE.as_secs());
        let (answer, receiver) = oneshot::channel();
        let (completion, _) = watch::channel(());
        let pending = Pending { target, workspace, expires, answer, completion };
        let approve = token(&self.key, &id, &pending, b'a')?;
        let deny = token(&self.key, &id, &pending, b'd')?;
        self.pending.insert(id, pending);
        Ok(Challenge { approve, deny, answer: receiver })
    }

    pub fn resolve(
        &mut self,
        token: &str,
        channel: &str,
        chat: &str,
        thread: Option<&str>,
        authorized: bool,
    ) -> Option<bool> {
        self.resolve_at(token, channel, chat, thread, authorized, now().ok()?)
    }

    fn resolve_at(
        &mut self,
        token: &str,
        channel: &str,
        chat: &str,
        thread: Option<&str>,
        authorized: bool,
        now: u64,
    ) -> Option<bool> {
        if !authorized {
            return None;
        }

        let (id, action, provided_signature) = decode(token)?;
        let pending = self.pending.get(id)?;

        if pending.expires <= now
            || pending.target.channel != channel
            || pending.target.chat != chat
            || (pending.target.channel == "telegram" && pending.target.thread.as_deref() != thread)
        {
            return None;
        }

        let expected = signature(&self.key, id, pending, action).ok()?;

        if !bool::from(expected.ct_eq(&provided_signature)) {
            return None;
        }

        self.complete(id, action == b'a', now)
    }

    fn complete(&mut self, id: &str, approved: bool, now: u64) -> Option<bool> {
        let pending = self.pending.remove(id)?;
        pending.answer.send(approved).ok()?;
        self.decisions.retain(|(_, _, _, expires)| *expires > now);

        if self.decisions.len() >= LIMIT {
            self.decisions.remove(0);
        }

        self.decisions.push((
            pending.target,
            pending.workspace,
            approved,
            now.saturating_add(LIFE.as_secs()),
        ));
        Some(approved)
    }

    pub fn decision(
        &self,
        target: &Target,
        workspace: &Option<String>,
        allowed: impl Fn(&Target) -> bool,
    ) -> Option<bool> {
        let now = now().ok()?;
        self.decisions
            .iter()
            .rev()
            .find(|(previous, root, _, expires)| {
                *expires > now
                    && root == workspace
                    && previous.tool == target.tool
                    && previous.args == target.args
                    && allowed(previous)
            })
            .map(|(_, _, approved, _)| *approved)
    }

    pub fn cancel(&mut self, token: &str) {
        if let Some((id, _, _)) = decode(token) {
            self.pending.remove(id);
        }
    }

    pub fn subscribe(&self, token: &str) -> Option<watch::Receiver<()>> {
        let (id, _, _) = decode(token)?;

        Some(self.pending.get(id)?.completion.subscribe())
    }

    #[cfg(test)]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn list(&mut self) -> io::Result<Vec<View>> {
        let now = now()?;
        self.pending.retain(|_, pending| pending.expires > now);
        Ok(self
            .pending
            .iter()
            .map(|(id, pending)| View {
                id: id.clone(),
                target: pending.target.clone(),
                expires: pending.expires,
            })
            .collect())
    }

    pub fn resolve_local(&mut self, id: &str, approved: bool) -> Option<bool> {
        if id.len() != ID_BYTES * 2 || id.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
            return None;
        }

        let now = now().ok()?;

        if self.pending.get(id)?.expires <= now {
            self.pending.remove(id);
            return None;
        }

        self.complete(id, approved, now)
    }
}

fn token(key: &[u8; 32], id: &str, pending: &Pending, action: u8) -> io::Result<String> {
    let signature = signature(key, id, pending, action)?;
    Ok(format!("{id}.{}.{}", hex(&signature), action as char))
}

fn signature(
    key: &[u8; 32],
    id: &str,
    pending: &Pending,
    action: u8,
) -> io::Result<[u8; MAC_BYTES]> {
    let mut value = Vec::with_capacity(128);
    value.extend_from_slice(b"krabb-approval-v1\0");
    value.extend_from_slice(id.as_bytes());
    value.push(action);
    value.extend_from_slice(&pending.expires.to_be_bytes());
    value.extend_from_slice(&serde_json::to_vec(&pending.target).map_err(io::Error::other)?);
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let signed = hmac::sign(&key, &value);
    let mut output = [0_u8; MAC_BYTES];
    output.copy_from_slice(&signed.as_ref()[..MAC_BYTES]);
    Ok(output)
}

fn decode(value: &str) -> Option<(&str, u8, [u8; MAC_BYTES])> {
    if value.len() != ID_BYTES * 2 + 1 + MAC_BYTES * 2 + 2 || !value.is_ascii() {
        return None;
    }

    let id = value.get(..ID_BYTES * 2)?;
    let middle = value.get(ID_BYTES * 2..)?.strip_prefix('.')?;
    let (signature, action) = middle.split_once('.')?;

    if id.bytes().any(|byte| !byte.is_ascii_hexdigit())
        || signature.len() != MAC_BYTES * 2
        || signature.bytes().any(|byte| !byte.is_ascii_hexdigit())
    {
        return None;
    }

    let action = match action {
        "a" => b'a',
        "d" => b'd',
        _ => return None,
    };

    let mut bytes = [0_u8; MAC_BYTES];

    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&signature[index * 2..index * 2 + 2], 16).ok()?;
    }

    Some((id, action, bytes))
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        value.push_str(&format!("{byte:02x}"));
    }

    value
}

fn now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::{Gate, ID_BYTES, Target, decode};
    use serde_json::json;

    fn target() -> Target {
        Target {
            channel: "telegram".into(),
            chat: "8".into(),
            thread: Some("4".into()),
            session: "telegram-8-thread-4".into(),
            tool: "write".into(),
            args: json!({"path": "note.txt", "text": "hello"}),
        }
    }

    #[tokio::test]
    async fn approves_a_bound_request_once() {
        let mut gate = Gate::new().unwrap();
        let challenge = gate.issue(target()).unwrap();

        assert!(challenge.approve.len() <= 64);
        assert_eq!(gate.resolve(&challenge.approve, "telegram", "8", Some("4"), true), Some(true));
        assert!(challenge.answer.await.unwrap());
        assert_eq!(gate.resolve(&challenge.approve, "telegram", "8", Some("4"), true), None);
    }

    #[tokio::test]
    async fn denies_a_bound_request_once() {
        let mut gate = Gate::new().unwrap();
        let challenge = gate.issue(target()).unwrap();

        assert_eq!(gate.resolve(&challenge.deny, "telegram", "8", Some("4"), true), Some(false));
        assert!(!challenge.answer.await.unwrap());
    }

    #[tokio::test]
    async fn lists_and_resolves_pending_requests_locally() {
        let mut gate = Gate::new().unwrap();
        let challenge = gate.issue(target()).unwrap();
        let pending = gate.list().unwrap();
        let id = pending[0].id.clone();

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].target.tool, "write");
        assert_eq!(gate.resolve_local(&id, true), Some(true));
        assert!(challenge.answer.await.unwrap());
        assert!(gate.list().unwrap().is_empty());
        assert_eq!(gate.resolve_local(&id, false), None);
    }

    #[tokio::test]
    async fn resolves_local_denials() {
        let mut gate = Gate::new().unwrap();
        let challenge = gate.issue(target()).unwrap();
        let id = gate.list().unwrap()[0].id.clone();

        assert_eq!(gate.resolve_local(&id, false), Some(false));
        assert!(!challenge.answer.await.unwrap());
    }

    #[tokio::test]
    async fn local_decisions_can_be_reused_only_after_delivery() {
        let mut gate = Gate::new().unwrap();
        let mut source = target();
        source.channel = "tui".into();
        let workspace = Some("/workspace".into());
        let mut destination = source.clone();
        destination.channel = "telegram".into();
        destination.session = "another-conversation".into();

        for approved in [true, false] {
            let challenge = gate.issue_in(source.clone(), workspace.clone()).unwrap();
            let id = gate.list().unwrap()[0].id.clone();

            assert_eq!(gate.resolve_local(&id, approved), Some(approved));
            assert_eq!(challenge.answer.await.unwrap(), approved);
            assert_eq!(
                gate.decision(&destination, &workspace, |previous| previous.channel == "tui"),
                Some(approved)
            );

            assert_eq!(gate.decision(&destination, &workspace, |_| false), None);
            assert_eq!(gate.decision(&destination, &Some("/other".into()), |_| true), None);
        }

        let mut cancelled = source;
        cancelled.args = serde_json::json!({"path": "cancelled.txt"});
        let challenge = gate.issue_in(cancelled.clone(), workspace.clone()).unwrap();
        let id = gate.list().unwrap()[0].id.clone();
        drop(challenge.answer);

        assert_eq!(gate.resolve_local(&id, true), None);
        assert_eq!(gate.decision(&cancelled, &workspace, |_| true), None);
    }

    #[test]
    fn rejects_invalid_local_approval_ids() {
        let mut gate = Gate::new().unwrap();

        assert_eq!(gate.resolve_local("invalid", true), None);
        assert!(gate.list().unwrap().is_empty());
    }

    #[test]
    fn rejects_untrusted_mismatched_and_tampered_callbacks() {
        let mut gate = Gate::new().unwrap();
        let challenge = gate.issue(target()).unwrap();

        assert_eq!(gate.resolve(&challenge.approve, "telegram", "8", Some("4"), false), None);
        assert_eq!(gate.resolve(&challenge.approve, "telegram", "9", Some("4"), true), None);
        assert_eq!(gate.resolve(&challenge.approve, "telegram", "8", None, true), None);
        let mut changed = challenge.approve.clone().into_bytes();
        let last = changed.len() - 1;
        changed[last] = b'd';
        let changed = String::from_utf8(changed).unwrap();

        assert_eq!(gate.resolve(&changed, "telegram", "8", Some("4"), true), None);
        assert_eq!(gate.resolve(&challenge.approve, "discord", "8", Some("4"), true), None);
    }

    #[test]
    fn bounds_tokens() {
        let mut gate = Gate::new().unwrap();
        let challenge = gate.issue(target()).unwrap();
        let (id, action, signature) = decode(&challenge.approve).unwrap();

        assert_eq!(id.len(), ID_BYTES * 2);
        assert_eq!(action, b'a');
        assert_eq!(signature.len(), 16);
        assert!(decode("invalid").is_none());
    }

    #[test]
    fn enforces_signed_token_expiry() {
        for approved in [true, false] {
            for (offset, accepted) in [(-1, true), (0, false), (1, false)] {
                let mut gate = Gate::new().unwrap();
                let mut challenge = gate.issue(target()).unwrap();
                let token = if approved { &challenge.approve } else { &challenge.deny };

                let (id, _, _) = decode(token).unwrap();
                let expires = gate.pending[id].expires;
                let now = expires.checked_add_signed(offset).unwrap();
                let expected = accepted.then_some(approved);

                assert_eq!(
                    gate.resolve_at(token, "telegram", "8", Some("4"), true, now),
                    expected,
                    "approved={approved}, offset={offset}"
                );

                if accepted {
                    assert_eq!(challenge.answer.try_recv(), Ok(approved));
                    assert!(!gate.has_pending());
                } else {
                    assert_eq!(
                        challenge.answer.try_recv(),
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                    );

                    assert!(gate.has_pending());
                }
            }
        }
    }

    #[test]
    fn removes_canceled_requests() {
        let mut gate = Gate::new().unwrap();
        let challenge = gate.issue(target()).unwrap();
        gate.cancel(&challenge.approve);

        assert!(gate.pending.is_empty());
    }

    #[tokio::test]
    async fn completion_is_scoped_to_resolution_cancellation_and_expiry() {
        let mut gate = Gate::new().unwrap();
        let first = gate.issue(target()).unwrap();
        let second = gate.issue(target()).unwrap();
        let mut first_done = gate.subscribe(&first.approve).unwrap();
        let mut second_done = gate.subscribe(&second.deny).unwrap();

        assert!(gate.subscribe("invalid").is_none());
        assert_eq!(gate.resolve(&first.approve, "telegram", "8", Some("4"), true), Some(true));
        assert!(first_done.changed().await.is_err());
        assert!(first.answer.await.unwrap());
        assert!(gate.subscribe(&first.approve).is_none());
        assert!(!second_done.has_changed().unwrap());

        gate.cancel(&second.approve);

        assert!(second_done.changed().await.is_err());
        assert!(second.answer.await.is_err());

        let expired = gate.issue(target()).unwrap();
        let mut expiry = gate.subscribe(&expired.approve).unwrap();
        let (id, _, _) = decode(&expired.approve).unwrap();
        gate.pending.get_mut(id).unwrap().expires = 0;

        assert!(gate.list().unwrap().is_empty());
        assert!(expiry.changed().await.is_err());
        assert!(expired.answer.await.is_err());
    }

    #[test]
    fn bounds_pending_requests_and_arguments() {
        let mut gate = Gate::new().unwrap();
        let mut challenges = Vec::new();

        for _ in 0..super::LIMIT {
            challenges.push(gate.issue(target()).unwrap());
        }

        let full = gate.issue(target()).err().unwrap();

        assert_eq!(full.kind(), std::io::ErrorKind::WouldBlock);
        assert!(challenges.iter().all(|challenge| challenge.approve.len() <= 64));

        let mut oversized = target();
        oversized.args = serde_json::json!({"text": "x".repeat(super::TARGET_LIMIT)});
        let large = gate.issue(oversized).err().unwrap();

        assert_eq!(large.kind(), std::io::ErrorKind::InvalidData);

        for challenge in challenges {
            gate.cancel(&challenge.approve);
        }

        assert!(gate.pending.is_empty());
    }

    #[tokio::test]
    async fn shared_decisions_require_exact_operation_workspace_source_and_lifetime() {
        let mut gate = Gate::new().unwrap();
        let operation = target();
        let workspace = Some("/workspace".into());
        let challenge = gate.issue_in(operation.clone(), workspace.clone()).unwrap();

        assert_eq!(
            gate.resolve(
                &challenge.approve,
                &operation.channel,
                &operation.chat,
                operation.thread.as_deref(),
                true
            ),
            Some(true)
        );

        assert!(challenge.answer.await.unwrap());
        assert_eq!(gate.decision(&operation, &workspace, |_| true), Some(true));
        assert_eq!(gate.decision(&operation, &workspace, |_| false), None);
        assert_eq!(gate.decision(&operation, &Some("/another".into()), |_| true), None);
        let mut changed = operation.clone();
        changed.args = serde_json::json!({"path":"other"});

        assert_eq!(gate.decision(&changed, &workspace, |_| true), None);
        gate.decisions[0].3 = 0;

        assert_eq!(gate.decision(&operation, &workspace, |_| true), None);
        let challenge = gate.issue_in(operation.clone(), workspace.clone()).unwrap();

        assert_eq!(
            gate.resolve(
                &challenge.deny,
                &operation.channel,
                &operation.chat,
                operation.thread.as_deref(),
                true
            ),
            Some(false)
        );

        assert!(!challenge.answer.await.unwrap());
        assert_eq!(gate.decision(&operation, &workspace, |_| true), Some(false));
    }
}
