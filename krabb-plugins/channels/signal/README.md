Signal uses a managed `signal-cli` process. Install the Krabb plugin:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- signal
```

## Setup

Install and register or link the unofficial Signal client using the
[signal-cli instructions](https://github.com/AsamK/signal-cli#usage), then run:

```sh
krabb signal setup
```

Enter the registered account at `KRABB_SIGNAL_ACCOUNT`. Run registration and
Krabb under the same OS user so `signal-cli` can access its account data.
Make `signal-cli` available on the daemon's `PATH`, or set `KRABB_SIGNAL_COMMAND`
in the daemon's environment to its executable path. Send the account a direct
message from another account to test the reply.

See [shared setup](../../../krabb-docs/authentication.md#plugin-setup) for encrypted
storage, daemon activation, environment overrides, and service configuration;
see [channel policy](../../../krabb-docs/configuration.md#channel-policy) for group access.

## Messages And Media

While a reply is being prepared, the plugin calls `signal-cli sendTyping` for
the individual or group recipient. This requires a `signal-cli` version that
supports that command. Indicator failures do not prevent the reply.

Text, images, voice, and text files are normalized through the channel boundary. Attachments are resolved
through `signal-cli` and cached in `KRABB_MEDIA` (or `KRABB_HOME/data/media`);
polled events are retained in a crash-safe inbox under `KRABB_HOME` until
the host acknowledges them. Cached attachments expire after 24 hours and are
bounded to 64 MiB and 256 files. Oldest files are evicted before storage, and cleanup
failures reject storage; protected session images are preserved. Local attachment files
are confined there before they are sent. Outgoing replies use the standard Krabb delivery contract.

The adapter uses the global `--output json` option for receive, as specified by
the [signal-cli manual](https://github.com/AsamK/signal-cli/blob/master/man/signal-cli.1.adoc).
Direct recipients are separated from options with `--`, including when sending
attachments. Invalid recipients, oversized text, and more than eight content items
are rejected before starting the process. Failed commands report an exit code;
raw stderr is not forwarded into Krabb messages or logs.

## Formatting

The handshake advertises `formats: ["markdown"]`. Requests with
`format: "markdown"` are rendered as signal-cli text styles with UTF-16 ranges for bold, italic, strikethrough and monospace; link targets remain visible.
Requests without this field keep their existing text semantics. Unsupported
Markdown structures remain readable text. The runtime preserves source Markdown
for scheduled deliveries and retries.
