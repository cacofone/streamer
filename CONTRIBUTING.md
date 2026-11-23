# Contributing

Thanks for your interest in Cacofone Streamer. Bug reports, ideas and pull requests are welcome.

## Before you start

- For anything bigger than a small fix, open an issue first so we can agree on the approach.
- Security problems go to the address in [SECURITY.md](SECURITY.md), not to a public issue.

## Development

You need the Rust toolchain pinned in `rust-toolchain.toml`. `rustup` installs it automatically. With [mise](https://mise.jdx.dev) you can also run `mise install`.

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

With mise, `mise run check` runs formatting, lint and tests together. CI runs the same checks on Linux, macOS and Windows, plus `cargo deny` for licenses and advisories.

## Commits and pull requests

- One line per commit in the form `type(scope): message`, for example `fix(core): handle devices that answer discovery twice`.
- Keep pull requests focused and make sure CI passes.
- New dependencies must be licensed under MIT, Apache-2.0 or another license listed in `deny.toml`.

## License

By contributing you agree that your work is dual licensed under the MIT license and the Apache License 2.0, as described in the [README](README.md#license).
