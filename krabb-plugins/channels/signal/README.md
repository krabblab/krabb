# Signal

Signal uses a managed `signal-cli` process. Install and register an account,
install the Krabb plugin, then set `KRABB_SIGNAL_ACCOUNT`:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- signal
```

Override the executable with
`KRABB_SIGNAL_COMMAND` when it is not on `PATH`. Text, images, voice, and text
files are normalized through the channel boundary. Attachments are resolved
through `signal-cli` and cached in `KRABB_MEDIA` (or `KRABB_HOME/media`);
polled events are retained in a crash-safe inbox under `KRABB_HOME` until
the host acknowledges them. Cached attachments expire after 24 hours and are
bounded to 64 MiB and 256 files. Oldest files are evicted before storage, and cleanup
failures reject storage; protected session images are preserved. Local attachment files
are confined there before they are
sent. Outgoing replies use the standard Krabb delivery contract.

Use `krabb signal setup` for hidden credential entry and encrypted storage.
Setup saves declared secrets; it does not register accounts, grant external app
permissions, or configure channel authorization. Environment values override
stored credentials. See [authentication](../../../krabb-docs/authentication.md#plugin-setup)
and [deployment validation](../../../krabb-docs/providers.md#deployment-validation).

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
