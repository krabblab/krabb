# Discord

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- discord
```

## Setup

Create and install a bot using [Discord's getting started guide](https://docs.discord.com/developers/quick-start/getting-started).
Enable the [Message Content intent](https://docs.discord.com/developers/events/gateway#privileged-intents)
for text conversations, then run:

```sh
krabb discord setup
```

Enter the bot token at `KRABB_DISCORD_TOKEN`. Send the bot a direct message to
test the reply. Guild channels and threads also need Krabb's
[channel allowlist](../../../krabb-docs/configuration.md#channel-policy).

See [shared setup](../../../krabb-docs/authentication.md#plugin-setup) for encrypted
storage, daemon activation, environment overrides, and service configuration.
The legacy `discord` keyring entry remains supported with `KRABB_KEYRING=1`.

## Messages And Media

While a reply is being prepared, the plugin uses Discord's native typing
indicator in the destination DM, channel, or thread. Indicator failures do not
prevent the reply.

The plugin receives `MESSAGE_CREATE` events through the Gateway, answers
heartbeats, ignores bot authors, normalizes text and attachments, and sends replies through the REST
API. Gateway session state is persisted under `KRABB_HOME` and advances only
after the host acknowledges an accepted event. Outgoing text is split at
Discord's message limit; `edit` updates the first existing message and sends
any remaining chunks to the same channel or thread. The host sends separate progress
for tool work and slow turns, keeps typing active until completion, and delivers
final answers as fresh messages so completion can trigger a notification. Progress
failures do not block final delivery.
`KRABB_DISCORD_GATEWAY_URL` and `KRABB_DISCORD_INTENTS` are optional
overrides for local integration tests.

Image, voice-message audio, and bounded text-file attachments are downloaded
through opaque references into the private `KRABB_MEDIA` cache. The plugin
expires files after 24 hours and evicts oldest files before storage to keep the
cache within 64 MiB and 256 files. Cleanup failures reject storage; the protected
session image directory is preserved. The plugin
supports attachment uploads, but the current daemon sends generated replies as
text. Direct messages work by default; guild channels and threads require the
configured channel allowlist.
A missing heartbeat acknowledgement closes the Gateway connection so the next
poll reconnects. A nonresumable session rejection clears the old session before
identifying again. Gateway connection establishment has a ten-second deadline.

## Formatting

The handshake advertises `formats: ["markdown"]`. Requests with
`format: "markdown"` are rendered as Discord Markdown, escaping literal punctuation, retaining links and code fences, and disabling generated mentions.
Requests without this field keep their existing text semantics. Unsupported
Markdown structures remain readable text. The runtime preserves source Markdown
for scheduled deliveries and retries.

Explicit HTTP 429 message rejections are retried at most twice using Discord's
`retry_after` delay (or `Retry-After` header), up to five seconds per wait. Only
the rejected operation is retried; delivered chunks and uncertain requests are
not replayed automatically.

Discord replies preserve quoted bot or user text; references outside the current channel are marked unavailable.
Missing reference text is marked unavailable; replies do not change sender permissions.
