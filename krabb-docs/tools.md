# Tools

These are the tools a model can call in Krabb, grouped by where they come from. Each name
is `radix_action` in snake case: the radix names the area or plugin, and the action is a
verb. A tool appears only when its source is installed and the conversation's policy allows
it.

## Host

Krabb itself owns these tools, and plugins cannot register their names.

| Tool | Purpose |
| --- | --- |
| `task_classify` | Classify a request as fast, thoughtful, or background, and start a background task. |
| `task_list` | List the tasks in this conversation. |
| `task_pause` | Pause a task after its active step settles. |
| `task_resume` | Resume a paused or blocked task. |
| `task_cancel` | Cancel a task and keep its completed effects. |
| `task_steer` | Replace a task's instruction. |
| `task_defer` | Pause a task until an exact time. |
| `task_extend` | Add minutes to a task's inference allowance. |
| `task_checkpoint` | Save a background task's progress and end its step. |
| `task_read` | Read part of a task's saved original request when it was too long to include. |
| `conversation_command` | Run a conversation command the person asked for in their own words. |
| `settings_change` | Change tools, shell, or approval in this conversation's settings section. |
| `message_send` | Send a message to the schedule's authorized destination during a schedule run. |

## Tools Plugin

| Tool | Purpose |
| --- | --- |
| `file_read` | Read a text file in the workspace. |
| `file_write` | Write a text file in the workspace, with approval. |
| `file_list` | List workspace entries. |
| `file_search` | Search workspace text. |
| `file_patch` | Apply a Git patch, with approval and shell access. |
| `web_fetch` | Fetch bounded text from a public HTTPS URL. |
| `git_run` | Run an approved Git operation, with shell access. |
| `shell_run` | Run an approved shell command, with shell access. |

## Memory

| Tool | Purpose |
| --- | --- |
| `memory_search` | Search memories visible in this conversation. |
| `memory_remember` | Save a memory in this conversation's scope. |

## Scheduler

| Tool | Purpose |
| --- | --- |
| `schedule_propose` | Schedule a one-time or recurring task the person asked for. |
| `schedule_list` | List schedules with their status, from local administrators only. |
| `schedule_update` | Replace a schedule's destinations. |
| `schedule_pause` | Pause an active schedule. |
| `schedule_resume` | Resume a paused schedule. |
| `schedule_cancel` | Cancel a schedule's future runs. |
| `schedule_run` | Run a schedule now, or rerun a finished one-time task. |
| `schedule_retry` | Retry a task paused after failure or skipped while offline. |

## SQLite

| Tool | Purpose |
| --- | --- |
| `data_query` | Create, explore, and change structured databases, tables, and records. |

## Experimental

These plugins are outside the supported v0.1 scope.

### Browser

| Tool | Purpose |
| --- | --- |
| `browser_open` | Start an isolated browser session. |
| `browser_close` | Close an owned browser session. |
| `browser_navigate` | Navigate an owned tab to an allowed URL. |
| `browser_tabs` | List tabs in an owned session. |
| `browser_new_tab` | Create a tab in an owned session. |
| `browser_close_tab` | Close a tab in an owned session. |
| `browser_snapshot` | Return visible text and interactive controls. |
| `browser_screenshot` | Capture a viewport, page, or element image. |
| `browser_click` | Click a locator or a point from the latest screenshot. |
| `browser_fill` | Fill an editable locator. |
| `browser_select` | Select an option from a locator. |
| `browser_press` | Press a key or key combination. |
| `browser_scroll` | Scroll an owned tab. |
| `browser_wait` | Wait for a locator. |

### Computer

| Tool | Purpose |
| --- | --- |
| `computer_open` | Claim the configured display for this session. |
| `computer_close` | Release the owned display. |
| `computer_displays` | Read the owned display's dimensions. |
| `computer_windows` | List visible window titles and bounds. |
| `computer_screenshot` | Capture the owned display as an image. |
| `computer_move` | Move the pointer. |
| `computer_click` | Click a mouse button. |
| `computer_double_click` | Double click. |
| `computer_drag` | Drag between two points. |
| `computer_scroll` | Scroll by display steps. |
| `computer_keypress` | Press a key or key combination. |
| `computer_type` | Type text. |

### Runner

| Tool | Purpose |
| --- | --- |
| `runner_delegate` | Start an external A2A agent task that the host monitors and resumes. |

Runner offers each declared operation as `runner_<operation>`.

### MCP

The MCP plugin bridges requests to MCP servers and offers no model tools of its own.
