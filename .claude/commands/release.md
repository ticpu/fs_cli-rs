Perform a release of fs_cli.

Optional override: $ARGUMENTS (format: vX.Y.Z). If provided, use that version.

## Version determination

1. Find the last release tag (`git tag --sort=-v:refname | head -1`).
2. Examine commits since that tag to classify the release type:
   - **Patch**: only bug fixes, dependency bumps, build changes, docs.
   - **Minor**: new features (`feat:`), user-visible behavior changes.
   - **Major**: breaking CLI or config changes (removed flags, incompatible fs_cli.yaml).
3. Bump the version accordingly. If **major**, stop and confirm before proceeding.

## Pre-release checks

Run in sequence — stop and report on any failure:

```sh
cargo fmt --all
cargo clippy --fix --allow-dirty --message-format=short
cargo test --release -- --quiet
cargo build --release
cargo build --release --target x86_64-pc-windows-gnu
```

## Steps

1. Bump `version` in `Cargo.toml`.

2. Run pre-release checks above.

3. Draft a changelog from `git log --oneline <last-tag>..HEAD`.

   **Rules:**
   - Group under: `New features:`, `Bug fixes:`, `Build:`, `Refactoring:` — omit empty sections.
   - Describe user-visible behavior, not implementation details.
   - Merge related commits for the same feature into one bullet.
   - No git hashes, no raw commit subjects, no co-author lines.

   Tag annotation format:
   ```
   vX.Y.Z

   New features:
   - what changed

   Bug fixes:
   - what was fixed

   Build:
   - what changed
   ```

4. Stage, commit, push:

```sh
git add Cargo.toml Cargo.lock
git commit -m "release: version X.Y.Z"
git push
```

5. Wait for CI to be green before tagging (`gh run watch` or `gh run list`). CI
   validates the binaries with `--version` and `--help` output checks on all
   targets. Never tag on red or pending CI.

6. Tag and push; GitHub Actions builds and publishes the release automatically:

```sh
git tag -as vX.Y.Z -m "$(cat <<'EOF'
vX.Y.Z

<changelog>
EOF
)"
git push --tags
```

7. Report the tag and changelog.

## Important

- **Cargo.lock is committed on the release commit** (binary crate) — stage it
  explicitly alongside Cargo.toml. It stays out of non-release commits.
- The tag is IMMUTABLE once pushed — never retag. Wrong? Make a new patch release.
- Release artifacts: `fs_cli_${version}_{amd64|arm64}.debian-compatible`,
  `fs_cli_${version}_amd64.windows.exe`.
- ARM64 smoke test if needed:
  `QEMU_LD_PREFIX=/path/to/aarch64/root qemu-aarch64-static fs_cli --version`.
