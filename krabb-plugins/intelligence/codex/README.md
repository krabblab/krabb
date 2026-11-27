# Codex

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- codex
```

The plugin drives the Codex CLI and signs in with your ChatGPT account: install the
Codex CLI and run `krabb codex login`, which `krabb init` offers after installing the
plugin. To use an OpenAI API key, install the `openai` provider instead.
Codex app-server owns browser or device-code login, credential storage, and
refresh. Krabb does not read or forward OAuth tokens. Set `KRABB_CODEX_HOME`
to select a Codex profile, or `KRABB_CODEX_BINARY` for a non-default
executable. Krabb checks the app-server calls it needs and fails closed if a
Codex update changes the protocol. No exact Codex CLI release is pinned;
compatibility is determined by the app-server operations Krabb uses.

After installation, `krabb codex` commands and foreground TUI model requests
can use the plugin immediately. Agent turns hosted by the background runtime
require that runtime to be running.

Discover the available operations with `krabb codex --help`. Use
`krabb codex login --help` for sign-in options, `krabb codex status` or
`krabb codex logout` to manage sign-in, and `krabb codex models` to list
account models. In the TUI, `/model help` shows model controls and
`/model list` lists Codex models; select one with `/model set <id>`.
Codex uses `gpt-6-luna` for an
unspecified, `default`, legacy `unset`, or blank model. It does not
inherit the Codex profile's model default. Explicit model IDs are preserved.
See [default models](../../../krabb-docs/providers.md#default-models) for verified
catalog sources and generation limits. Unavailable models fail without substitution.

Sign-in turns explicitly use live web search, overriding the cached-search default
of a read-only Codex session. Provider-hosted web search is separate from local
sandbox network access and Krabb's `web_fetch` tool. A search or cache miss does not prove that a URL is missing.

The plugin starts Codex read-only with its own shell off, so every change goes through
Krabb's tools and approvals. Its instructions tell the model that Codex's sandbox does
not limit those tools and that Krabb's permission summary is authoritative.

The plugin advertises image-request support during its handshake. It sends inline image inputs through the app-server
`turn/start` protocol and negotiates the experimental API needed for workspace
roots. Use a vision-capable selected model; Krabb does not pin the Codex CLI
release.

Krabb disables inherited MCP servers before each model thread so tools remain subject
to host approval. Unsupported configuration responses stop the request before execution.

Both routes use shared host context preparation before generation. Setup accepts
`KRABB_CODEX_CONTEXT_LENGTH` (`auto` or a token count greater than one) for the host
budget. Codex sign-in additionally manages its own harness context and generation
limits. Summary turns disable web search and dynamic tools. See
[context and search](../../../krabb-docs/providers.md#context).
