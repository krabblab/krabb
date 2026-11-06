Install the plugin:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- slack
```

## Setup

Follow [Slack's Socket Mode guide](https://docs.slack.dev/apis/events-api/using-socket-mode/)
to configure the app, event subscriptions, permissions, and tokens, then run:

```sh
krabb slack setup
```

Enter the bot token at `KRABB_SLACK_BOT_TOKEN` and the app token at
`KRABB_SLACK_APP_TOKEN`. Send the bot a direct message to test the reply.
Conversations in shared channels also need Krabb's
[channel allowlist](../../../krabb-docs/configuration.md#channel-policy).

See [shared setup](../../../krabb-docs/authentication.md#plugin-setup) for encrypted
storage, daemon activation, environment overrides, and service configuration.
For optional history polling, set `KRABB_SLACK_CHANNELS` to comma-separated
conversation IDs in the daemon's environment; this is separate from the allowlist.

## Messages And Media

The plugin receives Socket Mode events and falls back to Web API history polling
for configured conversations. Text, images, voice, and text files are normalized
through the channel boundary, and bounded downloads are stored under `KRABB_MEDIA`.
Inbound events and per-channel history cursors are stored under `KRABB_HOME` and
are removed only after the host acknowledges them.

Media storage reserves space before writing: 64 MiB, 256 files, and a 24-hour
cache lifetime. Failed cleanup rejects the download instead of silently
exceeding the limits; protected session images are preserved.

## Formatting

The handshake advertises `formats: ["markdown"]`. Requests with
`format: "markdown"` are rendered as structured Slack rich-text blocks, retaining emphasis, links, code and quotes without interpreting literal text as mentions.
Requests without this field keep their existing text semantics. Unsupported
Markdown structures remain readable text. The runtime preserves source Markdown
for scheduled deliveries and retries.
