# TUI

The TUI is a terminal client for daemon-managed conversations, sessions, tools,
and approvals. Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- tui
```

## Table Of Contents

- [Start And Chat](#start-and-chat)
- [Commands](#commands)
- [Tools And Safety](#tools-and-safety)
- [Preferences And History](#preferences-and-history)

## Start And Chat

Start the daemon with `krabb service start` or run `krabb-daemon` in the
foreground. Then open the client:

```sh
krabb tui
krabb tui --once "prompt"
```

`--once` prints one reply without opening the full-screen UI. It uses and
deletes an idle temporary session unless you pass `--session <id>`. Saved work
or pending results retain it. Unconfirmed cleanup is reported with a reopen command. Interactive
sessions resume the last session or start `default`.

Ask the model in any language to pause, resume, cancel, delay, or extend a saved
task. While inference is busy, use exact commands: `/task` lists tasks, and
`/task pause|resume|cancel|approve|deny <id>` controls one. Use
`/task defer <id> 2030-11-12T09:00 Europe/Rome` to save a one-off resume time
without the scheduler. Use the conversation where the task began. Existing
visibility can share context without allowing another conversation to control
that task.
After a lost daemon response, the TUI looks up the saved request instead of
generating it again. Reopening a conversation checks up to eight recent TUI user
messages for the latest accepted request, skipping control-only messages. Other
retained tasks remain available through `/task`.

Press Enter to send, Ctrl+O to add a line, and Escape or Ctrl+C to quit.
Shift+Enter and Alt+Enter also add a line where the terminal reports them, so
Ctrl+O is the key that works everywhere. Use Up/Down for input history,
Page Up/Page Down to scroll, and Shift-drag to select text. Typed text, including
capitals, shifted symbols, and Option or AltGr characters, arrives as the
keyboard layout produces it.

When the TUI closes for any reason other than quitting, such as lost terminal
input or an error, it prints why and how to open it again.
When the daemon restarts, for example after `krabb config set`, the TUI reconnects
on its own for about half a minute and clears approval prompts the restart dropped.
If it cannot reconnect, it says so once and asks you to reopen it with `krabb tui`.
Notices Krabb writes itself, such as retries, failures, and approval results, appear
as System rather than as your Krabb.

Attach images, PDFs, text or code files, and audio with `/attach <path>` or by
pasting or dragging their absolute paths into the prompt, or attach a clipboard
image with Ctrl+V (Alt+V where the terminal keeps Ctrl+V).
Attachments appear in the prompt title until you send them; Backspace on an
empty prompt removes the last one. Send them once Krabb's current turn ends. See
[Media](../../../krabb-docs/media.md) for what Krabb reads.

Assistant replies display bold, italic, strikethrough, headings, and inline code
with terminal text styles. Code blocks appear in language-labeled panels, and
links show their labels and URLs. Formatting stays readable while replies stream
and when revisiting saved sessions. Terminal support determines how styles look;
disabling colors preserves emphasis. Input and system command output remain literal.

## Commands

Use `/help` for commands available in the current session. Common commands:

- `/status` shows the same status as `krabb status`, and `/plugin` lists plugins
  as `krabb plugin list` does.
- `/approval`, `/approvals`, `/approve <id>`, and `/deny <id>` manage pending
  approvals, including while a turn waits for one.
- `/session`, `/new`, `/model`, and `/workspace` manage conversation context.
- `/deliveries`, `/retry`, and `/drop` inspect delivery state.
- `/schedule`, `/memory`, and `/data` are available when their plugins are
  installed. Use `/data value` for simple session values; `/store` remains a
  compatibility alias.
- `/compact` summarizes older turns; `/clear` clears an idle session.

The TUI stores its command exchanges and completed model turns in the shared
session history. See [Commands](../../../krabb-docs/commands.md) for CLI
operations and plugin guides for detailed schedule and memory behavior.

## Tools And Safety

Live approvals and `/approvals` display complete tool arguments. The host rejects
prompt-mode requests that exceed its review limits before issuing a challenge.

TUI model tools are enabled by default, and `krabb init` installs the `tools` plugin with
the TUI. Turn them off in `config.toml`:

```toml
[clients.tui]
tools = false
```

The global approval mode still controls mutating tools. To enable direct shell
commands entered with `!`, also set `shell = true`; restart the daemon after
changing that setting. A typed `!` command is an explicit user action, while
shell commands requested by the model remain subject to approval policy.
Workspace tools stay confined to the active session workspace.

## Preferences And History

Use `/history list`, `/history clear`, or `/history help` to manage input
history. `/compact` asks the selected model to summarize older turns; the TUI
also compacts automatically when saved conversation history reaches its limit.

Set `theme = false` under `[clients.tui]` to use the terminal's default colors.
Use `/statusline` to choose which details appear in the footer. Preferences are
saved under `KRABB_HOME/data/plugins/tui/` and apply to the next launch where
noted.


The TUI uses `--plugin` / `--model` first, then `KRABB_MODEL_PLUGIN` /
`KRABB_MODEL`, then `[clients.tui]` and global `provider` / `model` settings in
`config.toml`. With no configured selection it uses `codex` and `default`, resolved
by the intelligence plugin to its small model. Existing sessions retain their
model; `--model <id>` or `/model set <id>` changes it. Sessions saved as `unset`
can also generate using the provider default. See
[selection precedence](../../../krabb-docs/configuration.md#provider-and-model-precedence).

Use `/reply last <message>` to quote the last assistant answer, or
`/reply <message-id> <message>` for a specific message in the active session.
`krabb session show <id> --json` exposes message IDs. The selected text is sent as
bounded reference context alongside the new message. Replying to a system notice
requires its exact ID; `/reply last` skips system notices.
