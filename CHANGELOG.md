## 0.8.9

- Resolve Obsidian note links when the note name itself contains periods (for example, `[[Project.v2]]` → `Project.v2.md`).

# Changelog

## 0.8.3

- Refine callout layout: vertically center icons and reduce the oversized left accent stripe.


## 0.8.2

- Build the Windows desktop launcher with the GUI subsystem to avoid an unwanted terminal window.
- Reattach or allocate a console for explicit Windows CLI mode.
- Use Eframe 0.30 to avoid the newer dependency set requiring Rust 1.88.
- Remove unused direct URL, IDNA, and image dependencies.
- Pin the Rust toolchain to 1.87.0 and prefer MSRV-compatible dependency resolution.
- Add GitHub Actions CI for pull requests and main-branch pushes.
- Improve cross-platform release packaging and generated release notes.

## 0.8.4

- Fix native homepage file picker filter argument for tinyfiledialogs.

- Added native Markdown homepage file selection.
- Added one-click recent vault and homepage selections.
- Remember the last vault, homepage, and up to eight recent entries across launches.
- Start the native folder picker in the last-used vault location.

## 0.8.5

- Resolve Markdown links and image embeds whose filenames contain spaces.
- Start all file-tree folders collapsed by default.

## 0.8.6

- Fixed launcher preference serialization to write real line breaks, restoring recent-vault persistence.
- Validate selected folders by requiring a `.obsidian` directory before accepting or remembering them.
- Revalidate saved recent vaults on startup and when selected.

## 0.8.7

- Fix Rust character literal syntax in Markdown destination parsing.

## 0.8.8

- Fix `Default` derive placement for launcher preferences.
- Pass owned Markdown link destinations by reference to resolver helpers.
