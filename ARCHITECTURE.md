# VaultLite architecture

VaultLite is a small, dependency-light browser for an Obsidian vault. A native
launcher (or a CLI flag) starts a local HTTP server; the browser UI is a single
HTML shell with hand-written CSS and JavaScript. There is no build step for the
front end — the assets are compiled into the executable with `include_str!`.

This document describes how the pieces fit together, the main request and
rendering workflows, and the quirks worth knowing before changing the code.

## Repository map

| Path | Purpose |
| --- | --- |
| `src/main.rs` | Native launcher, HTTP server, vault index, Markdown renderer, navigation and page shell. |
| `src/config.rs` | Registration of palette commands, keyboard bindings, `commands.conf` creation and parsing. |
| `src/assets/page.html` | HTML skeleton, SVG icon sprite, placeholders for CSS/JS/content/config. |
| `src/assets/app.js` | All browser behaviour: navigation, search, TOC, command palette, canvas, preferences. |
| `src/assets/app.css` | Theme variables, reading layout, sidebar/TOC, canvas, palette, responsive rules. |
| `README.md` | User-facing feature and configuration documentation. |
| `CHANGELOG.md` | Per-release change log. |

`main.rs` is intentionally a single binary crate. Most server-side helpers are
free functions; shared state lives in `AppState`. The file is large but divided
into clearly commented sections (index, rendering, routes, launcher, tests).

## Runtime modes

`main()` chooses a mode based on arguments:

- **No arguments** → `run_gui()` opens the native eframe launcher. It remembers
  the last vault in the platform config directory (`preferences_path`), validates
  the folder (`is_valid_vault`), and starts the HTTP server in a background
  thread. The launcher displays the URL and can open the browser.
- **Any CLI argument** → `run_cli()` parses `--vault`, `--port`, `--index`, and
  related flags, then serves until interrupted. On Windows a console is
  attached first (`attach_cli_console`).

`start_server_for_vault` binds `tiny_http::Server` and returns a
`RunningServer`; its `Drop` implementation stops the server thread when the
launcher closes. This matters on Windows: the executable cannot be rebuilt while
a server still holds `target/debug/vaultlite.exe`.

## HTTP server

The server is deliberately synchronous and stateless per request. `respond`
dispatches on the URL prefix:

| Route | Behaviour |
| --- | --- |
| `/` | Configured index note, or an empty landing page. |
| `/note/<path>` | Full page shell with the rendered note. |
| `/file/<path>` | Raw file; Markdown renders as a note, images inline, PDFs inline. |
| `/image/<path>` | Standalone image preview fragment/page. |
| `/base/<path>` | Obsidian Bases table view. |
| `/canvas/<path>` | Canvas board page. |
| `/fragment/<kind>/<path>` | Content-only fragment used for in-page navigation. |
| `/canvas-data/<path>` | Raw `.canvas` JSON for the client renderer. |
| `/asset/<path>` | Image bytes referenced by notes. |
| `/raw/<path>` | Raw Markdown used by the “Copy note” command. |
| `/backlinks/<path>` | Backlink/outgoing-link HTML, possibly `202` while indexing. |
| `/api/*` | Token-protected POST endpoints for launcher/commands/exit. |

All filesystem paths pass through `safe_join`, which percent-decodes, rejects
absolute paths and `..` components, and canonicalizes the result under the vault
root. Path components in links use `url_path`, which percent-encodes with
`NON_ALPHANUMERIC`; as a result even `.` becomes `%2E` (for example
`/note/B%2Emd`). Tests assert these encoded forms.

### Session token and APIs

`session_token()` generates a random token embedded in the page as
`window.__VAULTLITE_CONFIG__.token`. `/api/*` requests are POST-only and checked
by `api_authorized`; they are used by the “open config folder” and “exit”
commands. Never move these actions to unauthenticated GET routes.

## Vault index and link resolution

`VaultIndex::build` walks the vault once at startup (`collect_files`), skipping
dotfiles and symlinks, and records:

- normalized relative paths (`by_relative_path`),
- lowercase file names (`by_name`),
- lowercase stems without the extension (`by_stem`).

This enables Obsidian-style resolution:

- exact/relative paths first (`resolve_file`),
- case-insensitive fallback (`find_case_insensitive`),
- Markdown targets with or without `.md`, including dotted note names
  (`resolve_markdown_target`),
- attachment/image fallback by stem (`resolve_embed_target`).

When adding a new file type, update the `is_*` helpers, `content_type`,
`file_icon`, and the route dispatch together.

### Backlinks

