# Resources

A minimal Krabbling needs only the daemon, one intelligence plugin, and one
channel or client. With remote inference, a whole minimal setup can use roughly
**50–70 MiB of RAM**, with near-zero CPU use while idle.

## Basic Footprint

| Component                            |   Approximate RSS |
| ------------------------------------ | ----------------: |
| Daemon                               |            21 MiB |
| One Intelligence Plugin (e.g. Codex) |         22–25 MiB |
| One Channel (e.g. Telegram)          |         24–25 MiB |
| **Minimal Setup Estimate**           | **Around 70 MiB** |

Lighter provider/client combinations can be closer to 50 MiB; for example, the
daemon with OpenRouter and TUI totals approximately 56 MiB.

Memory, scheduling, tools, additional providers, and additional channels are
optional. Install only the capabilities you need.

These are approximate Linux aarch64 process-memory figures, not whole-machine
RAM requirements or a guaranteed ceiling. Usage depends on configuration,
conversation history, and workload. Leave room for the operating system and other
services. CPU use rises while processing messages and running tools; local model
inference and any additional provider subprocesses need resources beyond these
figures.
