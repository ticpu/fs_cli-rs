# fs_cli-rs

Interactive FreeSWITCH CLI client written in Rust using
[freeswitch-esl-tokio](https://github.com/ticpu/freeswitch-esl-tokio).

## Features

- Readline with command history, search, and tab completion via `console_complete`
- Colorized log and command output (configurable: `never`, `tag`, `line`)
- YAML configuration profiles (`~/.config/fs_cli.yaml`, `/etc/freeswitch/fs_cli.yaml`)
- Automatic reconnection on connection loss (`-R`)
- Userauth support (`-u user@domain`)
- Non-interactive mode (`-x "command"`, repeatable)

## Installation

```sh
cargo build --release
```

Pre-built binaries for Linux AMD64/ARM64 and Windows are available on the
[releases page](https://github.com/ticpu/fs_cli-rs/releases).

## Usage

```sh
# Default connection (localhost:8021, password ClueCon)
fs_cli

# Remote host with profile
fs_cli -H 192.168.1.100 -P 8021 -p mypassword

# Userauth
fs_cli -u admin@default -p secret

# Non-interactive
fs_cli -x "sofia status" -x "show channels"

# Use a named profile from config
fs_cli production
```

Run `fs_cli --help` for full options, `fs_cli --list-profiles` to see
configured profiles.

## Configuration

On first run, `fs_cli` creates a default config at `~/.config/fs_cli.yaml`.
Profiles override defaults per-connection:

```yaml
default:
  host: localhost
  port: 8021
  password: ClueCon
  log_level: debug
  color: line

production:
  host: pbx.example.com
  password: secret
  quiet: true
```

## License

MIT OR Apache-2.0
