# OpenRouter

This provider is optional and is distributed separately from the
core release package.

Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- openrouter
```

Set `KRABB_OPENROUTER_KEY` and select a model with
`KRABB_OPENROUTER_MODEL` or the normal session model setting. The adapter
uses OpenRouter's OpenAI-compatible chat completions API, including bounded
streaming, images, and tool calls. The selected model must support the input
features requested by the turn.

`KRABB_OPENROUTER_BASE_URL` is available for HTTPS-compatible gateways and
loopback test fixtures. `KRABB_OPENROUTER_REFERER` and
`KRABB_OPENROUTER_TITLE` optionally provide OpenRouter attribution headers.

Run `krabb openrouter setup` to store credentials with hidden input and encrypted
storage. Environment values override stored credentials; see
[authentication](../../../krabb-docs/authentication.md#plugin-setup).

Structured tool calls and their results retain the provider's native format
across turns. Interrupted streams without the provider's completion marker
fail the generation; partial tool calls are not returned for execution.
Base URLs must use HTTPS outside loopback and cannot contain user credentials,
a query, or a fragment. The selected model must be available to your account.

See [deployment validation](../../../krabb-docs/providers.md#deployment-validation)
before relying on this adapter in production.


Implicit selections (`default`, legacy `unset`, or blank) use `deepseek/deepseek-v4.1-flash`.
Explicit model names take precedence. See [default models](../../../krabb-docs/providers.md#default-models)
for the verification date, catalog sources, and usage limits.

Model and endpoint metadata supply the shared host context budget. Older turns
are summarized before any final truncation. Native search is enabled only on
endpoints advertising it, with routing fallback disabled. The account-level web
plugin is explicitly disabled so unsupported models do not invoke another search
service. Replies retain source links; summary turns disable search and tools.
See [context and search](../../../krabb-docs/providers.md#context) for limits.
