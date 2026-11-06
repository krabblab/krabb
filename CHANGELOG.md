# Changelog

All notable Krabb changes are documented here using Keep a Changelog and
Semantic Versioning.

## [0.1.0]

- Isolate Git test fixtures from inherited hook and worktree settings so pre-commit
  checks cannot overwrite the contributor's branch, index, or Git configuration.
- Preserve supported rich formatting in channels, including streamed edits and scheduled messages.
- Research user-supplied URLs with live Codex search and distinguish retrieval failures from missing pages.
- Give agents file, Git, shell, browser, and desktop tools with access controls.
- Connect agents and tools through ACP, A2A, AG-UI, and MCP.
- Save memories and organize data across conversations.
- Schedule agent tasks and send their results to chat channels.
- Send images to supported models and transcribe voice locally.
- Add loops for speeding up development and improving robustness.
- Make contribloop recover from stale verification, commit-hook failures, and invalid PR metadata until checks pass.
- Poll pending contribution CI every five minutes by default, configurable with `--ci-interval`.
- Infer concise, single-word contribution branch names from the changes.
