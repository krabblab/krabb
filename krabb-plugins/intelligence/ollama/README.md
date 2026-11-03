# Ollama

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- ollama
```

The plugin supports local or cloud Ollama chat responses, including native newline-delimited JSON
streams normalized into Krabb protocol events. Set `KRABB_OLLAMA_HOST` for a non-default server and
`KRABB_OLLAMA_API_KEY` for cloud access, or use the `ollama` operating-system keyring
entry with `KRABB_KEYRING=1`.

The plugin advertises image-request support during its handshake and sends
confined images as bounded base64 data in Ollama chat requests. The selected
local or cloud model must support vision.

Run `krabb ollama setup` to store credentials with hidden input and encrypted
storage. Environment values override stored credentials; see
[authentication](../../../krabb-docs/authentication.md#plugin-setup).

Structured tool calls and their results retain the provider's native format
across turns. Interrupted streams without the provider's completion marker
fail the generation; partial tool calls are not returned for execution.
Local use requires a running Ollama server and an installed model; an API key
is optional. Remote requests carrying an API key require HTTPS. An unauthenticated
HTTP server remains supported on a trusted network. Hosts cannot contain embedded
credentials, a query, or a fragment.

See [deployment validation](../../../krabb-docs/providers.md#deployment-validation)
before relying on this adapter in production.
