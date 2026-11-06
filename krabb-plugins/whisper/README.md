# Whisper

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- whisper
```

Configure `KRABB_WHISPER_COMMAND` with a local Whisper-compatible executable.
The `transcribe` method accepts a path confined to `KRABB_MEDIA` (defaulting
to `KRABB_HOME/data/media`) and returns normalized text. After a successful
transcription, Krabb removes the raw voice file. No model or native runtime
is bundled into the core.

Conversation-local audio uses the optional `scoped` wrapper. The host supplies the
workspace; Whisper accepts only paths beneath that workspace's `attachments/`,
confined to `KRABB_HOME/worktrees`. Ordinary `transcribe` calls retain the existing
`KRABB_MEDIA` boundary. Retained conversation audio remains available after
transcription; transient installation-cache audio is removed after success.

The host uses `retained` for audio in the plain workspace, supplying `workspace`,
`owner`, and `path`. Whisper checks both the integration's attachment directory and
its file ownership before transcription. Unsupported calls never fall back to a
broader filesystem boundary.
