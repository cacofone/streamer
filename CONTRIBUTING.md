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

## Local install and test scripts

Build, test and install the binaries without `cargo install`. Scripts for macOS and Linux are `.sh`, for Windows `.bat`. With [mise](https://mise.jdx.dev) they are one command on every system:

```sh
mise run install-cli        # builds and installs `streamer` (also: install-tui)
mise run test-cli           # tests the crate and smoke-tests the binary (also: test-core, test-tui)
mise run uninstall-cli      # removes it again (also: uninstall-tui)
```

Pass options after `--`, for example `mise run install-cli -- --prefix /opt/streamer --dry-run`. Without mise, run the script for your platform directly, for example `crates/cli/bin/install.sh` or `crates\cli\bin\install.bat`.

The default install location is `~/.local/bin` on macOS and Linux and `%LOCALAPPDATA%\Programs\cacofone-streamer\bin` on Windows. The scripts never change your `PATH`. They only remove binaries that they installed themselves.

The shared implementation is in `scripts/`; the folders `crates/*/bin/` hold thin wrappers per crate. Keep each `.sh` and its `.bat` twin behaving the same. `.bat` files must keep CRLF line endings (`.gitattributes` takes care of this).

## Commits and pull requests

- One line per commit in the form `type(scope): message`, for example `fix(core): handle devices that answer discovery twice`.
- Keep pull requests focused and make sure CI passes.
- New dependencies must be licensed under MIT, Apache-2.0 or another license listed in `deny.toml`.

## License

By contributing you agree that your work is dual licensed under the MIT license and the Apache License 2.0, as described in the [README](README.md#license).
