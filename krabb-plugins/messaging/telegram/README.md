# Telegram

The Telegram plugin receives messages with long polling and sends replies
through the Bot API. Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- telegram
```

## Table Of Contents

- [Credentials](#credentials)
- [Messages And Media](#messages-and-media)
- [Replies](#replies)

## Credentials

Create a bot with BotFather, then run `krabb telegram setup` (equivalently,
`krabb plugin setup telegram`). The host prompts without echoing the token and
saves it encrypted in `credentials.json` under Krabb's home, or the file
selected by `KRABB_CREDENTIALS`. Setup works before the channel is active and
attempts to load or reload it in a compatible running daemon. Use
`--no-activate` to save without activation. It does not validate the token with
Telegram; normal polling begins when the channel is activated.

The random encryption key goes into the OS keyring when available; otherwise
Krabb uses a protected `credentials.key` file. `KRABB_CREDENTIALS_KEY` selects a
custom key file. Alternatively, `KRABB_TELEGRAM_TOKEN` overrides stored values.
See [credential storage](../../../krabb-docs/authentication.md#provider-credentials)
for backend selection and backups. The plugin's legacy `telegram` keyring entry
remains supported with `KRABB_KEYRING=1`.

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
