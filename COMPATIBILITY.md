# Compatibility

Track functional checks on each operating system and architecture. Mark `[x]`
only after the check passes on that platform; builds, unit tests, and protocol
handshakes alone do not qualify. Record results only by ticking the existing
checkboxes; do not add status prose, dates, versions, evidence links, or notes.

## Basic Checklist

Run these checks on an installed system. Use test accounts and a disposable
channel for external integrations.

- **Setup:** Run `krabb init` and `krabb doctor`; confirm plugin health.
- **Daemon:** Start, stop, and restart the daemon; confirm persistent state.
- **Intelligence:** Complete a model turn with a configured intelligence provider.
- **Channel:** Complete a message round trip through a configured channel.
- **Client:** Connect through a client, such as the TUI, and complete an interaction.
- **Plugins:** Successfully install at least one plugin.

Tick each check separately for every platform tested. A completed row means the
basic checks passed; use the detailed checklist for individual capabilities.

| Platform          | Setup | Daemon | Intelligence | Channel | Client | Plugins |
| ----------------- | ----- | ------ | ------------ | ------- | ------ | ------- |
| Linux (x86_64)    | [ ]   | [ ]    | [ ]          | [ ]     | [ ]    | [ ]     |
| Linux (aarch64)   | [x]   | [x]    | [x]          | [x]     | [x]    | [x]     |
| macOS (x86_64)    | [ ]   | [ ]    | [ ]          | [ ]     | [ ]    | [ ]     |
| macOS (aarch64)   | [ ]   | [ ]    | [ ]          | [ ]     | [ ]    | [ ]     |
| Windows (x86_64)  | [ ]   | [ ]    | [ ]          | [ ]     | [ ]    | [ ]     |
| Windows (aarch64) | [ ]   | [ ]    | [ ]          | [ ]     | [ ]    | [ ]     |

See [workflows](krabb-docs/workflows.md), [providers](krabb-docs/providers.md),
[commands](krabb-docs/commands.md), and [safety](krabb-docs/safety.md) for instructions.

## Detailed Checklist

Test each component's end-to-end behavior on each platform, following the plugin
READMEs for setup, prerequisites, and supported modes. Tick the corresponding
cell after it passes. Leave partial, untested, or unsupported checks unchecked.

| Component  | Linux (x86_64) | Linux (aarch64) | macOS (x86_64) | macOS (aarch64) | Windows (x86_64) | Windows (aarch64) |
| ---------- | -------------- | --------------- | -------------- | --------------- | ---------------- | ----------------- |
| CLI        | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Core       | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Daemon     | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Runtime    | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Date       | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| File       | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Logging    | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Process    | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| A2A        | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| ACP        | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| AG-UI      | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Anthropic  | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Browser    | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Codex      | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Computer   | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Discord    | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Gemini     | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| MCP        | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Memory     | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Ollama     | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| OpenAI     | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| OpenRouter | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Scheduler  | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Signal     | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Telegram   | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Tools      | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| TUI        | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| WhatsApp   | [ ]            | [x]             | [ ]            | [ ]             | [ ]              | [ ]               |
| Whisper    | [ ]            | [ ]             | [ ]            | [ ]             | [ ]              | [ ]               |
