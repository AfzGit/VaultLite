# VaultLite 0.8.9

VaultLite is a minimal, read-only browser for Obsidian vaults. It serves your vault on `127.0.0.1` and opens it in your default browser. The desktop launcher lets you choose a vault folder, optionally select a homepage note, and start or stop the local server.

## Desktop app (recommended)

Download the archive for your operating system from the [GitHub Releases](../../releases) page, extract it, and run the `vaultlite` executable (`vaultlite.exe` on Windows).

The small launcher window provides:

- **Choose vault** — select an Obsidian vault folder using the native folder picker. The picker starts in the last-used location.
- **Recent vaults** — reopen any of the last eight selected vaults with one click.
- **Homepage picker** — browse for a Markdown homepage inside the selected vault instead of typing its path.
- **Recent homepages** — reuse any of the last eight selected homepage entries.
- **Homepage note** — optionally enter a Markdown file path relative to that vault, such as `Home.md`.
- **Start server** — starts the local read-only server and opens the vault in your default browser.
- **Open browser** — reopens the current VaultLite URL if you closed the tab.
- **Stop server** — stops serving the vault. Closing the launcher also stops the server.

The server binds only to `127.0.0.1`; it is not exposed to your LAN and does not upload vault contents.

## Build from source

Install Rust 1.87.0 (the repository pins this toolchain in `rust-toolchain.toml`) and the native desktop build prerequisites for your platform, then run:

```sh
cargo build --release
```

The executable is written to `target/release/vaultlite` (or `target/release/vaultlite.exe` on Windows). Running it with no arguments opens the desktop launcher.

### Linux build prerequisites

On Debian/Ubuntu:

```sh
sudo apt-get install pkg-config libxkbcommon-dev libwayland-dev libx11-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgl1-mesa-dev
```

## Command-line mode

The original CLI workflow remains available for scripts and headless use:

```sh
vaultlite --vault /path/to/vault
```

Optional arguments:

- `--index path/to/Home.md` — render the supplied note as the homepage.
- `--port 8080` — use a fixed port. By default VaultLite selects an available local port.
- `--help` — show command-line usage.

## Cross-platform builds

GitHub Actions builds release binaries for:

- Linux x86_64
- Windows x86_64
- macOS Intel (x86_64)
- macOS Apple Silicon (aarch64)

Push a version tag such as `v0.8.0` to build and attach archives to a GitHub Release. Builds can also be started manually from the Actions tab.

## Features

- `[[Note]]`, `[[folder/Note]]`, and `[[Note|Alias]]` links.
- `[[document.pdf]]` opens the PDF in a new browser tab; the server marks PDF responses as inline so the browser viewer can display them.
- Other local file links resolve into the vault. Image links open an image preview inside the VaultLite reading pane.
- `![[image.png]]`, `![[image.png|Alt text]]`, and width hints such as `![[image.png|320]]` embed images in notes.
- Extensionless image embeds resolve by attachment filename stem, including dotted titles such as `1.04) Wudu (Ablution)`.
- Markdown image and file links are resolved relative to the current note. Previously generated `127.0.0.1:<port>/file/...` links are redirected into the current viewer; `.md` files always render as notes.
- Obsidian callouts such as `[!note]`, `[!warning]`, `[!tip]`, `[!success]`, and `[!quote]`, with collapsible `+` and `-` forms. Markdown links in callout titles are rendered as links.
- `.base` files open as in-browser Bases previews rather than downloads. The first configured view is displayed as a table, list, or card-style grid. Common file fields and simple `note.<property>` frontmatter columns are supported. Filter expressions, formulas, grouping, sorting, and the full Obsidian Bases expression language are not evaluated.
- Nested directory navigator with icons: 📁 folders, 📄 Markdown notes, 🖼️ images, and 🔗 other files.
- Fuzzy-by-default left search matches file names only; folders are shown only as the hierarchy for matching files. A separate search field filters headings in the current note. Press `/` to focus file search; Escape clears the active search.
- Light and dark themes, with the selected theme saved in the browser.
- Compact, collapsed frontmatter properties, filename heading, and right-hand table of contents.
- Icon-only controls with tooltips: collapse folders, resize or hide the left sidebar, independently wrap the left file tree and right TOC, and increase/decrease note-body font size. Font size changes do not affect either sidebar.
- Fast in-page navigation: switching notes, image previews, and Bases views fetches only the content fragment and preserves the application shell and file tree. Back/Forward works between these views. The bottom link-relations section is a two-column table: inbound backlinks on the left and outgoing note links on the right. Both columns show compact inline note-name links without folder paths.
- The vault is indexed once at startup for constant-time path/name/stem lookups. Rendered notes are cached in memory (bounded to approximately 32 MiB) and refreshed when file size or modification time changes. Images are served as raw bytes from a dedicated `/asset/` endpoint for both Markdown/Obsidian embeds and the standalone image preview; legacy `/file/` image URLs remain compatible. Images and other static files receive browser cache headers.
- CommonMark plus tables, strikethrough, task lists, and footnotes.

