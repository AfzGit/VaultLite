# Release checklist

Most of this is automated by the **Release** workflow.

## Before releasing

- [ ] Add user-facing notes under `## Unreleased` in `CHANGELOG.md`.
- [ ] Confirm the **CI** workflow passes on `main` (formatting, Clippy, tests, build, asset syntax).
- [ ] Choose the next semantic version.

## Release

- [ ] Open **Actions → Release → Run workflow** and enter the version (without a `v`).
- [ ] Wait for the release job to build Linux, Windows, Intel macOS, and Apple Silicon macOS archives.
- [ ] Confirm the GitHub Release contains the changelog section and every archive.

## After releasing

- [ ] Download and smoke-test at least the Windows and Linux archives.
- [ ] Announce the release.

## Local alternative

```sh
node scripts/prepare-release.mjs 0.9.1
git add Cargo.toml Cargo.lock README.md CHANGELOG.md
git commit -m "chore(release): v0.9.1"
git tag -a v0.9.1 -m "VaultLite v0.9.1"
git push origin main
git push origin v0.9.1
```

The tag-triggered workflow verifies the version files, builds, and publishes the release.
