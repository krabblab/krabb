mod continuation;
mod frame;
mod summary;
pub(crate) use continuation::Key;

use std::collections::VecDeque;
use summary::summarize;

use krabb_core::{
    Content, Error, Message, Result, Role,
    context::{self, Generation, Profile},
    plugin::Process,
    types::{Capability, ModelRequest, Request},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::{Duration, Instant};

const CACHE: usize = 16;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Record {
    model: String,
    workspace: Option<String>,
    session: String,
    through: String,
    fingerprint: Vec<u8>,
    pub summary: Message,
}

#[derive(Default)]
pub(crate) struct Context {
    profiles: VecDeque<(String, Instant, Profile)>,
    records: VecDeque<Record>,
    continuations: continuation::Cache,
}

impl Context {
    pub fn capture(&mut self, key: Option<Key>, response: &krabb_core::types::Response) {
        if let Some(key) = key {
            self.continuations.capture(key, response);
        }
    }

    pub async fn summarize(
        &mut self,
        process: &mut Process,
        input: &ModelRequest,
        id: u64,
        deadline: Instant,
    ) -> Result<String> {
        let profile = if process.hello.capabilities.contains(&Capability::Context) {
            self.profile(process, &input.model, id, deadline).await?
        } else {
            Profile {
                window: Some(context::Window { tokens: 16384, output: 2048 }),
                ..Profile::default()
            }
        };

        let window = profile.window.ok_or_else(|| {
            Error::Protocol("The provider did not supply a compaction budget.".into())
        })?;
        let target = (window.input()? / 4).clamp(1, 512).min(window.output);

        summarize(process, &profile, input, &input.messages, target, id, deadline).await
    }

    pub fn restore(&mut self, record: Record) {
        self.records.retain(|old| {
            old.model != record.model
                || old.workspace != record.workspace
                || old.session != record.session
        });

        self.records.push_back(record);

        if self.records.len() > CACHE {
            self.records.pop_front();
        }
    }

    pub fn record(&self, model: &str, workspace: Option<&str>, session: &str) -> Option<Record> {
        self.records
            .iter()
            .rev()
            .find(|record| {
                record.model == model
                    && record.workspace.as_deref() == workspace
                    && record.session == session
            })
            .cloned()
    }

    pub async fn prepare(
        &mut self,
        process: &mut Process,
        request: &mut Request,
        deadline: Instant,
    ) -> Result<()> {
        let Request::Call { id, method, params, .. } = request else {
            return Ok(());
        };

        if method != "generate" || !process.hello.capabilities.contains(&Capability::Context) {
            return Ok(());
        }

        let mut input: ModelRequest = serde_json::from_value(params.take())?;
        self.continuations.restore(&mut input);
        let profile = self.profile(process, &input.model, *id, deadline).await?;
        let Some(window) = profile.window else {
            *params = serde_json::to_value(input)?;
            return Ok(());
        };

        let budget = window.input()?;
        let session =
            input.messages.last().map(|message| message.session.clone()).unwrap_or_default();
        let record = self.record(&input.model, input.workspace.as_deref(), &session);

        let crowded = input.messages.len() >= krabb_core::session::MESSAGE_HISTORY_LIMIT - 10;

        if record.is_some()
            || crowded
            || context::estimate(&input.messages, &input.tools) > budget
            || !frame::fits(&input)
        {
            let source = input.messages.clone();

            if let Some(record) = record {
                record.apply(&mut input.messages)?;
            }

            if crowded
                || context::estimate(&input.messages, &input.tools) > budget
                || !frame::fits(&input)
            {
                self.compact(process, &profile, &mut input, &source, *id, deadline).await?;
            }
        }

        // Truncation is a last resort after the history has been summarized.
        while context::estimate(&input.messages, &input.tools) > budget || !frame::fits(&input) {
            if !truncate(&mut input.messages) {
                return Err(Error::Limit(
                    "System instructions, tools, and the current turn exceed the model's input \
                     budget; shorten the input, disable unused tools, or increase its context."
                        .into(),
                ));
            }
        }

        input.generation = Some(Generation {
            output: input.generation.map_or(window.output, |value| value.output.min(window.output)),
            summary: input.generation.is_some_and(|value| value.summary),
            search: profile.search && !input.generation.is_some_and(|value| value.summary),
        });

        *params = serde_json::to_value(input)?;
        Ok(())
    }

    async fn profile(
        &mut self,
        process: &mut Process,
        model: &str,
        id: u64,
        deadline: Instant,
    ) -> Result<Profile> {
        if let Some((_, _, profile)) = self
            .profiles
            .iter()
            .find(|(name, time, _)| name == model && time.elapsed() < Duration::from_secs(300))
        {
            return Ok(profile.clone());
        }

        let response = process
            .call_stream_timeout_async(
                Request::call(id, "model.info", serde_json::json!({"model":model})),
                deadline.saturating_duration_since(Instant::now()),
                |_| async { Ok(()) },
            )
            .await?;
        let profile: Profile = serde_json::from_value(result(response)?)?;

        if let Some(window) = profile.window {
            window.input()?;
        }

        self.profiles.retain(|(name, _, _)| name != model);
        self.profiles.push_back((model.into(), Instant::now(), profile.clone()));

        if self.profiles.len() > CACHE {
            self.profiles.pop_front();
        }

        Ok(profile)
    }

    async fn compact(
        &mut self,
        process: &mut Process,
        profile: &Profile,
        input: &mut ModelRequest,
        source: &[Message],
        id: u64,
        deadline: Instant,
    ) -> Result<()> {
        let Some(boundary) = input.messages.iter().rposition(|message| message.role == Role::User)
        else {
            return Ok(());
        };

        let older = input.messages[..boundary]
            .iter()
            .filter(|message| message.role != Role::System)
            .cloned()
            .collect::<Vec<_>>();
        let window = profile.window.unwrap();
        let target = (window.input()? / 4).clamp(1, 512).min(window.output);
        let end = summary::prefix(&older, window.input()?, target);
        let older = &older[..end];
        let Some(last) = older.last() else {
            return Ok(());
        };

        let through = last.id.clone();
        let Some(index) = source.iter().rposition(|message| message.id == through) else {
            if older.iter().all(|message| message.sender.as_deref() == Some("compaction")) {
                return Ok(());
            }

            return Err(Error::Protocol("Compaction history boundary is unavailable.".into()));
        };

        let fingerprint = fingerprint(&source[..=index])?;
        let summary = summarize(process, profile, input, older, target, id, deadline).await?;
        let session = last.session.clone();
        let summary = Message {
            id: format!("compact-{through}"),
            session: session.clone(),
            role: Role::Assistant,
            sender: Some("compaction".into()),
            content: vec![Content::Text {
                text: format!("Earlier conversation summary:\n{summary}"),
            }],
        };

        let record = Record {
            model: input.model.clone(),
            workspace: input.workspace.clone(),
            session,
            through: through.clone(),
            fingerprint,
            summary: summary.clone(),
        };

        let boundary = input.messages.iter().rposition(|item| item.id == through).unwrap() + 1;
        replace(&mut input.messages, boundary, summary);
        self.restore(record);
        Ok(())
    }
}

impl Record {
    pub fn apply(&self, messages: &mut Vec<Message>) -> Result<bool> {
        let Some(index) = messages.iter().position(|message| message.id == self.through) else {
            return Ok(false);
        };

        if fingerprint(&messages[..=index])? != self.fingerprint {
            return Ok(false);
        }

        replace(messages, index + 1, self.summary.clone());
        Ok(true)
    }
}

fn replace(messages: &mut Vec<Message>, boundary: usize, summary: Message) {
    let mut index = 0;

    messages.retain(|message| {
        let keep = index >= boundary || message.role == Role::System;
        index += 1;
        keep
    });

    let index = messages.iter().take_while(|message| message.role == Role::System).count();
    messages.insert(index, summary);
}

fn truncate(messages: &mut Vec<Message>) -> bool {
    let summary = messages.iter().position(|item| item.sender.as_deref() == Some("compaction"));
    let saved = summary.map(|index| (index, messages.remove(index)));
    let removed = context::truncate(messages);

    if let Some((index, message)) = saved {
        messages.insert(index.min(messages.len()), message);
    }

    removed || context::truncate(messages)
}

struct Fingerprint(Sha256);

impl std::io::Write for Fingerprint {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fingerprint(messages: &[Message]) -> Result<Vec<u8>> {
    let mut digest = Fingerprint(Sha256::new());

    for message in messages.iter().filter(|message| message.role != Role::System) {
        serde_json::to_writer(
            &mut digest,
            &(&message.id, &message.session, &message.role, &message.sender),
        )?;

        for content in &message.content {
            match content {
                // Summaries use image descriptions, not temporary paths or encoded pixels.
                Content::Image { alt, .. } => serde_json::to_writer(&mut digest, &("image", alt))?,

                Content::Context { .. } => {}
                content => serde_json::to_writer(&mut digest, content)?,
            }
        }
    }

    Ok(digest.0.finalize().to_vec())
}

fn result(response: krabb_core::types::Response) -> Result<serde_json::Value> {
    response.result.ok_or_else(|| {
        response.error.map_or_else(
            || Error::Protocol("The model returned no context response.".into()),
            |error| Error::Denied(error.message),
        )
    })
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, unix))]
use summary::{BATCHES, batches, text};