`build_backlink_index` scans Markdown notes and records both source→target and
target→source maps. It runs on a background thread at startup; while
`ready == false`, the `/backlinks/` route returns `202` and the client keeps
polling (see `hydrateBacklinks`). The index is rebuilt on a timer/trigger rather
than incrementally, so new links can take a moment to appear.

## Note rendering pipeline

`render_note` is the core server-side workflow. The order is important:

1. **Frontmatter** — `split_frontmatter` separates YAML-ish properties from the
   body and renders a compact property list.
2. **Wikilinks and embeds** — `rewrite_wikilinks` replaces `[[target]]` with
   anchors and `![[target]]` with either an image/attachment or an embedded note.
   Markdown embeds are rendered immediately but represented in the output by a
   unique token (`EmbedScope::token`). Embedded headings therefore do not appear
   in the outer table of contents.
3. **Markdown path rewriting** — `rewrite_markdown_paths` resolves relative
   image and file links without breaking ordinary web links.
4. **Markdown rendering** — `render_markdown_fragment` uses `pulldown-cmark`
   with tables, footnotes, strike-through, task lists, and smart punctuation,
   then post-processes headings, callouts, and image width hints.
5. **Heading ids and TOC** — headings are collected while ids are injected.
   The TOC is built afterwards so each entry knows whether it has child
   headings; entries carry `data-level` and `data-id` for the client.
6. **Embed substitution** — `apply_embeds` replaces tokens with the rendered
   `<section class="note-embed">` HTML after the heading/TOC pass.
7. **Page shell** — the article, backlink placeholder, and TOC are inserted into
   `page.html` with the CSS/JS assets and runtime config JSON.

### Embeds and recursion guards

`EmbedScope` tracks:

- a stack of canonical paths currently being rendered (direct and indirect
  recursion),
- `depth` with `MAX_EMBED_DEPTH` (8),
- a shared `budget` of `MAX_EMBED_BYTES` (4 MiB),
- a token counter.

Notes that embed other Markdown are not cached (`has_markdown_embed`), so edits
to the embedded note appear on the next request. Image-only embeds remain
cacheable. If an embed cannot be expanded, `embed_notice` emits a linked notice
such as “Recursive embed stopped.” rather than failing the page.

### Cache

`cached_note` reads a note, renders it, and stores the HTML in `NoteCache`
bounded by `CACHE_LIMIT`. The cache is keyed by canonical path and cleared when
the vault changes. Because render output for an embedding note depends on other
files, those notes bypass the cache. The cache stores final article HTML, not
the full page shell.

## Other renderers

- `render_base` reads an Obsidian `.base` YAML definition and renders a table,
  resolving note properties (`frontmatter_property`, `base_field`).
- `render_canvas` emits a host element plus a data URL; `renderCanvasBoard` in
  `app.js` draws the board client-side. Canvas rendering is read-only.
- `render_image` produces a standalone preview.
- `nav` builds the folder tree, with persisted open/closed state handled on the
  client.

## Front-end architecture

`app.js` is an IIFE with no framework. It is organized into comment-delimited
sections:

1. **Storage and theme** — `store` wraps `localStorage` with a `vaultlite-`
   prefix. Theme (`dark` by default via `data-theme="dark"` in `page.html`),
   palette, sidebar widths, font size, wrapping, and justification are persisted.
2. **Sidebars and TOC controls** — left/right sidebar visibility, right-sidebar
   width (`--toc-width`, 160–520 px), and level-1-only mode.
3. **File tree and fuzzy search** — folders persist their state; search ranks
   filenames and paths (substring, subsequence, bounded typo distance). The
   command palette reuses `fuzzyScore`.
4. **Backlinks** — polls `/backlinks/` until the server index is ready, then
   replaces the placeholder table.
5. **Canvas** — `renderCanvasBoard` builds DOM nodes (no `innerHTML` for note
   content), renders basic Markdown in text nodes, supports drag-to-pan, and
   scales around the cursor while zooming.
6. **In-page navigation** — intercepts local links, fetches
   `/fragment/<kind>/<path>`, swaps the content, and updates history without a
   full reload. Keyboard modifiers and external links fall through to the
   browser.
7. **Commands and shortcuts** — commands are declared in one array; their
   bindings come from `window.__VAULTLITE_CONFIG__.bindings` (generated from
   `commands.conf`). `bindingMatches` handles platform modifiers.
8. **Initial view** — syncs the active file, initializes the canvas, and hydrates
   backlinks.

### Table of contents behaviour

The TOC is rendered flat but each entry has `.toc-entry`, `data-level`,
`data-id`, and a toggle button. `setupToc` is the single owner of outline state:

- **Collapse** — the `collapsed` set hides entries deeper than a collapsed
  heading. Arrow buttons reflect `aria-expanded` and rotate via CSS.
