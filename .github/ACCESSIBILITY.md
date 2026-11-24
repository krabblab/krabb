# Accessibility

Krabb should be usable by everyone who runs it, including people who use screen
readers, magnification, high contrast, keyboard-only input, or other assistive
technology. This statement describes what works today, what does not yet, and how
to tell us about a barrier.

Krabb has not had a formal accessibility evaluation, so it claims no conformance
with a standard such as WCAG.

## Goals

- Every task works from the keyboard and from plain text, without relying on
  color, animation, or layout alone.
- Output stays readable with `NO_COLOR`, monochrome terminals, narrow windows,
  and redirected output.
- Documentation uses plain language, descriptive links, and text alternatives
  for images.

## Supported Environments

- **Command line:** `krabb` runs in any terminal on Linux, macOS, and Windows.
  `--json` gives structured output for tools and scripts.
- **Terminal interface:** `krabb tui` is keyboard-driven. `krabb tui --once
  "prompt"` prints one reply as plain text without the full-screen interface.
- **Chat channels:** Telegram, WhatsApp, Discord, Slack, and Signal let you use
  Krabb from the apps and assistive technology you already have.

## What Works Today

- `NO_COLOR` turns off color in the command line and the terminal interface.
  Set `theme = false` under `[clients.tui]` to use your terminal's own colors.
- `/animation off` in the terminal interface shows replies at once instead of
  typing them out.
- Errors name the problem and the next step in text, not only in color.

## Known Limitations

- The full-screen terminal interface has not been tested with screen readers,
  and redrawn regions may be announced poorly. Use `krabb tui --once`, the
  command line, or a chat channel instead.
- Help output starts with a decorative text banner, and some lists use box-drawing
  characters, which screen readers may read aloud.
- Media replies such as voice are not available yet, so replies are text only.

Planned work is tracked in issues [#147](https://github.com/krabblab/krabb/issues/147)
and [#201](https://github.com/krabblab/krabb/issues/201).

## Report a Barrier

[Open an accessibility issue](https://github.com/krabblab/krabb/issues/new?template=accessibility.yml).
Describe what you were trying to do and what got in the way. You do not need to
share any disability, screenshots, or recordings.

The maintainer aims to acknowledge reports within 14 days, on a best-effort basis,
and prioritizes barriers that block core use: installing, setting up, chatting,
and recovering.

## Contributing

When you change the command line, the terminal interface, or documentation, keep
it usable without color or animation, operable from the keyboard, and readable
as plain text. Give images a text alternative. These expectations apply equally
to changes prepared with coding agents.

## Keeping This Current

The maintainer reviews this statement with each release. To suggest a change,
open an issue or a pull request.
