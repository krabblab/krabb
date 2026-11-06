# Telegram

The Telegram plugin receives messages with long polling and sends replies
through the Bot API. Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- telegram
```

- [Setup](#setup)
- [Messages And Media](#messages-and-media)
- [Replies](#replies)

## Setup

Create a bot and obtain its token using the
[Telegram bot tutorial](https://core.telegram.org/bots/tutorial), then run:

```sh
krabb telegram setup
```

Enter the bot token at `KRABB_TELEGRAM_TOKEN`. Telegram uses long polling, so
no public webhook is needed. Send the bot a direct message to test the reply.

See [shared setup](../../../krabb-docs/authentication.md#plugin-setup) for encrypted
storage, daemon activation, environment overrides, and service configuration;
see [channel policy](../../../krabb-docs/configuration.md#channel-policy) for group access.
The legacy `telegram` keyring entry remains supported with `KRABB_KEYRING=1`.

## Messages And Media

The plugin normalizes private and group messages, topics, text, photos,
documents, and voice notes. Media downloads preserve raw bytes in the confined
cache; only textual Bot API responses are decoded as UTF-8.

- Images: PNG, JPEG, GIF, or WebP; up to 4 MiB total per turn. The selected
  model must support image input.
- Documents: bounded UTF-8 text or code files up to 64 KiB each.
- Voice: up to 4 MiB and requires the optional speech plugin. Raw audio is
  removed after transcription.

Media is downloaded into `KRABB_MEDIA`, or `KRABB_HOME/media` by default.
Cached files older than 24 hours are removed; oldest files are also evicted before
downloads are stored to keep the cache within 64 MiB and 256 files. Storage is
rejected if cache cleanup fails. Durable session images are kept
in the session's protected media area.

## Replies

The handshake advertises `formats: ["markdown"]`. Markdown sends and edits render
as plain text plus Telegram message entities, retaining bold, italic, strikethrough,
links, inline code, preformatted code, and quotes. No parse mode is needed. Entity
ranges use UTF-16 units and are clipped and rebased when messages split. Requests
without `format: "markdown"` retain their existing plain-text semantics. Streaming
`draft` requests render only the first chunk; final delivery sends overflow once.
An explicit Telegram “message is not modified” response counts as an unchanged
edit, allowing final overflow delivery to continue without replaying the edit.

Outgoing text is split at Telegram's message limit. An `edit` updates the first
message and sends any remaining chunks in the same topic.
While waiting for the first reply, Krabb sends a best-effort typing indicator
to the same chat and topic. It refreshes every four seconds and stops when
streaming begins or the turn ends. Typing failures do not fail the model turn.

Explicit rate-limit rejections with `retry_after` of at most five seconds are
retried at most twice, for the current API operation only. Longer delays and
ambiguous transport failures are reported to the host without automatic replay.
Malformed success responses fail instead of acknowledging delivery. Error
messages include the status/code and retry delay, not arbitrary upstream text.
See [deployment validation](../../../krabb-docs/providers.md#deployment-validation).