Wiki links resolve relative to the current note first, then relative to the vault. A case-insensitive filename fallback is also used. The directory index is built at startup; restart VaultLite after adding or renaming vault files so its navigator and link index are refreshed. VaultLite is intentionally a lightweight viewer rather than a complete Obsidian renderer. Plugins, Dataview, full Bases evaluation, graph features, editing, and live reload are not implemented.

## Dependencies

The desktop launcher uses `eframe` with only its OpenGL renderer and default fonts enabled. Release builds strip symbols, use thin LTO and a single codegen unit to reduce the shipped executable size. The folder picker uses `tinyfiledialogs`, avoiding the heavier asynchronous dialog stack. Optional eframe integrations (Wayland renderer, web screen reader and persistence) are disabled. The local server remains based on `tiny_http`; the browser is launched using native operating-system commands.

The launcher uses Eframe 0.30 with only the OpenGL renderer and X11 integration enabled. This avoids the newer Eframe dependency set that selected image 0.25.10 and ICU 2.4 (both requiring Rust 1.88). The unused direct URL/IDNA/image dependencies have been removed. `.cargo/config.toml` asks Cargo to prefer dependency versions compatible with the package MSRV when resolving a fresh lockfile. CI builds with the pinned Rust 1.87.0 toolchain.

On Windows, the executable is built as a GUI-subsystem application, so double-clicking it does not leave a console window open. When CLI arguments are supplied, VaultLite attaches to the invoking terminal (or allocates a console if launched directly), preserving command-line diagnostics.

## License

MIT


## GitHub release process

1. Create a GitHub repository and push this project to its `main` branch.
2. Confirm the **CI** workflow passes on the Actions tab.
3. Update `version` in `Cargo.toml`, the README heading, and `CHANGELOG.md`.
4. Commit the version bump and push it.
5. Create and push a matching version tag:

   ```sh
   git tag v0.8.2
   git push origin v0.8.2
   ```

6. The **Build desktop binaries** workflow builds Linux, Windows, Intel macOS, and Apple Silicon macOS archives and attaches them to a GitHub Release with generated release notes.

The workflows need the repository's default `GITHUB_TOKEN`; no third-party secrets are required. In repository Settings → Actions → General, allow GitHub Actions to create and approve releases if your organization has restricted workflow permissions.

### Windows behavior

The desktop launcher is compiled with the Windows GUI subsystem and therefore does not create a terminal window when started normally. CLI mode remains available when arguments are supplied; it attaches to the current console when launched from PowerShell/CMD, and opens a console when started directly with CLI arguments.

### Reporting issues

When filing a bug, include your operating system, VaultLite version, Rust version if built from source, and the steps needed to reproduce it. Do not attach private vault contents; use a minimal sample vault instead.


## Launcher preferences

The desktop launcher remembers the last selected vault, up to eight recent vault folders, the last homepage entry, and up to eight recent homepage entries. These preferences are stored locally in a small `launcher.conf` file:

- Windows: `%APPDATA%\\vaultlite\\launcher.conf`
- macOS: `~/Library/Application Support/vaultlite/launcher.conf`
- Linux: `$XDG_CONFIG_HOME/vaultlite/launcher.conf`, or `~/.config/vaultlite/launcher.conf`

Only folder paths and homepage paths are stored. Vault contents are not copied or uploaded. Missing recent folders are ignored when the launcher starts.


Folders in the navigator start collapsed by default. Markdown links and image destinations may contain spaces in filenames; angle-bracket destinations and optional Markdown link titles are also handled.


Vault selections are persisted as newline-separated entries in the launcher preferences file. A folder is accepted as a vault only when it contains an `.obsidian` directory; invalid folders are not saved to the recent list. Existing saved entries are revalidated on startup.
