# FreeSWITCH CLI Client Project

## Project Overview
Rust-based interactive CLI client for FreeSWITCH using ESL (Event Socket Layer).

## Build System
- Local: `cargo build --release`
- Container: `./build.sh` (Debian Buster for compatibility)
- CI/CD: `.github/workflows/build.yml` builds on push/tag

## Development Commands
- `cargo check --message-format=short` - Fast syntax check
- `cargo clippy --fix --allow-dirty --message-format=short` - Linting
- `cargo test --message-format=short` - Tests

## Release Process
1. Bump version in `Cargo.toml`
2. Commit and push to master 
3. `git tag -as vX.X.X -m "Release vX.X.X"`
4. GitHub Actions creates release automatically

## Project Structure
- Binary: `fs_cli` (see `Cargo.toml`)
- Main: `src/main.rs`
- Config system: `src/config.rs` + `src/args.rs` (YAML profiles, see `fs_cli.yaml`)
- Commands: `src/commands.rs`
- Release naming: `fs_cli_${version}_amd64.debian-compatible`

## Configuration (v0.2+)
- YAML config with profiles: see `fs_cli.yaml` for example
- Embedded default config in binary, auto-creates missing files
- Usage: `fs_cli [profile]`, `--config path`, `--list-profiles`
- Locations: `~/.config/fs_cli.yaml`, `~/.fs_cli.yaml`, `/etc/freeswitch/fs_cli.yaml`

## Dependencies
- `freeswitch-esl-rs`, `rustyline` (git deps)
- `tokio`, `serde_yaml`, `crossterm`