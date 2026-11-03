# Discord

Install the released plugin with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- discord
```

Set `KRABB_DISCORD_TOKEN`, or set `KRABB_KEYRING=1` to
use the `discord` operating-system keyring entry. The plugin receives
`MESSAGE_CREATE` events through the Gateway, answers heartbeats, ignores bot
authors, normalizes text and attachments, and sends replies through the REST
API. Gateway session state is persisted under `KRABB_HOME` and advances only
after the host acknowledges an accepted event. Outgoing text is split at
Discord's message limit; `edit` updates the first existing message and sends
any remaining chunks to the same channel or thread.
`KRABB_DISCORD_GATEWAY_URL` and `KRABB_DISCORD_INTENTS` are optional
overrides for local integration tests.

Image, voice-message audio, and bounded text-file attachments are downloaded
through opaque references into the private `KRABB_MEDIA` cache. The plugin
expires files after 24 hours and evicts oldest files before storage to keep the
cache within 64 MiB and 256 files. Cleanup failures reject storage; the protected
session image directory is preserved. The plugin
supports attachment uploads, but the current daemon sends generated replies as
text. Direct messages work by default; guild channels and threads require the
daemon channel allowlist.

Use `krabb discord setup` for hidden credential entry and encrypted storage.
Setup saves declared secrets; it does not register accounts, grant external app
permissions, or configure channel authorization. Environment values override
stored credentials. See [authentication](../../../krabb-docs/authentication.md#plugin-setup)
and [deployment validation](../../../krabb-docs/providers.md#deployment-validation).

A missing heartbeat acknowledgement closes the Gateway connection so the next
poll reconnects. A nonresumable session rejection clears the old session before
identifying again. Gateway connection establishment has a ten-second deadline.
