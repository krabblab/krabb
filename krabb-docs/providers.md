# Providers

Providers are plugins selected by capability, not compiled into the core. A
provider must normalize its API responses into Krabb protocol types and keep
vendor-specific fields inside the plugin process.

## Table Of Contents

- [Models](#models)
- [Channels](#channels)
- [Platform Setup](#platform-setup)
- [Deployment Validation](#deployment-validation)
- [Selection](#selection)
- [Other Capabilities](#other-capabilities)
- [Agent Interoperability](#agent-interoperability)

## Models

| Plugin | Service | Credential route |
| --- | --- | --- |
| `codex` | OpenAI-compatible chat completions or Codex app-server | `KRABB_CODEX_KEY` or Codex-managed sign-in; optional `KRABB_CODEX_BASE_URL` |
| `openai` | OpenAI API | `KRABB_OPENAI_KEY`; optional `KRABB_OPENAI_MODEL` and `KRABB_OPENAI_BASE_URL` |
| `anthropic` | Anthropic Messages | `KRABB_ANTHROPIC_KEY` or encrypted credentials; optional `KRABB_ANTHROPIC_BASE_URL` |
| `gemini` | Gemini Developer API | `KRABB_GEMINI_KEY` or encrypted credentials |
| `ollama` | Ollama local or cloud API | `KRABB_OLLAMA_HOST`, `KRABB_OLLAMA_API_KEY`, or encrypted credentials |
| `openrouter` | OpenAI-compatible routed models | `KRABB_OPENROUTER_KEY`, `KRABB_OPENROUTER_MODEL`, or encrypted credentials |

### Default models

An omitted model first inherits the host's
[argument, environment, and TOML defaults](configuration.md#provider-and-model-precedence).
`default`, the legacy `unset` value, and blank values resolve inside the selected
adapter. Explicit model names take precedence. OpenAI and
OpenRouter also honor their existing `KRABB_OPENAI_MODEL` and
`KRABB_OPENROUTER_MODEL` overrides before the built-in defaults.

These small/efficient defaults were verified against official catalogs on **2026-11-05**:

| Plugin / route | Default | Official source |
| --- | --- | --- |
| OpenAI | `gpt-6-luna` | [Model documentation](https://developers.openai.com/api/docs/models/gpt-6-luna) |
| Codex API key and ChatGPT sign-in | `gpt-6-luna` | [Codex models](https://learn.chatgpt.com/docs/models) |
| Anthropic | `claude-haiku-5-5` | [Haiku overview](https://platform.claude.com/docs/en/models/haiku-5-5/overview) |
| Gemini | `gemini-3.5-flash-lite` | [Flash-Lite documentation](https://aistudio.google.com/docs/models/gemini-3.5-flash-lite) |
| OpenRouter | `deepseek/deepseek-v4.1-flash` | [OpenRouter model](https://openrouter.ai/deepseek/deepseek-v4.1-flash) |
| Ollama local / other hosts | `qwen3.5:4b` | [Local model](https://ollama.com/library/qwen3.5:4b) |
| Ollama Cloud (`https://ollama.com`) | `deepseek-v4.1-flash` | [Live cloud catalog](https://ollama.com/api/tags) |

Qwen 3.5 4B is the selected small local Qwen model and must already be installed.
Qwen 3.6/3.8 have larger local tags; the checked Ollama cloud catalog does not list
Qwen 3.8 27B. DeepSeek V4.1 Flash is the selected efficient cloud model, verified
in both [Ollama's catalog](https://ollama.com/api/tags) and
[OpenRouter's catalog](https://openrouter.ai/deepseek/deepseek-v4.1-flash).
These Qwen and DeepSeek models support images and tools. Cloud API names omit
the CLI's `-cloud` suffix. Ollama Cloud requires an account and API key; a local
daemon proxying cloud models needs an explicit cloud model name. Krabb never pulls
models automatically.

Defaults are pinned in plugin releases, not discovered or upgraded at request time.
Unavailable defaults produce an error rather than falling back to a larger or older
model. Review the catalogs when updating these pins. Existing explicit session model
selections remain unchanged; saved `unset` sessions use these adapter defaults.
Codex sign-in now pins Luna instead of inheriting the account's model default.

Luna Chat Completions requests disable reasoning (required for tool calling) and cap
completion tokens at 4096. Ollama's selected default models disable thinking and
cap generation at 4096 tokens. OpenRouter's DeepSeek default disables reasoning
and caps output at 4096 tokens. Anthropic retains its 4096-token output limit.
These choices reduce incidental usage; they are not an aggregate spending limit.
Gemini's default stays on the current Flash-Lite even for old transcripts. Historical
tool calls missing thought signatures are rendered as text context, without replaying
tools or downgrading the model. Explicit Gemini 2.x selection retains the legacy
transcript compatibility path.

The OpenAI API plugin is independent from Codex and uses OpenAI API credentials.
Codex preserves the Codex app-server integration and optional compatible API
key route. Model plugins accept normalized messages and return a bounded `ModelReply`.
They validate provider status codes, response bodies, tool calls, and stream
events before returning them to the host. The Codex plugin uses an explicit
API key when configured; otherwise it delegates to the installed Codex CLI
app-server and its ChatGPT sign-in. Personal Codex sign-in requires Codex CLI
to be installed; other providers and API-key authentication do not require it.
The adapter validates app-server responses and fails closed if required
operations are incompatible. Run `krabb codex login` or
`krabb codex login --device` to authenticate.

Codex sign-in turns explicitly request live web search rather than inheriting
Codex's cached-search default. This is provider-hosted retrieval, separate from
Krabb's optional `fetch` tool and local sandbox network access. API-key adapters
do not gain built-in web search from this setting. When the tools plugin is
installed and channel tools are enabled, the agent is instructed to prefer
`fetch` for an exact HTTPS URL. Missing search results and cache misses are
retrieval limitations, not evidence that a repository does not exist.

OpenAI and OpenRouter send the full tool catalog supplied by the host. The
runtime permits up to 16 tool invocations per turn, counted once each across
model steps. OpenAI, OpenRouter, Anthropic, Ollama, and the Codex API-key adapter
coalesce text by size or a short timer and send at most 256 text notifications
per request. The final reply retains all generated text within
the response size limit, including text received after notifications stop.
Codex API-key and Ollama streams complete at their terminal marker without waiting
for transport closure. Trailing data cannot change the completed reply or invalidate
it, including when it shares a transport chunk with the marker.
OpenRouter resolves implicit selections using `KRABB_OPENROUTER_MODEL`, then its
built-in DeepSeek V4.1 Flash default. Explicit model names take precedence.

Telegram images are inlined only after the host confines and bounds their
cached files. Codex-compatible chat completions use image URL content parts;
Anthropic uses base64 or HTTPS image blocks; Ollama receives base64 image data;
and Codex app-server receives inline data URLs. Model plugins advertise image
request support during their handshake, and the host rejects image turns when
the selected adapter does not advertise it. The selected model must still
support vision; Krabb does not silently substitute another model. Historical
downloaded images expire from the cache after 24 hours, while images committed
to a session are retained in its protected media area.

Codex sign-in forwards normalized screenshot results as app-server image items,
with the complete response bounded by the protocol frame limit. Gemini preserves
inline screenshot pixels alongside function responses and legacy tool history.

## Channels

| Plugin | Transport | Required secret |
| --- | --- | --- |
| `telegram` | Bot API long polling | `KRABB_TELEGRAM_TOKEN` |
| `discord` | Gateway receive and REST send | `KRABB_DISCORD_TOKEN` |
| `whatsapp` | Cloud API webhook receive and Graph API send | `KRABB_WHATSAPP_TOKEN`, `KRABB_WHATSAPP_APP_SECRET`, `KRABB_WHATSAPP_VERIFY` |
| `signal` | Managed signal-cli receive and send | `KRABB_SIGNAL_ACCOUNT` |
| `slack` | Web API polling and Socket Mode credentials | `KRABB_SLACK_BOT_TOKEN`, `KRABB_SLACK_APP_TOKEN` |

Channel plugins own polling cursors, provider limits, message chunking and
edits, and attachment normalization. They never decide whether a sender is
Channel turns use portable Markdown. Channel plugins advertise accepted input
formats in their handshake and render supported formatting at the platform boundary.
Telegram uses native message entities, Signal uses signal-cli text styles, Slack
uses structured rich-text blocks, and Discord and WhatsApp receive their own markup.
Emphasis, links, code, lists, headings, and quotes retain their supported presentation;
unsupported structures remain readable text. Signal and WhatsApp expose link targets
as visible URLs. Plugins without Markdown support receive a plain-text fallback.

Formatting applies to sends, streamed edits, scheduled messages, and delivery retries.
The original Markdown stays in transcripts and the outbox. Rich-text chunks keep
styles valid and obey platform limits; streaming drafts update only the first chunk,
with overflow sent once during final delivery. TUI and CLI formatting remain unchanged.

Use the channel guide for installation and Krabb setup. Each guide links to the
platform's own documentation for creating accounts, apps, permissions, and tokens.

| Channel | Krabb guide | External setup |
| --- | --- | --- |
| Telegram | [Setup](../krabb-plugins/channels/telegram/README.md#setup) | [Telegram bot tutorial](https://core.telegram.org/bots/tutorial) |
| Discord | [Setup](../krabb-plugins/channels/discord/README.md#setup) | [Discord getting started](https://docs.discord.com/developers/quick-start/getting-started) |
| WhatsApp | [Setup](../krabb-plugins/channels/whatsapp/README.md#setup) | [Meta Cloud API](https://developers.facebook.com/documentation/business-messaging/whatsapp/get-started) |
| Signal | [Setup](../krabb-plugins/channels/signal/README.md#setup) | [signal-cli installation and account setup](https://github.com/AsamK/signal-cli#usage) |
| Slack | [Setup](../krabb-plugins/channels/slack/README.md#setup) | [Slack Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode/) |

After installation, run `krabb <id> setup` as the same OS user as the daemon.
Setup saves the declared secrets and settings in encrypted storage and requests
activation. Follow the channel guide for any external prerequisites, then send
a direct message and check the reply. Existing channels run alongside new ones.
For group conversations, configure [channel policy](configuration.md#channel-policy).

`krabb plugin setup <id>` is the full command. All plugins declaring secrets or
settings support setup, including intelligence plugins: `krabb codex setup`
configures an API key, while `krabb codex login` performs personal account sign-in.
See [shared setup](authentication.md#plugin-setup) for service paths, environment
overrides, and activation details.

## Deployment Validation

Setup configures credentials; a successful setup or handshake does not prove
that an external account, model, or channel is ready. Local fixture tests
cover protocol behavior and failure recovery. Record real installation results
separately in [Compatibility](../COMPATIBILITY.md).

Before enabling an integration for regular use:

1. Install the plugin, run `krabb <id> setup`, and configure the model or channel
   explicitly. Use `krabb doctor` to inspect local health. Confirm the account's
   model access, app permissions, and channel allowlist.
2. With a test account, complete a text turn, a two-step tool turn, and an image
   turn when the selected model supports images. Check that repeated calls to
   the same tool receive their own results and that streaming reaches completion.
3. For a channel, verify receive/reply, long messages, thread routing, and permitted
   media. Try an unauthorized sender and confirm no agent turn is started.
4. Restart the daemon with an inbound event awaiting acknowledgement. Check
   recovery and delivery state. Disconnect and reconnect the integration; ensure
   it resumes without replaying a mutating tool or losing accepted messages.
5. Run it under representative load and monitor errors, latency, memory, disk,
   and queue depth. Record the model/service version, platform, duration, load,
   and outcome; choose acceptance limits appropriate to the deployment.

Automated tests use synthetic credentials and loopback HTTP/WebSocket or process
fixtures. They never contact live providers. Live acceptance is an operator-run
step and may incur provider charges. A passing local suite is not a live-service
or cross-platform production certification.

All five non-Codex model adapters retain structured tool history. Gemini now
streams SSE responses as well; blocked or empty responses are errors. Streaming
adapters reject premature connection closure instead of returning partial tool
calls as a successful generation. Native tool history uses the existing protocol;
legacy text-only history remains readable without inventing provider call IDs
for old text markers.
If a previous turn ended without recording a tool result, the next request marks
its execution status as unknown. It does not replay the tool or report success.

The daemon currently sends generated Discord and WhatsApp replies as text even
though their adapters support media uploads. Signal requires an independently
installed and registered `signal-cli`; Slack needs its external app configured;
WhatsApp needs a public HTTPS webhook proxy. These are deployment prerequisites,
not actions performed by `setup`.

## Selection

Use `krabb plugin list` and `krabb doctor` to inspect installed providers.
All loaded channel plugins receive messages concurrently. Set `provider` and
`model` under `[channels.<id>]` to route different channels to different intelligence
plugins; see [channel configuration](configuration.md#channel-policy).
The global provider/model remains the fallback, and TUI sessions can choose their
provider independently. Requests to distinct provider processes can run concurrently;
requests to the same process are serialized.

## Other Capabilities

| Capability | Plugins | What it provides |
| --- | --- | --- |
| `tool` | Tools, Browser, Computer | Filesystem and Git operations, approved shell access, browser automation, and desktop control. |
| `mcp`, `resource` | MCP | Access to tools and resources exposed by MCP servers. |
| `memory` | Memory | Scoped agent memory records with search and management commands. |
| `scheduler` | Scheduler | Persistent scheduled agent tasks and controlled execution. |
| `store` | SQLite | Agent-accessible structured data, key/value values, leases, durable queue records, and event deduplication. |
| `speech` | Whisper | Voice transcription through a local Whisper-compatible executable. |
| `client` | TUI | A terminal interface connected to the daemon. |

Model plugins also advertise the `vision` capability when they accept image
inputs. The host checks that capability before dispatching an image turn.

## Agent Interoperability

The `acp` plugin starts a configured local executable that implements ACP. The
`a2a` plugin discovers and calls a remote A2A agent. The `ag-ui` plugin serves
a loopback run endpoint with run, text, frontend tool-call, state snapshot, and
error events. These protocols connect compatible agents; they do not imply
compatibility with unrelated agent-specific interfaces. See each plugin README
for supported features and limits. ACP and A2A coalesce updates after the first
255 notifications, reserving one final notification for the remaining text.
ACP filesystem calls are routed through Krabb's tools plugin with the selected
workspace confined to `KRABB_ROOT` and writes subject to the approval mode.
The daemon reports a missing or incompatible capability instead of silently
substituting another provider. Select a model for a session with
`krabb session model <id> <model>`. When a model plugin is installed, use
`krabb ask [--provider <id>] [--model <model>] <prompt...>` for a one-shot request.
Omitted flags inherit the configured defaults using the precedence described above.
