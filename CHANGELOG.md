## Unreleased

- Fix the right sidebar layout so the table of contents always occupies its own grid column and cannot overlap the note or the top control rail.
- Dock the heading sidebar to the right viewport edge as a full-height panel, matching the file navigator's positioning.
- Keep the right-sidebar reopen button in the top-right control rail when the sidebar is hidden.
- Add a read-only `.canvas` board viewer with text, group, file, and link nodes plus curved edges, zoom, and fit-to-view controls.
- Render basic Markdown in canvas text nodes and allow dragging the board with the left mouse button to pan.
- Support `![[Note]]` and `![[Note.md]]` embeds with depth, size, and direct/indirect recursion guards.
- Let the right sidebar be widened or narrowed like the left sidebar, and add a toggle that collapses the table of contents to level 1 headings.
- Make dark theme the default for new visitors.
- Replace the filename filter with ranked fuzzy search: subsequence matching across the full name, typo tolerance, multi-word and folder-path matching, and best-match ordering. The command palette uses the same ranking.
- Highlight the active table-of-contents heading while scrolling, and keep the clicked heading pinned below the top rail while lazy images settle.
- Add expand/collapse arrows to table-of-contents sections.
- Swap the right-sidebar width buttons so the wider control comes first.
- Make canvas zoom keep the point beneath the cursor fixed.
- Add a palette command to toggle justified paragraph text.
- Add `ARCHITECTURE.md` documenting the server, rendering pipeline, front-end modules, workflows, and implementation quirks.
- Add palette commands to copy the current note, open it in Obsidian, open the VaultLite config folder, and exit VaultLite while stopping the server.
- Add a configurable command palette (`Ctrl+P`) and file search shortcut (`Ctrl+O`).
- Add editable keyboard bindings in `commands.conf`, with `Alt+Z` for the left sidebar and `Alt+X` for the right sidebar by default.
- Add responsive mobile support: off-canvas navigator with backdrop, stacked TOC and backlinks, touch-sized controls, and no horizontal overflow.
- Make the file tree more compact and replace emoji icons with an inline SVG icon set.
- Split the browser assets out of `main.rs` and add command-line tests for shortcut configuration.

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
