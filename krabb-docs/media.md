# Media

What Krabb can receive and send on each channel and client. Krabb normalizes
every input the same way, so a row reads the same across columns.

Legend: ✅ read by Krabb · 📝 skipped, and Krabb is told what was skipped and why
· — not offered by that platform.

## Input

| Input | Telegram | WhatsApp | Discord | Slack | Signal | TUI |
|---|---|---|---|---|---|---|
| Text | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Photo or image | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ Ctrl+V or file |
| Image sent as a file | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ file |
| GIF | ✅ first frame | 📝 sent as video | ✅ upload or GIF picker | ✅ upload or GIPHY | 📝 sent as video | ✅ file |
| Sticker | ✅ | ✅ | ✅ (Lottie 📝) | — | ✅ | — |
| Voice note | ✅ | ✅ | ✅ | ✅ | ✅ | — |
| Audio file | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ file |
| PDF | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ file |
| Text or code file | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ file |
| Video | 📝 | 📝 | 📝 | 📝 | 📝 | 📝 |
| Other files (office, archives) | 📝 | 📝 | 📝 | 📝 | 📝 | 📝 |

What each ✅ needs:

- **Images, GIFs, and stickers** need a model that accepts images. Animated
  images and WebP are sent as a PNG of their first frame, which every provider
  reads; Telegram MP4 GIFs and animated stickers use Telegram's preview frame.
- **Voice notes and audio files** need the [Whisper](../krabb-plugins/whisper/README.md)
  speech plugin; without it Krabb is told transcription is unavailable.
- **PDFs** contribute their text layer; scanned pages without text are reported.
  Text from a PDF or text file is cut off after 64 KiB.
- **Signal stickers** need `signal-cli` 0.13 or newer.
- **TUI attachments**: use `/attach <path>`, paste or drag an absolute file path
  into the prompt, or press Ctrl+V (Alt+V where the terminal keeps Ctrl+V) to
  attach a clipboard image. Clipboard images need a local
  display; over SSH, use `/attach` instead.

Each file may be up to 4 MiB. Images sent to the model are limited to 4 MiB
per turn. See [Plugins](../krabb-handbook/plugins.md) for how files are read and bounded.

## Output

Krabb replies with text on every channel and client.

| Output | Telegram | WhatsApp | Discord | Slack | Signal | TUI |
|---|---|---|---|---|---|---|
| Text | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Files and images | v0.3 | v0.3 | v0.3 | v0.3 | v0.3 | v0.3 |
| Voice replies | v0.3 | v0.3 | v0.3 | v0.3 | v0.3 | v0.3 |
| Generated images or video | v0.3 | v0.3 | v0.3 | v0.3 | v0.3 | v0.3 |

## Planned

Video, office documents, file and voice replies, and media generation are
planned for the [v0.3 | Media](../ROADMAP.md#v03--media) milestone.
