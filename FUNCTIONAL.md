# Functional Test Matrix

Track successful end-to-end checks for each v0.1 user-testable crate and plugin,
regardless of operating system or architecture. Mark `[x]` after the component
passes on an installed system; `[ ]` means functional success is not yet recorded.
Builds, unit tests, and protocol handshakes alone do not qualify.

Follow the [command guide](krabb-docs/commands.md) and plugin READMEs for setup,
prerequisites, and supported modes. Use test accounts and a disposable channel
for external integrations. Record results only by ticking existing checkboxes;
do not add status prose, dates, versions, evidence links, or notes. Leave partial,
untested, or unsupported checks unchecked.

A checked component has passed on at least one platform. Track basic operation
per OS/architecture in [Compatibility](COMPATIBILITY.md).

## Crates

Internal crates and shared libraries are exercised through the CLI, daemon, and
plugins and do not have separate manual checks.

| Crate  | Tested |
| ------ | ------ |
| CLI    | [x]    |
| Daemon | [x]    |

## Plugins

| Plugin     | Tested |
| ---------- | ------ |
| Anthropic  | [x]    |
| Codex      | [x]    |
| Discord    | [x]    |
| Gemini     | [x]    |
| Memory     | [x]    |
| Ollama     | [x]    |
| OpenAI     | [x]    |
| OpenRouter | [x]    |
| Scheduler  | [x]    |
| Signal     | [x]    |
| Slack      | [x]    |
| SQLite     | [x]    |
| Telegram   | [x]    |
| Tools      | [x]    |
| TUI        | [x]    |
| WhatsApp   | [x]    |
