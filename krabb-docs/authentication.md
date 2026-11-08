# Authentication

Krabb has two separate authentication decisions: whether a plugin can reach
its provider and whether a channel sender can use a capability.

## Table Of Contents

- [Provider Credentials](#provider-credentials)
- [Plugin Setup](#plugin-setup)
- [Codex Compatibility](#codex-compatibility)
- [Channel Trust](#channel-trust)
- [Service Credentials](#service-credentials)

## Provider Credentials

Use `krabb <id> setup` for persistent credentials. Nonempty plugin environment
variables, such as `KRABB_TELEGRAM_TOKEN`, always override stored values.

Krabb encrypts credentials in `credentials.json` under its home directory
(usually `~/.config/krabb`). A new store gets a cryptographically random 256-bit
key. By default, the OS keyring holds that key when available. Otherwise Krabb
creates a separate, protected `credentials.key` file. No passphrase needs to be
invented, typed, or saved in shell history.

| Setting | Effect |
| --- | --- |
| `KRABB_CREDENTIALS` | Choose a different encrypted credentials file. |
| `KRABB_CREDENTIALS_KEY` | Choose a different fallback key file; default: `credentials.key` in Krabb's home. |
| `KRABB_KEYRING=auto` (default) | Prefer the keyring for a new store; otherwise use the protected key file. |
| `KRABB_KEYRING=1` | Require keyring storage during setup or service credential capture. |
| `KRABB_KEYRING=0` | Use a protected key file during setup or service credential capture. |

An existing store remembers where its key lives. Temporary keyring failures
do not silently switch credentials. Running setup with an explicit `0` or `1`
can move a readable store to that key backend. The daemon must be able to access
the same keyring or key file. On a headless server without a usable keyring,
the encrypted file works without an interactive unlock after a restart.

Keep both files private and outside version control. Back up the fallback key
securely: losing it makes the encrypted credentials unrecoverable. Moving only
the credentials file does not move its key. Encryption protects a copied
credentials file, but someone who obtains both files—or controls the running
account—can still recover the secrets. Place the key separately when useful;
Krabb does not claim protection against a complete host compromise.

Legacy protected plaintext JSON files remain readable for compatibility. The
next setup migrates their contents to encryption and preserves unrelated keys.
Old key files and keyring entries are retained for other stores and backups.
Credential values are never written to manifests, logs, transcripts, or plugin locks.

## Plugin Setup

Use `setup` for a token or API key you already have. Use the plugin's own command
for an interactive account sign-in:

| Configure a channel | `krabb <id> setup` | Follow the [channel guide](providers.md#platform-setup) for required values and external setup. |
| Enter a Telegram bot token | `krabb telegram setup` | Krabb prompts and saves it securely. |
| Enter an Anthropic API key | `krabb anthropic setup` | The same secure setup workflow. |
| Enter a Codex API key | `krabb codex setup` | Krabb stores the API key. |
| Sign in to a personal Codex account | `krabb codex login` | The plugin runs its account sign-in flow. |
the plugin's declared secrets and settings; neither invokes custom login flows.
Plugins without declared secrets or settings do not gain a setup shortcut. See
[the plugin author guide](plugins.md#credentials-and-sign-in) when creating a plugin.

Setup works before the plugin is active. It prompts for every declared secret,
including optional ones, followed by declared settings, without echoing input.
Press Enter at a secret prompt to leave it empty and clear any previously saved
value. Empty secrets are omitted from the plugin environment; plugins that require
authentication still need valid credentials to operate. Environment overrides
continue to take precedence and must be removed separately.
Settings can declare a visible default; press Enter to accept it. For example,
WhatsApp setup also asks for the phone number ID, versioned Graph API URL, and
listener address (default `127.0.0.1:8787`). Ollama setup asks for the API key and
host (default `https://ollama.com`; use `http://127.0.0.1:11434` for a local server).
It then asks for thinking mode (default `false`) and context length (default `auto`).
See [Ollama controls](../krabb-plugins/intelligence/ollama/README.md#thinking-and-context)
for supported values and a local smoke-test configuration.
These settings are saved in the same encrypted store and supplied when the daemon
starts or reloads the plugin, without service-file changes. A terminal is required,
even with `--json`; tokens
cannot be arguments or piped input. Escape or Ctrl-C cancels without saving.
Multiline and oversized values are rejected.
Prompt labels show the declared environment variable names and public manifest
defaults, never stored values. An empty setting is rejected when no default exists.

OpenAI and Codex setup also accept a context budget: `auto` uses the documented
model-family catalog, or enter a token count greater than one for your deployment.
See [shared context preparation](providers.md#context).

Setup replaces the entered secrets and settings and preserves others using
locked, crash-safe encrypted writes. Its output identifies whether the encryption key is in the
keyring or a file. Unusable files, missing keys, and decryption errors are rejected.

Setup then asks the running daemon to load or reload the plugin. Use
`--no-activate` to save only. If activation stays pending, start or restart the
runtime with the same credential file and access to its key, no overriding setup
environment variables. Additional channels and providers activate alongside existing
ones. Older daemons need a restart. Run
`krabb doctor` for readiness checks. JSON results report `saved` and `active`
separately; a successful save does not guarantee activation.

Run setup as the same OS user as the service, using the same `KRABB_HOME`,
`KRABB_CREDENTIALS`, and `KRABB_CREDENTIALS_KEY` when customized. Nonempty environment
values override saved values; remove stale service overrides if you want setup to
control those settings. For an already running WhatsApp listener, use
`krabb whatsapp setup --no-activate` followed by `krabb service restart` so the old
process releases its port before the replacement starts.

Follow the [channel guide](providers.md#platform-setup) for external account and
app configuration. Setup does not validate credentials with the platform;
activation starts normal operation. Configure a working model and review
[channel policy](configuration.md#channel-policy) before testing messages.

## Codex Compatibility

For personal ChatGPT sign-in, the Codex plugin invokes the installed Codex
CLI's app-server and delegates authentication and token refresh to it. Install
a compatible Codex CLI using the [official Codex CLI setup guide](https://developers.codex.com/codex/cli/),
then run:

```sh
krabb codex --help
krabb codex login
```

For a headless host, use `krabb codex login --device`. Check or clear the
Codex-owned sign-in with `krabb codex status` or `krabb codex logout`.
Use `krabb codex login --help` for login options and `krabb codex models` to
see model IDs available to the signed-in account.
The default Codex home is the user's standard `.codex` directory; set
`KRABB_CODEX_HOME` to use another profile. Krabb does not read, persist, or
pass Codex OAuth tokens to the Codex HTTP client.

Krabb does not pin a Codex CLI release. It checks that the CLI starts and
validates each required app-server response. If a Codex update changes the
protocol or a required operation, Krabb fails closed with an error. Codex
CLI is required only for personal Codex sign-in; OpenAI API-key authentication
and other providers do not require it. Use an OpenAI API key for unattended
shared deployments.

## Channel Trust

Direct messages are accepted by default, but tools still require a trusted
policy and daemon approval. Group chats must appear in the channel `allow`
list. Optional owner, admin, member, topic, thread, and mention filters narrow
that allowlist. Configure `tools = true` only for senders who may access the
workspace.

## Service Credentials

`krabb service install` materializes declared provider variables into an
encrypted service credential file using the same keyring preference as setup.
The service definition records the credential path and custom fallback key path.
Use `KRABB_KEYRING=0` when the service account cannot access a keyring. Inspect the
generated definition with `krabb service status` and
uninstall it with `krabb service uninstall -y` when rotating credentials.
Service installation also recognizes an existing default credentials file.
For an already installed service with its own credential file, point
`KRABB_CREDENTIALS` at that file when running setup; changing another file will
not rotate the service's credentials. Remove overriding secret variables from
the service environment and restart it when needed.
For a custom fallback key location, set `KRABB_CREDENTIALS_KEY` during installation
and setup. A service running under a different account may not be able to access
your login keyring; configure its credential store under the service account or
use the encrypted-file backend. Do not assume a desktop login unlocks a system service.
