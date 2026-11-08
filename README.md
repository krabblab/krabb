# Krabb

[![Main](https://github.com/krabblab/krabb/actions/workflows/main.yml/badge.svg)](https://github.com/krabblab/krabb/actions/workflows/main.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org/)

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

Install the CLI, daemon, and the plugins you plan to use. The
[first-run guide](krabb-docs/workflows.md) covers installation, provider setup,
and starting the daemon.

```sh
krabb init
krabb plugin list
krabb doctor
krabb status
krabb-daemon
```

Configure at least one intelligence provider before starting the daemon.
To receive chat messages, follow a
[channel setup guide](krabb-docs/providers.md#platform-setup).
The daemon stays in the foreground until you stop it with
Ctrl-C; use the [service guide](krabb-docs/workflows.md#background-service) to
run it in the background.

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
