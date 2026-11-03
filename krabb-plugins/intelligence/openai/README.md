# OpenAI

This provider is optional and is distributed separately from the
core release package.

Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- openai
```

Set `KRABB_OPENAI_KEY` and select a model with the normal session model
setting. Set `KRABB_OPENAI_MODEL` when the request uses the `default` model.
The plugin uses OpenAI's Chat Completions API for text, image input, streaming,
and function calls. The selected model must support the requested features.

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
a query, or a fragment. Select a model available to your account explicitly.

See [deployment validation](../../../krabb-docs/providers.md#deployment-validation)
before relying on this adapter in production.