- **Active heading** — on a throttled scroll handler, the last heading whose top
  passed the top 20% of the viewport is highlighted and marked
  `aria-current="location"`. Clicking a heading updates the highlight first.
- **Stable navigation** — clicking a TOC link scrolls the heading below the top
  rail. A short-lived stabilization window re-aligns the same heading when a
  `ResizeObserver` sees layout changes or an image finishes loading, so lazy
  images cannot push the target away. User scroll input cancels the lock.
- **Heading search** — filters `.toc-entry` elements by label text.

`setupToc` runs on initial load and after every fragment navigation; the old TOC
is removed with the replaced content, so listeners do not accumulate.

### Preference keys

| Key | Meaning |
| --- | --- |
| `vaultlite-theme` / `vaultlite-palette` | Appearance |
| `vaultlite-sidebar-width`, `vaultlite-toc-width` | Panel widths |
| `vaultlite-sidebar-hidden`, `vaultlite-right-sidebar-hidden` | Sidebar visibility |
| `vaultlite-note-font-size` | Reading text size |
| `vaultlite-justify` | Justified paragraph text |
| `vaultlite-folder-state` | Open/closed folders |
| `vaultlite-toc-level-one` | TOC level-1-only mode |
| `vaultlite-left-wrap`, `vaultlite-right-wrap` | Text wrapping modes |

## Command configuration

`src/config.rs` owns the command IDs and default bindings. At startup,
`UiConfig::load` reads `%APPDATA%\vaultlite\commands.conf` (or the path in
`VAULTLITE_COMMANDS_FILE`), writes a commented template on first run, then
merges user overrides. Unknown commands are ignored and invalid shortcuts keep
the default. The resulting map is serialized into the page as JSON.

When adding a command:

1. Add the ID and default binding (use `""` for palette-only) to
   `DEFAULT_BINDINGS`.
2. Add the handler to the `commands` array in `app.js` with the same ID.
3. Update the template comment and `README.md`.
4. If the command needs state, persist it through `store` and expose a UI
   indicator when appropriate.

## Security model

VaultLite is a local-only reader. It still treats vault content as untrusted:

- All user/vault text is HTML-escaped with `esc` before being inserted into
  server-rendered HTML.
- `safe_join` prevents directory traversal; symlinks are skipped while indexing.
- Canvas note text is rendered with DOM APIs, never `innerHTML`.
- Mutating APIs are POST-only and require the session token.
- Responses for arbitrary files include `X-Content-Type-Options: nosniff`.

There is no authentication on read routes; bind to loopback only. Do not expose
the server on a public interface.

## Development workflow

```powershell
cargo fmt --all
cargo check --all-targets
cargo test --all-targets
node --check src/assets/app.js
cargo build
```

Because the assets are compiled in, any change to `app.js`, `app.css`, or
`page.html` requires a rebuild before the browser sees it. If `cargo build` fails
with `Access is denied` on Windows, stop the running VaultLite process first —
the executable is still in use.

Tests live in `src/main.rs` (`page_tests`) and `src/config.rs`. Front-end tests
are usually driven through a headless browser (Chrome DevTools Protocol) against
a temporary vault; keep such scripts outside the repository.

## Quirks and pitfalls

- **Percent encoding**: `url_path` encodes `.` as `%2E`. Build links with
  `url_path`/`relative_url` rather than `format!` if you want matching tests.
- **TOC ordering**: the TOC is generated after headings are collected, but
  before embeds are substituted. Do not move `apply_embeds` earlier or embedded
  headings will pollute the outline.
- **Cache correctness**: any feature that makes a note depend on another file
  must also bypass or invalidate the note cache (`has_markdown_embed` is the
  existing example).
- **Backlink latency**: `202` responses are normal during startup; the client is
  designed to poll, so avoid treating them as failures.
- **Sticky TOC**: the right sidebar is a sticky, full-height panel. Resizing it
  changes a grid column, so any new floating control must reserve the top rail
  (`--top-rail`) to avoid overlap.
- **Fragment navigation**: `navigateNote` replaces `#content` and re-runs the
  per-page initializers. New UI that binds to document-wide elements must either
  be idempotent or scoped to the replaced content.
- **Zoom anchoring**: the canvas stage is scaled but the board's layout size is
  not; scroll offsets are in scaled stage pixels. Cursor-centred zoom converts
  through `scale` before and after the change. The stage reserves one viewport
  of padding around the board (`data-pad-x`/`data-pad-y`) so a fitted board
  still has scroll range to anchor against; remove that padding and zooming a
  small board will clamp and drift.
- **Windows file locks**: the running executable blocks rebuilds; stop the
  server/launcher you started rather than killing all VaultLite processes.
