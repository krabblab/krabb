# Slack

This guide covers direct messages, public and private workspace channels, and
the contexts where Slack can show typing status.

Install the plugin:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- slack
```

## Setup

### Create The Slack App

The [Slack app manifest](app.yaml) configures a minimal direct-message text test:
Socket Mode, a writable Messages tab, the `message.im` event, and the
`chat:write` and `im:history` bot scopes. This is Slack's app configuration;
`krabb-plugin.toml` is the separate Krabb plugin manifest.

1. Open [Slack Apps](https://api.slack.com/apps) and choose **Create New App**.
2. Choose **Paste a manifest**, select your workspace, and paste the contents of
   [app.yaml](app.yaml) into the YAML editor. Review and create the app.
3. Under **Basic Information → App-Level Tokens**, generate a token named `krabb`
   with the `connections:write` scope. Keep this `xapp-…` app token for setup.
4. Under **Socket Mode**, confirm **Enable Socket Mode** is on. Krabb establishes
   the connection; you do not need to call `apps.connections.open` yourself.
5. Under **OAuth & Permissions**, install the app into your workspace and keep
   its `xoxb-…` bot token for setup.

See [Slack's manifest reference](https://docs.slack.dev/reference/app-manifest/)
and [Socket Mode guide](https://docs.slack.dev/apis/events-api/using-socket-mode/)
for the platform settings. The manifest contains no credentials.

### Configure Krabb

For a development checkout, install the locally built plugin from the repository
root instead of using the release installer above:

```sh
cargo build --locked -p krabb-plugin-slack
krabb plugin install slack --link --yes
```

Existing installations must refresh the installed manifest with the command
above before setup can offer the new conversation-list prompt.

### Find The Conversation ID

The plugin requires a conversation list for **Socket Mode events as well as
history polling**, including direct messages. To find the DM conversation ID:

1. Open the app's Messages tab in Slack and send a message.
2. Open that message's **⋯ menu → Copy link**.
3. Copy the segment after `/archives/`. For example,
   `https://your-workspace.slack.com/archives/D0123456789/p...` identifies
   conversation `D0123456789`. Use the conversation ID, not a user ID or app ID.

### Save Tokens And Conversations

Run:

```sh
krabb slack setup
```

Enter the three prompted values:

| Prompt | Value |
| --- | --- |
| `KRABB_SLACK_BOT_TOKEN` | Your `xoxb-…` bot token. |
| `KRABB_SLACK_APP_TOKEN` | Your `xapp-…` app token. |
| `KRABB_SLACK_CHANNELS` | One conversation ID, or a comma-separated list such as `D0123456789,C9876543210`. |

Setup saves all three values in encrypted storage and asks the running daemon to
load or reload the Slack plugin. Saving these values needs no service-file editing
or `krabb config set` command. Workspace channels also need the host allowlist
described below. Run setup again to replace the conversation list; it prompts
for both tokens again too.

