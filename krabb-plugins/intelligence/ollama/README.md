# Ollama

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- ollama
```

The plugin supports local and cloud Ollama chat, images, tool calls, and streaming.
The default host is `https://ollama.com`. Run `krabb ollama setup` to save the API
key and host in encrypted storage and reload the plugin in the running daemon.
Changing the saved host does not require reinstalling the Krabb service.

| Setup | Host entered during setup | Model selection |
| --- | --- | --- |
| Cloud | Press Enter for `https://ollama.com` | A model available to your Ollama account |
| Local | `http://127.0.0.1:11434` | An exact model tag installed on that server |
| Another server | Its reachable base URL, including the port | A model installed on that server |

Use a base URL without `/api` or `/v1`; the plugin appends `/api/chat`. Loopback
refers to the machine or container running the Krabb daemon.

Setup prompts for `KRABB_OLLAMA_API_KEY` first, then `KRABB_OLLAMA_HOST`. For local
use, press Enter at the API-key prompt to leave it empty and clear any previously
saved key. The local Ollama server requires no key. Cloud use requires a valid key.
Nonempty environment values override saved settings. Remove old overrides before
using setup as described under [Existing Service Overrides](#existing-service-overrides).
See [authentication](../../../krabb-docs/authentication.md#plugin-setup) for storage
and activation details.

## Cloud Setup

Create an [Ollama API key](https://ollama.com/settings/keys), then run:

```sh
krabb ollama setup
```

Enter the API key and press Enter at the host prompt to accept `https://ollama.com`.
Direct cloud access does not require installing Ollama, starting a local server,
or downloading a model. Your account must have access and sufficient allowance or
credits for the selected model; a public catalog listing does not prove account
entitlement. Check [Ollama pricing](https://ollama.com/pricing) and your account's
usage before testing.

For example, select DeepSeek for Telegram:

```sh
krabb config set 'channels.telegram.provider="ollama"'
krabb config set 'channels.telegram.model="deepseek-v4.1-flash"'
```

Use the model name from the [direct cloud catalog](https://ollama.com/api/tags).
Tags ending in `:cloud` are used when routing cloud models through a local Ollama
server; direct requests to `https://ollama.com` use that catalog's names.

## Local Setup

Install [Ollama](https://docs.ollama.com/quickstart) on the server that will run the
model. Choose a model and quantization that fit its RAM and, when using a GPU, its
VRAM. Krabb uses the same API for CPU and GPU execution; Ollama handles hardware
selection. Follow [Ollama's GPU guide](https://docs.ollama.com/gpu) for supported
hardware and drivers.

Start Ollama, then download and test your chosen model. On Linux with the Ollama
system service installed:

```sh
sudo systemctl start ollama
ollama pull <model-tag>
ollama run <model-tag> "Reply with hello."
```

Replace `<model-tag>` throughout with the exact installed tag. If running Ollama
in the foreground instead, start `ollama serve` in a separate terminal. Keep the
server running while Krabb uses it. Krabb never downloads models automatically.

Run `krabb ollama setup`, press Enter at the API-key prompt, and explicitly enter
`http://127.0.0.1:11434` at the host prompt. Pressing Enter at the host prompt selects
cloud. For a separate server, use its
reachable base URL. Remote requests carrying a key require HTTPS.

Select the installed model for Telegram:

```sh
krabb config set 'channels.telegram.provider="ollama"'
krabb config set 'channels.telegram.model="<model-tag>"'
```

These changes apply to the next turn in existing Telegram conversations, preserving
their history. An explicit `krabb session model` selection pins that conversation;
run `krabb session model <telegram-session-id> inherit` to follow the channel again.
Changing only the host through setup reloads the plugin directly. Direct edits to
`config.toml` still require a daemon restart. Retained history contributes to
processing time and memory use.

### Small CPU Test

For a lightweight smoke test, [Qwen 3.5 0.8B](https://ollama.com/library/qwen3.5:0.8b)
is a small example:

```sh
ollama pull qwen3.5:0.8b
ollama run qwen3.5:0.8b "Reply with hello."
```

Use `qwen3.5:0.8b` for `<model-tag>` in the local setup commands, then send a short
text message through Telegram. This example does not limit local setup to small
models; larger CPU or GPU models use the same steps. Speed and reliability depend
on the model, hardware, and conversation. A basic reply does not verify image or
tool behavior.

Ollama's context must accommodate both the conversation and generated output.
Long retained histories can exhaust a small context before the model produces a
visible answer, particularly with thinking models. See
[Ollama's context guide](https://docs.ollama.com/context-length) to adjust it for
your available memory. The plugin rejects completed streams containing neither
visible text nor tool calls, so an empty reply becomes a failed turn rather than
a blank channel message. Its HTTP timeout is five minutes; daemon turns share a
five-minute overall budget across model calls and tools, and remain cancellable.

## Existing Service Overrides

If an earlier service install captured `KRABB_OLLAMA_HOST`, remove that override
once so setup can control the host. For Krabb's Linux user service, run
`systemctl --user edit krabb.service` and add:

```ini
[Service]
UnsetEnvironment=KRABB_OLLAMA_HOST
```

Then run:

```sh
unset KRABB_OLLAMA_HOST
systemctl --user daemon-reload
krabb service restart
krabb ollama setup
```

This keeps the installed service and its other configuration. A nonempty
`KRABB_OLLAMA_API_KEY` environment override must also be removed if setup reports
pending activation; credentials saved through setup need no service override.
On other service managers, remove the host override from the daemon's environment
and restart once. Later host changes can use setup alone.

## Protocol

Structured tool calls and their results retain the provider's native format
across turns. Interrupted streams without the provider's completion marker
fail the generation; partial tool calls are not returned for execution.
Local use requires a running Ollama server and an installed model; an API key
is optional. Remote requests carrying an API key require HTTPS. An unauthenticated
HTTP server remains supported on a trusted network. Hosts cannot contain embedded
credentials, a query, or a fragment.

See [deployment validation](../../../krabb-docs/providers.md#deployment-validation)
before relying on this adapter in production.

Blank, `default`, and legacy `unset` model selections use `qwen3.5:4b` on
local/custom hosts, and `deepseek-v4.1-flash` on `https://ollama.com`. Host matching uses the
parsed hostname, not substring matching or the presence of an API key. Explicit
model names always win, including cloud model tags used through a local proxy.

The local Qwen 4B must already be installed. Both selected models support images
and tools; cloud calls require authentication and account access. Krabb never downloads
models or retries with a different model. Default models disable thinking and cap
generation at 4096 tokens. See [default models](../../../krabb-docs/providers.md#default-models)
for current catalog sources and the verification date.
