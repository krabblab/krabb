# OpenAI

This provider is optional and is distributed separately from the
core release package.

Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- openai
```

Set `KRABB_OPENAI_KEY` and select a model with the normal session model
setting. Set `KRABB_OPENAI_MODEL` to override the built-in default.
The plugin uses Responses for native web search and retains Chat Completions for
other compatible models. Text, images, streaming, and function calls are normalized
for the host. The selected model and endpoint must support the requested features.

Credentials can also come from the protected `KRABB_CREDENTIALS` JSON file,
or the `openai` operating-system keyring entry when `KRABB_KEYRING=1`.
`KRABB_OPENAI_BASE_URL` supports HTTPS-compatible gateways and loopback test
fixtures.

Run `krabb openai setup` to store credentials with hidden input and encrypted
storage. Environment values override stored credentials; see
[authentication](../../../krabb-docs/authentication.md#plugin-setup).

Structured tool calls and their results retain the provider's native format
across turns. Interrupted streams without the provider's completion marker
fail the generation; partial tool calls are not returned for execution.
Base URLs must use HTTPS outside loopback and cannot contain user credentials,
a query, or a fragment. The selected model must be available to your account.

See [deployment validation](../../../krabb-docs/providers.md#deployment-validation)
before relying on this adapter in production.


Implicit selections (`default`, legacy `unset`, or blank) use `gpt-6-luna`.
Explicit model names take precedence. See [default models](../../../krabb-docs/providers.md#default-models)
for the verification date, catalog sources, and usage limits.

Context preparation is shared with every intelligence provider: summarize older
turns, then truncate only if needed. `krabb openai setup` accepts
`KRABB_OPENAI_CONTEXT_LENGTH` (`auto` or a token count greater than one).
Native web search is enabled for supported model families, with clickable source
links. See [context and search](../../../krabb-docs/providers.md#context) for
catalog limits, compatible endpoints, and account requirements.
