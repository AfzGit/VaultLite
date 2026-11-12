# VaultLite

**VaultLite** is a lightweight, read-only browser for [Obsidian](https://obsidian.md) vaults. It serves a vault locally at `127.0.0.1` and opens it in your default browser—without uploading your notes or exposing the server to your local network.

The desktop launcher lets you choose a vault, optionally select a homepage note, and start or stop the local server. For scripts and headless workflows, VaultLite also provides a command-line mode.

- **CLI memory usage:** less than `8 MB`
- **Desktop launcher memory usage:** approximately `70 MB`
- **Access:** local machine only (`127.0.0.1`)
- **Mode:** read-only

![VaultLite screenshot](https://github.com/user-attachments/assets/450f3c16-a64e-45bc-aee8-c884c14b8fd2)

## Getting started

### Desktop application (recommended)

1. Open the [GitHub Releases](../../releases) page.
2. Download the archive for your operating system.
3. Extract the archive and run `vaultlite` (`vaultlite.exe` on Windows).
4. Choose your Obsidian vault and, optionally, a homepage note.
5. Select **Start server** to open the vault in your default browser.

The launcher includes the following controls:

| Control | Description |
| --- | --- |
| **Choose vault** | Select a vault folder with the native folder picker. It starts in the last-used location. |
| **Recent vaults** | Reopen any of the last eight selected vaults. |
| **Homepage picker** | Choose a Markdown homepage from the selected vault. |
| **Recent homepages** | Reuse any of the last eight selected homepage entries. |
| **Homepage note** | Optionally enter a path relative to the vault, such as `Home.md`. |
| **Start server** | Start the local read-only server and open the vault in your default browser. |
| **Open browser** | Reopen the current VaultLite URL if you closed the browser tab. |
| **Stop server** | Stop serving the vault. Closing the launcher also stops the server. |

The server binds only to `127.0.0.1`. It is not accessible from other devices on your LAN, and vault contents are not uploaded.

## Features

### Notes and links

- Renders CommonMark Markdown, including tables, strikethrough, task lists, and footnotes.
- Supports Obsidian wiki links: `[[Note]]`, `[[folder/Note]]`, and `[[Note|Alias]]`.
- Resolves wiki links relative to the current note first, then relative to the vault. A case-insensitive filename fallback is also supported.
- Opens `[[document.pdf]]` in a new browser tab. PDF responses are served inline so the browser's PDF viewer can display them.
- Resolves other local file links within the vault. Image links open in the VaultLite reading pane.
- Supports Markdown image and file links relative to the current note, including destinations with spaces, angle-bracket destinations, and optional Markdown link titles.
- Supports image embeds such as `![[image.png]]`, `![[image.png|Alt text]]`, and width hints such as `![[image.png|320]]`.
- Resolves extensionless image embeds by attachment filename stem, including dotted titles such as `1.04) Wudu (Ablution)`.
- Supports note embeds with `![[Note]]` and `![[Note.md]]`. Embedded notes render inline as Markdown and include a control for opening the source note.
- Detects direct and indirect recursive note embeds and displays a **Recursive embed stopped** notice instead of entering an infinite loop. Depth and total-size limits protect against very large embed graphs.
- Re-renders embedded notes on each request so edits to their source files appear immediately.

### Bases and Canvas

- Opens `.base` files as in-browser previews rather than downloads. The first configured view is displayed as a table, list, or card grid.
- Supports common file fields and simple `note.<property>` frontmatter columns in Bases previews.
- Opens `.canvas` files in a read-only board viewer. Text, group, file, and link nodes are drawn at their saved coordinates, with connecting edges.
- Supports Canvas board dragging, zooming, and fit-to-window controls. `Ctrl` + scroll zooms around the point beneath the cursor.
- Renders basic Markdown in Canvas text nodes, including headings, lists, **bold**, *italic*, `code`, strikethrough, and highlights.

### Navigation and search

- Provides a compact, nested file navigator with icons for folders, Markdown notes, Bases, canvases, images, and other files.
- Uses fuzzy filename search by default, with typo tolerance, subsequence matching, and multi-word queries. Closer filename matches rank higher than folder-name matches.
- Shows matching files in their folder hierarchy. Folder names are searchable, but filename matches receive higher priority.
- Includes a separate search field for headings in the current note.
- Press `/` or `Ctrl+O` to focus file search. Press `Escape` to clear the active search.
- Includes a right-hand table of contents (TOC) with expandable and collapsible sections. The current heading is highlighted while scrolling.
- Clicking a TOC heading scrolls it to the top and expands its subheadings. Collapsing and reopening a section reveals the next heading level.
- Keeps the TOC and file navigator in separate, full-height sidebars. Both sidebars can be resized independently.
- Provides a toolbar control to collapse all TOC headings to level 1.
- Displays inbound backlinks and outgoing note links in a two-column section at the bottom of each note.

### Reading and layout

- Includes dark and light themes. Dark mode is the default for new visitors, and the selected theme is saved in the browser.
- Provides responsive layouts: below 1024 px, the TOC moves below the note; below 760 px, the file navigator becomes a touch-friendly drawer with a backdrop.
- Keeps the right-sidebar toggle visible in the top-right rail whether the TOC is open or closed.
- Provides icon-only controls with tooltips for folder expansion, sidebar visibility and resizing, independent wrapping of the file tree and TOC, and note-body text size.
- Keeps note font-size changes separate from sidebar text sizes.
- Supports optional filename and TOC wrapping, as well as justified note text.
- Preserves the application shell and file tree when navigating between notes, image previews, Bases, and Canvas views. These view changes fetch only the content fragment.
- Supports browser Back and Forward navigation between views.

### Performance and file handling

- Indexes the vault once at startup for constant-time path, filename, and stem lookups.
- Caches rendered notes in memory, bounded to approximately 32 MiB. Cached notes are refreshed when their file size or modification time changes.
- Serves images as raw bytes through a dedicated `/asset/` endpoint for embeds and standalone image previews. Legacy `/file/` image URLs remain compatible.
- Adds browser cache headers to images and other static files.
- Redirects previously generated `127.0.0.1:<port>/file/...` links into the current viewer. `.md` files are always rendered as notes.

## Command palette and keyboard shortcuts

Press `Ctrl+P` (`Cmd+P` on macOS) to open the command palette. Type to filter commands, use `↑` and `↓` to navigate, press `Enter` to run a command, and press `Escape` to close the palette.

Press `Ctrl+O` to open file search.

| Command | Default shortcut |
| --- | --- |
| Open command palette | `Ctrl+P` (`Cmd+P` on macOS) |
| Search files | `Ctrl+O` |
| Toggle left sidebar | `Alt+Z` |
| Toggle right sidebar | `Alt+X` |
| Toggle dark mode | `Alt+T` |
| Collapse all folders | `Alt+C` |
| Expand all folders | `Alt+Shift+C` |
| Increase note text size | `Alt+=` |
| Decrease note text size | `Alt+-` |
| Reset note text size | `Alt+0` |
| Toggle file-name wrapping | `Alt+W` |
| Toggle TOC wrapping | `Alt+Shift+W` |
| Choose colour palette | `Alt+P` |
| Go to vault home | `Alt+H` |

The following commands are available from the palette and have no default shortcut:

- Collapse all headings to level 1
- Toggle justified text
- Copy the current note
- Open the current note in Obsidian
- Open the VaultLite configuration folder
- Exit VaultLite and stop the server

You can assign shortcuts to these commands in `commands.conf`. This also lets you change or disable existing bindings.

### Shortcut configuration

`commands.conf` is stored next to `launcher.conf`:

| Platform | Location |
| --- | --- |
| Windows | `%APPDATA%\vaultlite\commands.conf` |
| macOS | `~/Library/Application Support/vaultlite/commands.conf` |
| Linux | `$XDG_CONFIG_HOME/vaultlite/commands.conf`, or `~/.config/vaultlite/commands.conf` |

The file is created with documented defaults on first run. Edit the `[commands]` section and restart VaultLite to apply changes.

Example:

```ini
[commands]
toggle-left-sidebar = Ctrl+Shift+L
toggle-right-sidebar = Alt+X
open-command-palette = Ctrl+P
open-search = Ctrl+O

# Use "off" to disable a command's shortcut.
choose-palette = off

# Palette-only commands can also be assigned shortcuts.
copy-note = Alt+Shift+C
open-in-obsidian = Alt+Shift+O
```

Set `VAULTLITE_COMMANDS_FILE` to load the configuration from a different location, for example when running from a portable directory or in tests.

## Command-line mode

The CLI remains available for scripts and headless workflows:

```sh
vaultlite --vault /path/to/vault
```

### Options

| Option | Description |
| --- | --- |
| `--vault <path>` | Path to the Obsidian vault. |
| `--index <path>` | Optional homepage note path relative to the vault, such as `Home.md`. |
| `--port <port>` | Use a fixed port, for example `8080`. By default, VaultLite selects an available local port. |
| `--help` | Display command-line usage. |

Running `vaultlite` without arguments opens the desktop launcher.

## Build from source

VaultLite pins Rust `1.87.0` in `rust-toolchain.toml`. Install that toolchain and the native build prerequisites for your operating system, then run:

```sh
cargo build --release
```

The executable is written to:

- Linux and macOS: `target/release/vaultlite`
- Windows: `target/release/vaultlite.exe`

### Linux prerequisites

On Debian or Ubuntu, install the required packages:

```sh
sudo apt-get install pkg-config libxkbcommon-dev libwayland-dev libx11-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libgl1-mesa-dev
```

## Cross-platform releases

GitHub Actions builds release archives for:

- Linux x86_64
- Windows x86_64
- macOS Intel (x86_64)
- macOS Apple Silicon (aarch64)

### Release process

Releases follow semantic versioning (`MAJOR.MINOR.PATCH`) and use tags in the form `vX.Y.Z`.

1. CI runs on every pull request and every push to `main`. It checks formatting, runs Clippy with warnings denied and the test suite, performs a debug build, and checks JavaScript and release-script syntax.
2. Add release notes under `## Unreleased` in `CHANGELOG.md` as changes are merged.
3. In GitHub, open **Actions → Release → Run workflow** and enter the release version, such as `0.9.1`.
4. The workflow then:
   - promotes the `Unreleased` entries into a dated version section in `CHANGELOG.md`;
   - updates `Cargo.toml`, `Cargo.lock`, and the README version heading;
   - commits the release as `chore(release): vX.Y.Z` and creates the matching annotated tag;
   - verifies that the tag, Cargo version, README, and changelog agree;
   - builds archives for all four supported targets; and
   - publishes a GitHub Release using that version's changelog section as the release notes, with all archives attached.

The workflow fails if `Unreleased` is empty, ensuring that each release has documented changes.

To prepare a release locally instead, run:

```sh
node scripts/prepare-release.mjs 0.9.1
```

Commit the version bump and push the `v0.9.1` tag. The tag-triggered workflow performs the same verification, build, and publishing steps.

The workflows use the repository's default `GITHUB_TOKEN`; no third-party secrets are required. If your organization restricts workflow permissions, enable permission for GitHub Actions to create and approve releases under **Settings → Actions → General**.

### Windows behavior

The Windows executable is built with the GUI subsystem, so starting it normally does not open a terminal window. When CLI arguments are supplied, VaultLite attaches to the invoking PowerShell or Command Prompt session. When launched directly with CLI arguments, it opens a console so command-line diagnostics remain available.

## Launcher preferences

The desktop launcher remembers the last selected vault, up to eight recent vault folders, the last homepage entry, and up to eight recent homepage entries. These settings are stored locally in `launcher.conf`:

| Platform | Location |
| --- | --- |
| Windows | `%APPDATA%\vaultlite\launcher.conf` |
| macOS | `~/Library/Application Support/vaultlite/launcher.conf` |
| Linux | `$XDG_CONFIG_HOME/vaultlite/launcher.conf`, or `~/.config/vaultlite/launcher.conf` |

Only vault folder paths and homepage paths are stored. Vault contents are not copied or uploaded. Missing recent folders are ignored at startup.

A folder is accepted as a vault only if it contains an `.obsidian` directory. Invalid folders are not added to the recent list, and saved entries are revalidated when the launcher starts.

Keyboard shortcut overrides are stored separately in `commands.conf` in the same configuration directory. See [Shortcut configuration](#shortcut-configuration).

## Known limitations

VaultLite is designed to be a lightweight, read-only viewer, not a complete Obsidian replacement. The following are not implemented:

- Editing notes or other vault files
- Obsidian plugins
- Dataview
- Full Obsidian Bases expression evaluation, including filters, formulas, grouping, and sorting
- Graph features
- Live reload

The vault directory index is built at startup. Restart VaultLite after adding or renaming files so the navigator and link index can be refreshed.

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for details about the server, rendering pipeline, front-end modules, configuration flow, security model, and implementation notes.

## Dependencies and build notes

- The local server is based on `tiny_http`.
- The desktop launcher uses `eframe` 0.30 with the OpenGL renderer and X11 integration enabled.
- The folder picker uses `tinyfiledialogs` to avoid a heavier asynchronous dialog stack.
- Optional `eframe` integrations, including the Wayland renderer, web screen reader, and persistence, are disabled.
- Release builds strip symbols and use thin LTO with a single codegen unit to reduce executable size.
- The unused direct URL, IDNA, and image dependencies have been removed.
- `.cargo/config.toml` asks Cargo to prefer dependency versions compatible with the package MSRV when resolving a fresh lockfile. CI builds with the pinned Rust 1.87.0 toolchain.
- On Windows, the launcher uses the GUI subsystem for normal startup while preserving console diagnostics for CLI use.

## Reporting issues

When opening a bug report, include:

- Your operating system
- Your VaultLite version
- Your Rust version, if you built from source
- The steps needed to reproduce the issue

Do not attach private vault contents. Use a minimal sample vault to demonstrate the problem.

## License

VaultLite is licensed under the [MIT License](LICENSE).

