# Test Support

This unpublished workspace crate provides shared local fixtures through dev-dependencies.
It is not part of a production plugin or host dependency graph. Consumer tests own the
`#[test]` entry points, platform gates and provider adapters; fixtures use explicit APIs.
Run `make verify` to exercise the contracts through those consumers. The full coverage
gate measures this crate through OpenAI, Telegram and Discord fixtures; each production
consumer retains its independent coverage gate.
