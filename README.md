# Cacofone Streamer

Plays music from your home media server to speakers and receivers on your network, using DLNA and UPnP. Part of [Cacofone](https://cacofone.com/streamer/).

## Layout

One Rust core, many front ends.

| Path | What |
| --- | --- |
| `crates/core` | Shared Rust core: discovery, library browsing, playback control |
| `crates/cli` | Command line interface (`streamer`) for Linux, macOS and Windows |
| `crates/tui` | Terminal UI (`streamer-tui`) for Linux, macOS and Windows |
| `apps/android` | Android app |
| `apps/ios` | iOS app |
| `apps/macos` | macOS app |
| `apps/windows` | Windows app |
| `apps/linux` | Linux desktop app |

The native apps use the core through bindings, which are not set up yet.

## Usage

```sh
streamer discover                 # search for 5 seconds, print a table of servers and renderers
streamer discover --json          # one JSON object per line, for scripts
streamer discover --watch         # keep running, print devices as they appear and disappear
streamer discover --type renderer --interface wlp3s0 --timeout 10
```

Options: `--timeout <secs>`, `--type server|renderer|all` (repeatable; `all` lists every UPnP device, for diagnostics), `--interface <name-or-ip>` (repeatable), `--json`, `--watch`. Logs go to stderr (`-v`, `-vv`), so stdout stays clean for scripts.

## Install from source

```sh
mise run install-cli     # installs `streamer` to ~/.local/bin (Windows: %LOCALAPPDATA%\Programs\cacofone-streamer\bin)
mise run install-tui     # installs `streamer-tui`
```

Without mise, run `crates/cli/bin/install.sh` (macOS, Linux) or `crates\cli\bin\install.bat` (Windows). See [CONTRIBUTING.md](CONTRIBUTING.md) for the test and uninstall scripts.

## Build

```sh
cargo build
cargo run -p cacofone-streamer-cli
cargo test
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
