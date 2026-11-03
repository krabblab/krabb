# Anthropic

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- anthropic
```

The plugin supports Anthropic Messages responses, including Server-Sent Event
streams. Set `KRABB_ANTHROPIC_KEY`, or use the
`anthropic` operating-system keyring entry with `KRABB_KEYRING=1`.

Streaming generations finish at `message_stop`, preserving the completed text
and tool calls without waiting for the transport to close.

The plugin advertises image-request support during its handshake and sends
confined images as bounded base64 or HTTPS image blocks. The selected Claude
model must support vision.

Run `krabb anthropic setup` to store credentials with hidden input and encrypted
storage. Environment values override stored credentials; see
[authentication](../../../krabb-docs/authentication.md#plugin-setup).

Structured tool calls and their results retain the provider's native format
across turns. Interrupted streams without the provider's completion marker
fail the generation; partial tool calls are not returned for execution.
Base URLs must use HTTPS outside loopback and cannot contain user credentials,
a query, or a fragment. Select a model available to your account explicitly.

See [deployment validation](../../../krabb-docs/providers.md#deployment-validation)
before relying on this adapter in production.
