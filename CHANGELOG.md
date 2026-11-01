# Changelog

All notable Krabb changes are documented here using Keep a Changelog and
Semantic Versioning.

## [Unreleased]

### Fixed

- Save memories reliably on Windows when data paths exceed the traditional path length limit.
- Return a complete busy response when the AG-UI server has no available request slots.

## [0.1.0]

### Added

- Run Krabb from the command line, as a daemon, or in an interactive terminal chat.
- Use hosted and local models from Codex, Anthropic, OpenAI, Gemini, Ollama, or OpenRouter.
- Connect Telegram, Discord, WhatsApp, Signal, and Slack to your agents.
- Give agents file, Git, shell, browser, and desktop tools with access controls.
- Connect agents and tools through ACP, A2A, AG-UI, and MCP.
- Save memories and organize data across conversations.
- Schedule agent tasks and send their results to chat channels.
- Send images to supported models and transcribe voice locally.
- Add loops for speeding up development and improving robustness.
