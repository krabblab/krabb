# Design

How Krabb and Krabblab look. Follow these rules for any design work in this repository,
whether by a person or an agent. This file will grow into the design system; today it
covers the brand.

## Logo

- The mark is a hooked K: a straight stem with arms whose tips bend inward like a claw.
- One mark serves both names, and the wordmark says which: `krabblab` is the organization,
  `krabb` is the product.
- The wordmark is Space Grotesk Bold, lowercase, with letter-spacing `-0.03em`. The SVGs
  hold it as outlines, so no font is needed to render them.
- Keep clear space around the logo at least equal to the stem width (about a fifth of the
  mark's height). The mark needs room to read as a K.

## Colors

| Name | Hex | Use |
| --- | --- | --- |
| Coral | `#FF6F2C` (255, 111, 44) | Brand color: the logo on any background, accents and text on dark, large fills in either mode. |
| Coral Deep | `#B8480B` | Only small text, links, icons, and thin lines on light backgrounds. Never on dark. |
| Slate | `#1E2327` | Ink and dark backgrounds, instead of pure black. |
| Ivory | `#F4F1EC` | Light backgrounds and text on dark. |

- Coral Deep exists because Coral is too light for small text on light backgrounds.
- Slate replaces black because pure black is harsher and off-brand next to Coral.

## Contrast

Text must reach 4.5:1 against its background.

| Pair | Ratio | Verdict |
| --- | --- | --- |
| Coral on Slate | 5.7:1 | Text allowed. |
| Ivory on Slate | 14.1:1 | Text allowed. |
| Coral Deep on white | 5.3:1 | Text allowed. |
| Coral Deep on Ivory | 4.7:1 | Text allowed. |
| Coral on white | 2.8:1 | Logo and large fills only. |
| Coral on Ivory | 2.5:1 | Logo and large fills only. |
| Coral Deep on Slate | 3.0:1 | Never; use Coral on dark. |

## Assets

Brand files live in `krabb-assets/brand/`. SVGs in `svg/` are the sources; PNGs in `png/`
are exports for places that need raster images. Each PNG has the SVG's name plus its size.

| Asset | Files | Use |
| --- | --- | --- |
| Krabb logo, dark | `krabb-logo-on-dark.svg`, `krabb-logo-on-dark-512.png` | Product logo on Slate or other dark backgrounds. |
| Krabb logo, light | `krabb-logo-on-light.svg`, `krabb-logo-on-light-512.png` | Product logo on Ivory, white, or other light backgrounds. |
| Krabblab logo, dark | `krabblab-logo-on-dark.svg`, `krabblab-logo-on-dark-512.png` | Organization logo on dark backgrounds. |
| Krabblab logo, light | `krabblab-logo-on-light.svg`, `krabblab-logo-on-light-512.png` | Organization logo on light backgrounds. |
| Mark, coral | `krabb-mark-coral.svg`, `krabb-mark-coral-512.png` | The default mark alone, on any background. |
| Mark, slate | `krabb-mark-slate.svg`, `krabb-mark-slate-512.png` | Single-color mark on light or Coral backgrounds. |
| Mark, white | `krabb-mark-white.svg`, `krabb-mark-white-512.png` | Single-color mark on dark backgrounds or photos. |
| App icon | `krabb-app-icon.svg`, `krabb-app-icon-512.png` | Slate mark on a rounded Coral square, for application icons. |
| GitHub avatar | `krabblab-avatar-coral.svg`, `krabblab-avatar-coral-512.png` | Organization avatar. |
| GitHub avatar, slate | `krabblab-avatar-dark.svg`, `krabblab-avatar-dark-512.png` | Alternative avatar where Coral clashes. |
| Social preview, coral | `krabb-social-preview-coral.svg`, `krabb-social-preview-coral-1280x640.png` | Repository social preview and the README cover. |
| Social preview, dark | `krabb-social-preview-dark.svg`, `krabb-social-preview-dark-1280x640.png` | Alternative cover for dark contexts. |

Logo PNGs are 512 pixels tall and as wide as the wordmark needs. Recompress new PNG
exports losslessly before adding them.

## Terminal

- The TUI accent is `ACCENT_COLOR = Color::Rgb(255, 111, 44)`, the Coral value, in
  `krabb-plugins/clients/tui/src/ui.rs`. It marks headers, borders, prompts, and inline
  code.
- CLI help and guided commands use bold Coral for headers, usage, and prompts, from
  `krabb-runtime/src/style.rs`.
- Coral looks right on dark and light terminals alike, so nothing detects the theme.
  Body text, including code, keeps the terminal's own foreground, which is light on dark
  themes and dark on light ones. Never set text to white, black, or a light gray.
- Animated glows stay in the Coral range: vary green within about 111 to 150 and blue
  within about 44 to 90. They must never drift toward yellow-orange, which reads as a
  warning rather than the brand.
- Use the terminal's own grays for secondary text; they follow the user's theme.

## Writing

These rules cover everything a person reads: CLI output and errors, chat replies to slash
commands, and TUI messages. Model-facing prompts and tool results are exempt.

- Put one sentence on each line. Merge two short sentences into one rather than sharing
  a line.
- Use no semicolons. Join clauses with "and", "so", or "because", or start a new line.
- Group related lines under a short title, with a blank line before and after each
  group and none inside it.
- Number questions and steps, and state each one's limits and an example, such as
  `Up to 40 characters, for example: Pinch`.
- Offer choices as a vertical list with a `›` marker on the selection, moved with the
  arrow keys. Without a terminal, take the choice from a flag instead.
- Say each thing once. Instead of repeating a summary or printing a file's contents,
  print the command that shows them.
- End every error and failure with one next action, so the user always knows how to recover:
  the command to run, or what to change when no command fits.
- End with the next action as a runnable command, and end that sentence with a period
  like any other.
- Mark finished steps with `✓` and a full sentence that names the kind of thing, such as
  `✓ Installed openai provider.`
- Set headers, prompts, and the selection in bold Coral, hints in faint text, examples
  in faint italics, and values that must stand out, such as links, in bold.
- Keep every line readable without color, because `NO_COLOR`, `TERM=dumb`, and pipes
  print plain text.
- Start every line at the left edge. Don't indent hints, choices, or answers to line up
  under a question, and don't justify text into columns. Blank lines and color carry the
  grouping instead.
- Mark where the person types with `|> `.

```text
Identity

1. What should your Krabb be called?
Up to 40 characters, for example: Pinch
|> Pinch

Intelligence

2. Which provider should your Krabb think with?

› OpenAI
  Anthropic
  Ollama Local
  Skip

↑/↓ to move, Enter to choose

✓ Installed openai provider.

Your Krabb is ready.
Chat with it with `krabb tui`
```

## Tool Names

These rules cover every tool name a model sees. [Tools](krabb-docs/tools.md) lists them.

- Name each tool `radix_action` in snake case, such as `browser_open` or `task_pause`.
- The radix names the area or plugin, and the action is a verb.
- Give each action its own tool rather than an `action` argument.
- Renaming a tool follows the [deprecation policy](DEPRECATIONS.md).

## Don't

- Don't recolor, stretch, rotate, or outline the mark; it is recognized by its exact shape.
- Don't use pure black; use Slate.
- Don't use Coral for small text on light backgrounds; use Coral Deep.
- Don't put the Slate K on Coral outside the app icon, the avatars, and the coral social
  preview. Elsewhere, Coral is the mark's color, not its background.
- Don't crowd the logo; keep clear space equal to the stem width.

## Known Gaps

- The TUI and CLI always use Coral, which is below 4.5:1 for small text on light terminal
  themes, so they set it in bold.
- The social previews use tints outside the palette for their large background K:
  `#F2621F` on Coral and `#262D32` on Slate. Keep these tints to the covers.
