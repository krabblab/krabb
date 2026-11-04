# Text

Shared bounded CommonMark parsing for the runtime's plain-text fallback and channel
adapters. `Document` retains text and semantic style spans using UTF-8 byte offsets.
Adapters select their supported features and convert offsets to their wire format;
`chunks` clips and rebases spans before rendering. `Syntax` accepts a channel's
markup delimiters and splits complete rendered fragments within its message limit.

Links are preserved as native destinations or visible text. Raw HTML remains literal
text; it is never executed or accepted as platform markup. Input, expanded output,
and style counts are bounded. This crate has no network or platform API clients.

Run `cargo test --locked -p krabb-text` for parser, formatting and chunking tests.
