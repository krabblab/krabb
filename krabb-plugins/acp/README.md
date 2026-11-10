# ACP

> **Experimental:** This plugin is not fully tested and is outside the supported
> v0.1 scope. See the [functional test matrix](../../FUNCTIONAL.md) for the
> components included in v0.1.

ACP connects Krabb to an external process that implements the stable Agent
Client Protocol v1 over newline-delimited JSON-RPC on stdin and stdout.

Configure `KRABB_ACP_COMMAND` with the executable path. Optional arguments
must be a JSON string array in `KRABB_ACP_ARGS`; the plugin never invokes a
shell. Install the plugin, then run `krabb acp --workspace PATH PROMPT`. Add
`--session ID` to resume a persisted session when the external agent advertises
ACP `loadSession` support.

The adapter initializes ACP, creates or resumes sessions, streams agent text
and tool status updates, and returns the agent's session ID. With `--json`,
stream updates go to stderr and the final JSON result includes the complete
agent answer in `text`. It advertises ACP
text-file read and write methods and routes them through Krabb's confined tools
and approval policy. It does not advertise terminal methods. ACP permission
requests are rejected because Krabb does not expose a generic agent permission
callback. Agents execute their own tools and retain their own session
semantics. External agents must implement ACP v1. Responses must end with a newline
and fit within the 4 MiB frame limit, including that newline. The read is bounded even
when an agent never terminates its output line.

Add `--quote "Earlier message"` to a message request to include bounded reference
context alongside the prompt. The adapter sends ordinary protocol text; it does
not require the external agent to support a Krabb-specific message type.
