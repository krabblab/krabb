This optional plugin uses the WhatsApp Cloud API. Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/krabblab/krabb/main/install.sh | sh -s -- whatsapp
```

## Setup

1. Follow [Meta's Cloud API setup guide](https://developers.facebook.com/documentation/business-messaging/whatsapp/get-started)
   to create the app and test number, obtain credentials, and verify your test recipient.
2. Run `krabb whatsapp setup` and enter the values below.
3. Complete Meta's webhook setup: forward a public HTTPS callback to Krabb's
   listener, use the same verification token, and subscribe to `messages`.
   Also [subscribe your app to the WhatsApp Business Account (WABA)](https://www.postman.com/meta/whatsapp-business-platform/request/ju40fld/subscribe-app-to-waba-s-webhooks),
   including when using a test number. Verify the subscription with
   [Meta's subscription list](https://www.postman.com/meta/whatsapp-business-platform/request/1950zr8/get-all-subscriptions-for-a-waba).
   Then send the test number a message from your verified recipient and check the reply.

| Setup prompt | Value |
| --- | --- |
| `KRABB_WHATSAPP_TOKEN` | Access token for your Meta app. Temporary test tokens expire. |
| `KRABB_WHATSAPP_APP_SECRET` | Your Meta app secret. |
| `KRABB_WHATSAPP_VERIFY` | A verification string you choose and also enter in Meta. |
| `KRABB_WHATSAPP_PHONE` | **Phone Number ID**, not the display telephone number or WABA ID. |
| `KRABB_WHATSAPP_GRAPH_URL` | Versioned Graph API base from Meta's request example. |
| `KRABB_WHATSAPP_LISTEN` | Press Enter for `127.0.0.1:8787`. |

For a temporary HTTPS address, see [Cloudflare Quick Tunnels](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/).
For ongoing use, keep a stable public callback URL. A plain browser visit or
`curl` to the callback returns 400 without Meta's verification parameters;
that alone does not mean the listener is broken.

If WhatsApp is already running, save the values and restart to release its listener:

```sh
krabb whatsapp setup --no-activate
krabb service restart
```

Update the CLI, daemon, and WhatsApp plugin together to get all six setup prompts.
See [shared setup](../../../krabb-docs/authentication.md#plugin-setup) for encrypted
storage, environment overrides, and matching the service's account and credential paths.
Setup saves these values without service-file edits; existing environment overrides
still take precedence. Other channels, including Telegram, can remain configured.

## Messages And Media

Inbound text, images, voice messages, and text files are normalized. The plugin
can send all four content types, while the current daemon response path sends
generated replies as text. Verified webhook events are stored under
Media storage reserves space before writing: 64 MiB, 256 files, and a 24-hour
cache lifetime. Failed cleanup rejects the download instead of silently
exceeding the limits; protected session images are preserved.

## Formatting

The handshake advertises `formats: ["markdown"]`. Requests with
`format: "markdown"` are rendered as WhatsApp markup for bold, italic, strikethrough, monospace and quotes; link targets remain visible.
Requests without this field keep their existing text semantics. Unsupported
Markdown structures remain readable text. The runtime preserves source Markdown
for scheduled deliveries and retries.
