use super::*;
use futures_util::Stream;

pub(super) async fn parse<S, C, E>(
    mut stream: S,
    model: &str,
    provider: &str,
    emitter: &mut Emitter,
) -> Result<ModelReply>
where
    S: Stream<Item = std::result::Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut buffer = Vec::new();
    let mut bytes = 0_usize;
    let mut pending = String::new();
    let mut emitted = 0_usize;
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(80));
    ticker.tick().await;

    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };

                let chunk = chunk.map_err(|_| {
                    Error::Denied("OpenAI response transport failed.".into())
                })?;
                bytes = bytes.saturating_add(chunk.as_ref().len());

                if bytes > LIMIT {
                    return Err(Error::Limit("OpenAI stream exceeded the frame budget.".into()));
                }

                buffer.extend_from_slice(chunk.as_ref());

                while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line = buffer.drain(..=end).collect::<Vec<_>>();
                    let line = std::str::from_utf8(&line).map_err(|_| {
                        Error::Protocol("OpenAI stream was not UTF-8.".into())
                    })?;
                    let Some(data) = line.trim().strip_prefix("data:") else { continue };

                    let value: Value = serde_json::from_str(data.trim())?;

                    match value["type"].as_str().unwrap_or_default() {
                        "response.output_text.delta" | "response.refusal.delta" => {
                            if let Some(text) = value["delta"].as_str() {
                                pending.push_str(text);
                            }
                        },

                        "response.completed" | "response.incomplete" => {
                            let result = reply(&value["response"], model, provider)?;
                            emit(&mut pending, &mut emitted, emitter).await?;
                            return Ok(result);
                        },

                        "error" | "response.failed" => {
                            return Err(Error::Denied(
                                "OpenAI stream returned an error event.".into(),
                            ));
                        },

                        _ => {},
                    }

                    if pending.len() >= 128 {
                        emit(&mut pending, &mut emitted, emitter).await?;
                    }
                }
            },

            _ = ticker.tick(), if !pending.is_empty() => {
                emit(&mut pending, &mut emitted, emitter).await?;
            },
        }
    }

    Err(Error::Denied("OpenAI stream ended before its completion marker.".into()))
}

async fn emit(pending: &mut String, emitted: &mut usize, emitter: &mut Emitter) -> Result<()> {
    if !pending.is_empty() && *emitted < 256 {
        emitter.event(json!({"kind":"text", "text":std::mem::take(pending)})).await?;
        *emitted += 1;
    }

    pending.clear();
    Ok(())
}
