# AG-UI Plugin

> **Experimental:** This plugin is not fully tested and is outside the supported
> v0.1 scope. See the [functional test matrix](../../FUNCTIONAL.md) for the
> components included in v0.1.

The AG-UI plugin serves a local HTTP endpoint that accepts AG-UI run input and
streams events as chunked server-sent events. It calls the configured Krabb
model through the host. The listener binds to `127.0.0.1:8787` by default; set
`KRABB_AG_UI_LISTEN` to change it. Every POST requires `Authorization: Bearer <token>`. Set `KRABB_AG_UI_TOKEN`
to a cryptographically random token of at least 32 ASCII characters before
starting Krabb; the plugin refuses to start without it. Keep this token secret.
Browser access is disabled by default. Set `KRABB_AG_UI_ORIGINS` to a
comma-separated list of exact loopback origins, for example
`http://localhost:3000`. Other origins are rejected even with a valid token.
Preflight requests do not need a token; actual requests always do.
Incomplete request headers and bodies time out after 10 seconds, releasing
the connection slot. At most four requests are processed concurrently. Additional
requests receive HTTP 503; the server drains bounded input after sending the response
to avoid resetting the connection before the client can read it.

Run `krabb ag-ui`. The command stays active until interrupted and prints its
bound address. Send a JSON run input to `POST /` with `threadId`, `runId`, and
`messages`. Text and tool events are forwarded as the configured model streams
them. Client tools are returned with `pendingToolCallIds`; the frontend executes
those tools and sends their results in a later run. Client state and context are
passed as untrusted model context, and input state is emitted as `STATE_SNAPSHOT`.
Model failures produce `RUN_ERROR` events.

The plugin does not mutate client state or persist AG-UI thread sessions.
Disconnecting a run cancels its pending host request; the host restarts the
configured model process to clear its in-flight protocol stream. Human approval
for frontend tools remains the frontend's responsibility. The host still
selects the model and applies its configured policy.

Krabb supports an optional `reply` extension on user messages: an object with `id`,
optional `sender`, and optional `text`. Supplied quoted text is bounded to 4096 UTF-8
bytes; ID-only references resolve from earlier messages in the same run input.
Missing targets are marked unavailable. Standard AG-UI inputs without this extension
remain unchanged. A final answer that differs from its streamed prefix starts a new
assistant message, preserving both the earlier progress and the complete final text.
