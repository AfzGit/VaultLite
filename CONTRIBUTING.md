# Contributing

## Development setup

Install Rust 1.87.0. The repository's `rust-toolchain.toml` selects it automatically when rustup is installed.

On Debian/Ubuntu, install native GUI build dependencies:

```sh
sudo apt-get install pkg-config libxkbcommon-dev libwayland-dev libx11-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgl1-mesa-dev
```

Build and run:

```sh
cargo build
cargo run
```

Run checks before opening a pull request:

```sh
cargo fmt --all -- --check
cargo test --all-targets
```

## Pull requests

- Keep the viewer read-only and focused on fast vault browsing.
- Avoid adding large GUI/browser runtimes or unnecessary dependencies.
- Preserve compatibility with Rust 1.87.
- Include a concise description, reproduction steps for fixes, and screenshots for visible UI changes.
- Do not include personal vault data in commits or test fixtures.
