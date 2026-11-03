# Gemini

Gemini is an optional intelligence plugin for the Google Gemini Developer API.
Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- gemini
```

Set `KRABB_GEMINI_KEY` and select a model with `KRABB_MODEL` or the normal
session model setting. Requests without a model, or with model `default` or the
host's `unset` sentinel, use the supported `gemini-3.8-flash` model. The older
host fallback `gpt-6-luna` is treated the same way when Gemini is selected. Gemini 3 tool calls
preserve the provider's thought signatures across turns; legacy marker
transcripts using these defaults temporarily use `gemini-2.5-flash`, and explicitly selected Gemini
2.x models remain supported through the legacy marker compatibility layer. That
layer can be removed after Gemini 2.x support is retired. The default API
endpoint is HTTPS and can be replaced with `KRABB_GEMINI_BASE_URL` for a
loopback test fixture.

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
