# Release checklist

- [ ] Update `package.version` in `Cargo.toml`.
- [ ] Update the README version heading and add a `CHANGELOG.md` entry.
- [ ] Run `cargo fmt --all -- --check` and `cargo test --all-targets` on Rust 1.87.
- [ ] Confirm CI passes on Linux, Windows, and both macOS architectures.
- [ ] Commit and push changes to `main`.
- [ ] Create and push the matching tag (`vX.Y.Z`).
- [ ] Download and smoke-test each release archive before announcing the release.
