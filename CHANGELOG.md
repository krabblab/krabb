# Changelog

All notable Krabb changes are documented here using Keep a Changelog and
Semantic Versioning.

## [0.1.0]

### Added

- Add plugin attach/detach mechanism for plugins for safe enabling/disabling.
- Support concurrent channels with isolated queues and approvals, plus global and per-channel/client provider/model defaults.
- Run Krabb from the command line, as a daemon, or in an interactive terminal chat.
- Use hosted and local models from Codex, Anthropic, OpenAI, Gemini, Ollama, or OpenRouter.
- Connect Telegram, Discord, WhatsApp, Signal, and Slack to your agents.
- Preserve supported rich formatting in messaging channels, including streamed edits and scheduled messages.
- Research user-supplied URLs with live Codex search and distinguish retrieval failures from missing pages.
- Give agents file, Git, shell, browser, and desktop tools with access controls.
- Connect agents and tools through ACP, A2A, AG-UI, and MCP.
- Save memories and organize data across conversations.
- Schedule agent tasks and send their results to chat channels.
- Send images to supported models and transcribe voice locally.
- Add loops for speeding up development and improving robustness.
- Make contribloop recover from stale verification, commit-hook failures, and invalid PR metadata until checks pass.
- Infer concise, single-word contribution branch names from the changes.
