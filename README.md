# Krabb

[![Main](https://github.com/krabblab/krabb/actions/workflows/main.yml/badge.svg)](https://github.com/krabblab/krabb/actions/workflows/main.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-1E2327.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-FF6F2C.svg)](https://www.rust-lang.org/)

![Krabb: a self-hostable, open-source agent operating system](krabb-assets/brand/png/krabb-social-preview-coral-1280x640.png)

Krabb is a self-hostable, open-source agent operating system for building,
running, and orchestrating native and external AI agents.

Krabb keeps its core provider-neutral and adds capabilities through isolated
plugins. Native agents use the Krabb runtime. External agents connect through
ACP, agent collaboration uses A2A, and user interfaces connect through AG-UI.

> Krabb is pre-release software. Commands and configuration may change
> incompatibly before version 1.0.

## Table Of Contents

- [Get Started](#get-started)
- [Capabilities](#capabilities)
- [Resources](#resources)
- [Documentation](#documentation)
- [Development](#development)
- [Support And License](#support-and-license)

## Get Started

Krabb builds for Linux, macOS, and Windows on x86_64 and aarch64. A platform is
experimental until its row in [COMPATIBILITY.md](COMPATIBILITY.md) is complete.

Install the CLI and daemon on Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/krabblab/krabb/main/install.ps1 | iex
```

> [!NOTE]
> Until Krabb 0.1.0 is released, install the release candidate with the installer
> attached to it, which installs that release:
>
> ```sh
> curl -fsSL https://github.com/krabblab/krabb/releases/download/v0.1.0-rc.6/install.sh | sh
> ```
>
> In PowerShell:
>
> ```powershell
> irm https://github.com/krabblab/krabb/releases/download/v0.1.0-rc.6/install.ps1 | iex
> ```

The installer puts `krabb` and `krabb-daemon` in `~/.local/bin`, or
`%LOCALAPPDATA%\Krabb\bin` on Windows, where it is also added to your `PATH`. If your
shell then reports `command not found: krabb`, add the line the installer printed to
your shell profile, such as `~/.zshrc`, and open a new terminal.

To install without running a script, download the archive for your platform from
[GitHub Releases](https://github.com/krabblab/krabb/releases) and follow
[Manual Download](krabb-docs/workflows.md#manual-download). Archives are named
`krabb-core-v<version>-<target>`, with these targets:

- Linux: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (`.tar.gz`)
- macOS: `x86_64-apple-darwin`, `aarch64-apple-darwin` (`.tar.gz`)
- Windows: `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` (`.zip`)

Then set it up:

```sh
krabb init
```

`krabb init` guides you through five steps: the home, an intelligence provider, a
personality, where you talk to your Krabb, and the background service. Run only the
steps you need, such as `krabb init provider`, and run it again any time to continue
or change one. `krabb status` lists each plugin's state and the next command to run.

If you chose Terminal during `krabb init`, send your first message once the service
is running:

```sh
krabb tui
```

`krabb tui --once "Hello"` sends one message and prints the reply. With a channel
such as Telegram, send your bot a direct message; groups first need an `allow`
entry in the [channel policy](krabb-docs/configuration.md#channel-policy).

Remote inference sends your conversation to a hosted provider and needs an account
with a key or sign-in. Local inference runs a model on an Ollama server you
control, keeps conversations off hosted services, and needs the memory and CPU
described in the
[resource guide](krabb-docs/resources.md).

| Provider | Inference | What setup asks for |
| --- | --- | --- |
| OpenAI, Anthropic, Gemini, OpenRouter | Remote | The provider's API key |
| Codex | Remote | `krabb codex login` with the Codex CLI installed |
| Ollama | Local or remote | The local server address, such as `http://127.0.0.1:11434`; an API key for Ollama Cloud |

Telegram and Discord need a bot token, Slack needs bot and app tokens, Signal
needs a registered or linked `signal-cli` account, and WhatsApp needs Meta Cloud
API credentials and a public HTTPS callback; the
[channel setup guides](krabb-docs/providers.md#channels) cover each platform.
Some plugins need software Krabb does not install: local inference needs Ollama,
Codex sign-in needs the Codex CLI, Signal needs `signal-cli`, and sandboxed shell
tools need Docker or Podman.

The [first-run guide](krabb-docs/workflows.md#first-run) covers provider choice,
backups, and upgrades. To remove Krabb, run the
[uninstall script](krabb-docs/workflows.md#uninstall); it keeps your data unless you
purge it. [Storage](krabb-docs/storage.md) lists where
Krabb keeps its data, and [Troubleshooting](krabb-docs/troubleshooting.md) helps
when a reply does not arrive.

## Capabilities

| Capability | Description | Plugins |
| --- | --- | --- |
| Intelligence | Connects Krabb to the models it thinks with. | OpenAI, Anthropic, Gemini, OpenRouter, Codex, Ollama |
| Channels | Connects Krabb to chat channels for receiving and sending messages. | Telegram, Discord, WhatsApp, Signal, Slack |
| Agent Interoperability | Connects external agents and local agent clients. | ACP, A2A, AG-UI (experimental) |
| Tools | Provides filesystem, Git, shell, browser, and desktop operations under host policy. | Tools; Browser, Computer (experimental) |
| MCP | Connects MCP servers and exposes their tools and resources. | MCP (experimental) |
| Memory | Saves and searches scoped agent memories. | Memory |
| Scheduling | Creates and runs scheduled agent tasks. | Scheduler |
| Background Execution | Runs extended background operations through external agents. | Runner (experimental) |
| Storage | Persists values, leases, and delivery state. | SQLite |
| Speech | Transcribes voice recordings with a local Whisper-compatible executable. | Whisper (experimental) |
| Terminal Client | Provides an interactive terminal interface to the daemon. | TUI |

Plugins outside the [v0.1 functional test matrix](FUNCTIONAL.md) are experimental,
not fully tested, and outside the supported v0.1 scope. Their READMEs identify this status.

All plugins are optional. Intelligence plugins live in
`krabb-plugins/intelligence/`; channel plugins live in
`krabb-plugins/channels/`; client plugins, including TUI, live in
`krabb-plugins/clients/`. Plugin manifests declare capabilities and permissions.

## Resources

A minimal Krabbling needs a daemon, one intelligence plugin, and one channel or
client. Together they can use roughly **50-70 MiB of RAM** with remote inference. For example, the daemon
with Codex and Telegram is around 70 MiB. CPU use is near zero while idle.
Additional plugins and local model inference add their
own resource needs. See the [resource guide](krabb-docs/resources.md) for basic
sizing figures and what they include.

## Documentation

Use the [documentation home](krabb-docs/home.md) for the full guide index.
Common starting points:

- [First Run](krabb-docs/workflows.md)
- [Provider Setup](krabb-docs/providers.md)
- [Configuration](krabb-docs/configuration.md)
- [Visibility And Worktrees](krabb-docs/sharing.md)
- [Storage](krabb-docs/storage.md)
- [Plugin Development](krabb-docs/plugins.md)
- [Tools](krabb-docs/tools.md)

## Development

Use Rust 1.96 or newer and Cargo. Run the complete local checks before opening
a pull request:

```sh
lefthook install
make verify
```

See [contributing](CONTRIBUTING.md) for repository conventions. Tests use local
fixtures and do not call paid provider APIs.

## Support And License

Run `krabb doctor` for local diagnostics. Read [support](SUPPORT.md) before
reporting an issue and [security](SECURITY.md) before reporting a vulnerability.
Krabb is licensed under [Apache-2.0](LICENSE).
