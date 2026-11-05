# Gemini

Gemini is an optional intelligence plugin for the Google Gemini Developer API.
Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- gemini
```

Set `KRABB_GEMINI_KEY` and optionally select a model with `KRABB_MODEL` or the
session model setting. Blank, `default`, and legacy `unset` requests use
`gemini-3.5-flash-lite`. The historical host fallback `gpt-6-luna` is still treated
as an implicit selection for compatibility. Explicit Gemini model names are preserved.

Gemini 3 tool calls retain thought signatures across turns. Old transcripts missing
those signatures are rendered as text history without replaying tool calls or selecting
an older model. Explicit Gemini 2.x models retain legacy marker transcript compatibility.
The default API endpoint is HTTPS; `KRABB_GEMINI_BASE_URL` permits a loopback test
fixture. See [default models](../../../krabb-docs/providers.md#default-models).

Run `krabb gemini setup` to store credentials with hidden input and encrypted
storage. Environment values override stored credentials; see
[authentication](../../../krabb-docs/authentication.md#plugin-setup).

Streaming uses `streamGenerateContent` with bounded SSE parsing and coalesced
text events. The final reply retains text, function-call IDs, thought signatures,
and token usage. Thought-only text is excluded from the visible reply. Blocked,
empty, malformed, and prematurely closed responses fail explicitly; truncated
tool calls are never dispatched. API keys travel in the `x-goog-api-key` header,
not the request URL. Base URLs cannot contain embedded credentials, a query,
or a fragment.

See [deployment validation](../../../krabb-docs/providers.md#deployment-validation)
before relying on this adapter in production.