See [shared setup](../../../krabb-docs/authentication.md#plugin-setup) for encrypted
storage, daemon activation, environment overrides, and service configuration.
If you previously set `KRABB_SLACK_CHANNELS` in the daemon's environment, remove
that override and restart the daemon once so setup can activate the saved value.
Nonempty environment variables continue to take precedence over saved settings.

```sh
krabb status
krabb doctor
```

The supplied manifest is limited to text DMs. For workspace channels, follow
[Workspace Channels](#workspace-channels) after the initial setup.

### Test A Reply

Send a fresh direct message after setup reports that the plugin is loaded:

> Remember the word pineapple. Reply with bold, italic, inline code, and a short code block.

Then ask:

> What word did I ask you to remember?

This checks receiving, replies, formatting, and conversation history. If no reply
arrives, check both tokens, the `message.im` subscription, the writable Messages
tab, and the conversation ID entered during setup. For service diagnostics:

```sh
journalctl --user -u krabb.service -n 100 --no-pager
```

## Workspace Channels

Enable each conversation explicitly. Installing the app into a Slack workspace
does not give Krabb access to every channel.

### Slack Permissions And Membership

In your Slack app settings, keep the existing `chat:write`, `im:history`, and
`message.im` entries, then add the entries for the channel types you want:

| Conversation | Add under OAuth & Permissions → Bot Token Scopes | Add under Event Subscriptions → Bot Events |
| --- | --- | --- |
| Public channel | `channels:history` | `message.channels` |
| Private channel | `groups:history` | `message.groups` |

These are Slack's [public-channel events](https://docs.slack.dev/reference/events/message.channels/)
and [private-channel events](https://docs.slack.dev/reference/events/message.groups/).
The current plugin processes `message` events; subscribing only to `app_mention`
does not enable Socket Mode message handling.

1. Save the event subscriptions and reinstall the Slack app into your workspace
   to approve the additional scopes.
2. Invite the app into each channel, including private channels. In the channel,
   use `/invite @Krabb` or add the app through the channel's Integrations tab.
3. Copy a message link from that channel and take the conversation ID after
   `/archives/`, as in the DM steps. Use the actual ID from Slack, not the channel
   name; do not infer whether it is public or private from its ID prefix.

### Krabb Conversation Lists

Both lists below are required for a workspace channel. Run the commands on the
machine running Krabb, as its service user and with the same `KRABB_HOME`.

| Setting | Purpose | How to set it |
| --- | --- | --- |
| `KRABB_SLACK_CHANNELS` | Conversations the Slack plugin receives, including DMs. | `krabb slack setup` |
| `channels.slack.allow` | Workspace channels the host permits Krabb to answer in. | `krabb config set` |

Run setup again:

```sh
krabb slack setup
```

Enter both tokens and the complete comma-separated conversation list, preserving
your existing DM ID. For example, replace these IDs with your DM and channel IDs:

```text
D0123456789,C0123456789
```

Then allow the workspace channel in Krabb's policy:

```sh
krabb config set 'channels.slack.allow=["C0123456789"]' --force
```

This replaces the allowlist, so include all workspace channels you want to keep.
`--force` restarts a running daemon to apply the policy. Private Slack channels
also require an allowlist entry; they are not direct messages. DM IDs need to
remain in `KRABB_SLACK_CHANNELS` but do not need to be added to `channels.slack.allow`.

Send a fresh message in each configured channel. With no additional policy
filters, Krabb responds to ordinary human messages there. Existing `owner`,
`admin`, `member`, `mention`, `topic`, or `thread` filters still apply; see
[channel policy](../../../krabb-docs/configuration.md).

## Typing Status

| Context | Expected behavior |
| --- | --- |
| Ordinary DM message outside a thread | No typing indicator. |
| Supported assistant thread in a DM | Krabb attempts to show typing status. |
| Public or private workspace channel | No typing indicator. |

For supported DM threads, the plugin uses
[`assistant.threads.setStatus`](https://docs.slack.dev/reference/methods/assistant.threads.setStatus/).
Slack requires an app configured for its assistant or agent messaging experience
and access to that method (`chat:write`, or the legacy `assistant:write` scope).
The supplied minimal DM manifest does not configure that experience; creating an
ordinary message thread alone does not guarantee typing status is available.
Slack clears the status on reply or after its two-minute timeout. Unsupported
contexts and API failures do not prevent replies.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Slack will not let you message the app | Enable the writable Messages tab in App Home. |
| Neither DMs nor channels receive replies | Check both tokens, Socket Mode, event subscriptions, and the conversation list saved by setup. |
| DMs work but a channel does not | Check the channel's event and scope, reinstall the Slack app, invite it into the channel, and include the ID in both Krabb lists above. |
| A private channel does not work | Add `groups:history` and `message.groups`, invite the app, and add its conversation ID to both lists. |
| Setup saves a new list but activation stays pending | Remove any old `KRABB_SLACK_CHANNELS` service/environment override and restart the daemon once. |
| Setup offers only the two token prompts | Update or reinstall the Krabb Slack plugin to refresh its installed manifest. |
| Replies arrive without typing status | Check the context table above; ordinary DMs and channels do not show it. |

Run `krabb status` and `krabb doctor` on the daemon's machine. For a Linux systemd
user service, inspect `journalctl --user -u krabb.service -n 100 --no-pager`.

## Messages And Media

Krabb responds to human messages. Messages identified by Slack as bot messages
through their subtype, `bot_id`, or `bot_profile` are ignored, including Krabb's
own replies, to prevent reply loops.

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
