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

Install the CLI and daemon on Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/krabblab/krabb/main/install.ps1 | iex
```

Then create the home, add one intelligence provider and one channel, and check them:

```sh
krabb init
krabb hatch
krabb plugin install openai telegram
krabb plugin setup openai
krabb plugin setup telegram
krabb status
krabb service install
krabb service start
```

`krabb hatch` gives your Krabb a name, a personality, and a way of writing.
`krabb status` lists each plugin's state and the next command to run. Use
`ollama` instead of a hosted provider for local inference. The
[first-run guide](krabb-docs/workflows.md#first-run) covers provider choice, data
locations, backups, upgrades, and uninstalling; the
[channel setup guides](krabb-docs/providers.md#channels) cover each platform.

## Capabilities

| Capability | Description | Plugins |
| --- | --- | --- |
| Channels | Connects Krabb to chat channels for receiving and sending messages. | Telegram, Discord, WhatsApp, Signal, Slack |
| Agent Interoperability | Connects external agents and local agent clients. | ACP, A2A, AG-UI |
| Tools | Provides filesystem, Git, shell, browser, and desktop operations under host policy. | Tools, Browser, Computer |
| MCP | Connects MCP servers and exposes their tools and resources. | MCP |
| Memory | Saves and searches scoped agent memories. | Memory |
| Scheduling | Creates and runs scheduled agent tasks. | Scheduler |
| Storage | Persists values, leases, and delivery state. | SQLite |
| Speech | Transcribes voice recordings with a local Whisper-compatible executable. | Whisper |
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

## Development

Use Rust 1.89 or newer and Cargo. Run the complete local checks before opening
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
