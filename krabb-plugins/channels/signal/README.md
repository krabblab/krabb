Signal uses a managed `signal-cli` process. Install the Krabb plugin:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- signal
```

## Setup

Krabb uses an existing Signal account through the unofficial `signal-cli` client.
Account registration, device linking, and the client's own account data belong to
Signal and `signal-cli`; `krabb signal setup` configures the Krabb integration.

### External Account And Client

Use the upstream guides for these prerequisites:

- [Signal registration](https://support.signal.org/hc/en-us/articles/360007318691-Register-a-phone-number)
  and [linked devices](https://support.signal.org/hc/en-us/articles/360007320551-Linked-Devices).
- [signal-cli installation and usage](https://github.com/AsamK/signal-cli#installation),
  [release downloads](https://github.com/AsamK/signal-cli/releases), and the
  [command manual](https://github.com/AsamK/signal-cli/blob/master/man/signal-cli.1.adoc).
- [Native library requirements](https://github.com/AsamK/signal-cli/wiki/Provide-native-lib-for-libsignal)
  and [upstream build recipes](https://github.com/AsamK/signal-cli/tree/master/reproducible-builds)
  for platforms without a matching release executable.

For a distinct Krabb identity, register a separate Signal account and send it
messages from your personal account. Standard registration uses another phone
number. Signal also documents
[numberless Android beta accounts](https://support.signal.org/hc/en-us/articles/11197884108826-Phone-Numberless-Registration-for-Android-beta);
that account type has not been verified with Krabb. Linking your personal account
instead makes Krabb reply as you to incoming messages from other accounts.
**Note to Self is not supported**: the adapter processes incoming `dataMessage`
events, not synchronized outgoing messages from your linked devices.

Install `signal-cli` on the machine running the Krabb daemon. Match the executable
to the machine's OS and architecture; the v0.14.9 Linux native release executable
is x86-64, not ARM64. An ARM64 native build can be produced in a container with
GraalVM and a matching ARM64 `libsignal` library, then installed on the host without
a Java runtime. Preserve any companion native libraries produced by that build.
Use the upstream build and library guidance above rather than executing an
incompatible release binary. Keep the client current as Signal changes.

Use the **same OS user as the Krabb service** for installation, linking, and setup.
The daemon must see both the executable and the account data in that user's
`signal-cli` data directory. Check the executable before configuring Krabb:

```sh
signal-cli --version
```

If the account is registered on your phone, link it from that host:

```sh
signal-cli link -n Krabb
```

Scan the QR code using the **bot account's** Signal app under Settings → Linked
devices. Follow the upstream linking instructions if your client version handles
QR display differently. Successful linking authorizes the client; it does not yet
configure Krabb. Do not run `register` again for an already registered phone
account: upstream documents that this replaces its existing primary client.

### Install The Krabb Plugin

The installer command at the top of this guide installs the published plugin.
When working from the repository, build and install the local plugin instead:

```sh
cargo build -p krabb-plugin-signal --locked
krabb plugin install signal --link --yes
```

These commands must use the same `KRABB_HOME` as your daemon. Ensure an intelligence
plugin is installed and authenticated; Signal transports messages, while the
intelligence plugin generates replies. See [provider setup](../../../krabb-docs/providers.md).

### Configure And Activate

Run:

```sh
krabb signal setup
```

Enter **the bot account's phone number**, including `+` and the country code, at
`KRABB_SIGNAL_ACCOUNT`. This is the account linked in the previous step, not the
personal account that will send test messages. For a numberless account, current
`signal-cli` uses its ACI/account ID instead; do not enter its recovery key.

Setup stores the account identifier in Krabb's encrypted credential store and
loads or reloads the Signal plugin in the running daemon. It does not copy Signal's
cryptographic keys, relink the account, install `signal-cli`, or verify a message
round trip. Look for the confirmation that the plugin was loaded. If activation is
pending, start the daemon with `krabb service start`, or restart an older daemon
with `krabb service restart`, then run `krabb doctor` and `krabb status`.

If no service has been installed yet, run `krabb service install` before
`krabb service start`. For foreground testing, run `krabb-daemon` instead of the
service, under the same OS user and with the same Krabb home. Only one daemon
should own that home at a time. See
[service commands](../../../krabb-docs/commands.md#initialization-and-service-management).

The daemon invokes `signal-cli` from its own `PATH`. A binary visible only in an
interactive shell is insufficient. On the tested Linux user-service installation,
`~/.local/bin` was already in the daemon's path, so no service-file edit was needed.
For another executable location, set `KRABB_SIGNAL_COMMAND` to its absolute path
in the **daemon's environment** and restart the service. This optional executable
override is not currently a prompt in `krabb signal setup`.

Nonempty `KRABB_SIGNAL_ACCOUNT` environment values override the saved account.
Remove stale service overrides if setup should control it. An export in a new
terminal does not update the environment of an already running service.
Use the same `KRABB_HOME` and customized credential paths for the CLI and daemon.
See [shared setup](../../../krabb-docs/authentication.md#plugin-setup) and
[service credentials](../../../krabb-docs/authentication.md#service-credentials).

### Test A Direct Message

With the daemon running, send the bot account a message from your personal Signal
account, for example, “Reply with pong.” A reply confirms the account, executable,
plugin, and intelligence provider work together. The separate-account flow has
been exercised on Linux ARM64 using a native build of signal-cli v0.14.9.

Krabb manages receiving and sending itself. Do not run another `signal-cli receive`,
`daemon`, or JSON-RPC receiver against the same linked-device data alongside Krabb.
No incoming webhook URL or public listening port is required for this integration.
Direct messages are accepted by default, subject to any configured sender or
message filters. Tools remain subject to the host's permissions and approval mode.

### Group Messages

Add the bot account to the Signal group, then allow that group's ID in Krabb.
The ID is the base64 group identifier reported by `signal-cli`, not the group's
display name or invitation link. See `listGroups` in the upstream command manual.
If inspecting the same account from the command line, stop Krabb first to avoid
competing for its account lock:

```sh
krabb service stop
signal-cli -a '<bot-number-in-international-format>' listGroups
krabb config set 'channels.signal.allow=["<group-id>"]'
krabb service start
```

Replace the placeholders with the actual values. The configuration command replaces
the complete allowlist; include all groups you want to keep. If the bot has just
joined a group, allow Krabb to receive the membership update before listing groups.

Additional sender and mention filters also apply. A configured mention is a text
marker that must appear in the message. Group membership alone does not grant
Krabb's tools or shell access. See
[channel policy](../../../krabb-docs/configuration.md#channel-policy) for permissions
and [provider selection](../../../krabb-docs/configuration.md#provider-and-model-precedence)
for a Signal-specific intelligence provider. Configuration changes require a daemon
restart; setup can activate Signal alongside other configured channels.

### Troubleshooting And Disconnecting

| Symptom | Check |
| --- | --- |
| `krabb signal setup` is unavailable | Install the Signal plugin in the daemon's `KRABB_HOME`. |
| `KRABB_SIGNAL_ACCOUNT is not configured` | Run setup and confirm activation; check stale environment overrides or a different credential store. |
| `signal-cli` is not found or cannot execute | Check the daemon's `PATH`, executable permission, OS, and CPU architecture. |
| Account is missing or not registered | Check the account identifier, OS user, data directory, and linked-device status using upstream guidance. |
| Account is locked or messages disappear from Krabb's receive stream | Stop other receivers using the same linked-device data. |
| Setup succeeds but no DM reply arrives | Confirm the daemon and intelligence provider are ready, message from another account, and check channel filters. |
| DMs work but groups do not | Check membership, the exact group ID in `channels.signal.allow`, and any sender or mention filters. |
| Note to Self receives no reply | That conversation type is not supported. Test from another account. |
| No typing indicator appears | Check Signal's typing settings and the client's `sendTyping` support; indicator failures do not prevent replies. |

Use `krabb doctor`, `krabb status`, and the application logs under
`KRABB_HOME/logs/` to distinguish activation, polling, and provider failures.
Do not paste account keys or credential files into diagnostics.

To stop the integration while keeping its installation, run:

```sh
krabb plugin detach signal
```

Detachment lasts for the current daemon lifetime. Use `krabb plugin attach signal`
to resume, or `krabb plugin uninstall signal` to remove the plugin permanently.
Uninstalling does not delete the external Signal account or its `signal-cli` data.
To revoke the linked client's access, remove it from the bot account's Linked
devices screen; follow [Signal's account-deletion guide](https://support.signal.org/hc/en-us/articles/360007061192-Delete-Account)
only if you also want to delete the account itself.

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

Signal quotes preserve the original timestamp, author and supplied text.
Missing reference text is marked unavailable; replies do not change sender permissions.
