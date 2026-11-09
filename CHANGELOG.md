# Changelog

All notable Krabb changes are documented here using Keep a Changelog and
Semantic Versioning.

## [0.1.0]

### Added

- Update plugins through a graceful handover with declaration review and rollback.
- Handle oversized histories without repeated failure replies.
- Compact conversation history across intelligence providers before truncating it.
- Enable native web search for supported models.
- Configure Ollama thinking and context through setup.
- Change channel models live while preserving conversations.
- Allow daemon model requests to use the full five-minute turn budget.
- Clear saved plugin keys through interactive setup.
- Switch local and cloud Ollama hosts through interactive setup.
- Configure Slack conversations through interactive setup.
- Display rich text and code panels in terminal conversations.
- Control data visibility between integrations and isolate conversations with worktrees.
- Show typing feedback on Discord, WhatsApp, Signal, and supported Slack threads.
- Isolate Git test fixtures from inherited hook and worktree settings so pre-commit
  checks cannot overwrite the contributor's branch, index, or Git configuration.
- Preserve supported rich formatting in channels, including streamed edits and scheduled messages.
- Research user-supplied URLs with live Codex search and distinguish retrieval failures from missing pages.
- Give agents file, Git, shell, browser, and desktop tools with access controls.
- Connect agents and tools through ACP, A2A, AG-UI, and MCP.
- Save memories and organize data across conversations.
- Run scheduled tasks once and deliver results independently to multiple chats.
- Send images to supported models and transcribe voice locally.
- Add loops for speeding up development and improving robustness.
- Make contribloop recover from stale verification, commit-hook failures, and invalid PR metadata until checks pass.
- Poll pending contribution CI every five minutes by default, configurable with `--ci-interval`.
- Infer concise, single-word contribution branch names from the changes.
