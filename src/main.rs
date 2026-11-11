#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod config;

use config::UiConfig;
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use pulldown_cmark::{html, Options, Parser};
use regex::Regex;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    io::Read,
};
use std::{
    env, fs,
    net::TcpListener,
    path::{Component, Path, PathBuf},
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};
use tiny_http::{Header, Response, Server, StatusCode};

fn wikilink_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"!\[\[([^\]]+)\]\]|\[\[([^\]]+)\]\]").unwrap())
}
fn markdown_image_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"!\[([^\]]*)\]\((<[^>]+>|[^)]+)\)"#).unwrap())
}
fn markdown_link_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"\[([^\]]+)\]\((<[^>]+>|[^)]+)\)"#).unwrap())
}

fn markdown_destination(raw: &str) -> String {
    let value = raw.trim();
    if value.starts_with('<') && value.ends_with('>') && value.len() >= 2 {
        return value[1..value.len() - 1].to_string();
    }
    // Optional Markdown titles follow the destination after whitespace.
    // Ordinary spaces inside filenames remain part of the destination.
    let bytes = value.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            let tail = value[i..].trim_start();
            if tail.starts_with('"') || tail.starts_with('\'') || tail.starts_with('(') {
                return value[..i].trim().to_string();
            }
        }
    }
    value.to_string()
}

fn heading_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?is)<h([1-6])>(.*?)</h[1-6]>").unwrap())
}
fn html_tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
}

fn open_default_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(error) = result {
        eprintln!("Could not open the default browser automatically: {error}");
    }
}

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn url_path(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn normalized_rel(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn safe_join(root: &Path, encoded_rel: &str) -> Option<PathBuf> {
    let decoded = percent_decode_str(encoded_rel).decode_utf8().ok()?;
    let relative = Path::new(decoded.trim_start_matches('/'));
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    let full = root.join(relative);
    let canonical = full.canonicalize().ok()?;
    if canonical.starts_with(root) {
        Some(canonical)
    } else {
        None
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            let hidden = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with('.'))
                .unwrap_or(false);
            if hidden {
                continue;
            }
            if path.is_dir() {
                collect_files(root, &path, out);
            } else if path.is_file() && path.strip_prefix(root).is_ok() {
                out.push(path);
            }
        }
    }
}

#[derive(Default)]
struct VaultIndex {
    files: Vec<PathBuf>,
    by_relative_path: HashMap<String, PathBuf>,
    by_name: HashMap<String, Vec<PathBuf>>,
    by_stem: HashMap<String, Vec<PathBuf>>,
}

impl VaultIndex {
    fn build(root: &Path) -> Self {
        let mut discovered = Vec::new();
        collect_files(root, root, &mut discovered);
        discovered.sort();

        let mut index = Self::default();
        for candidate in discovered {
            let Ok(canonical) = candidate.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(root) || !canonical.is_file() {
                continue;
            }
            let Some(relative) = canonical.strip_prefix(root).ok() else {
                continue;
            };
            let relative_key = normalized_rel(relative).to_lowercase();
            let name_key = canonical
                .file_name()
                .map(|v| v.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let stem_key = canonical
                .file_stem()
                .map(|v| v.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            index
                .by_relative_path
                .insert(relative_key, canonical.clone());
            index
                .by_name
                .entry(name_key)
                .or_default()
                .push(canonical.clone());
            index
                .by_stem
                .entry(stem_key)
                .or_default()
                .push(canonical.clone());
            index.files.push(canonical);
        }
        index
    }
}

struct CachedNote {
    modified: Option<SystemTime>,
    len: u64,
    html: String,
}

// FIFO eviction keeps inserts O(1) amortized. The previous implementation
// summed every cached entry and cleared the entire cache when it reached the
// limit, which caused avoidable rescans and re-renders while browsing large vaults.
#[derive(Default)]
struct NoteCache {
    entries: HashMap<PathBuf, CachedNote>,
    oldest: VecDeque<PathBuf>,
    bytes: usize,
}

#[derive(Default)]
struct BacklinkIndexState {
    ready: bool,
    by_target: HashMap<PathBuf, Vec<PathBuf>>,
    by_source: HashMap<PathBuf, Vec<PathBuf>>,
}

struct AppState {
    root: PathBuf,
    index: VaultIndex,
    nav_html: String,
    index_page: Option<PathBuf>,
    note_cache: Mutex<NoteCache>,
    backlinks: RwLock<BacklinkIndexState>,
    config_json: String,
    token: String,
}

/// Name Obsidian uses for a vault, which is normally the folder name.
fn vault_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Vault".into())
}

/// A short unguessable value injected into the page and required by endpoints
/// that can change local state or terminate the process.
fn session_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}{:x}", nanos, process::id(), nanos >> 32)
}

fn cached_note(state: &AppState, path: &Path) -> std::io::Result<(String, String)> {
    const CACHE_LIMIT: usize = 32 * 1024 * 1024;
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified().ok();
    let len = metadata.len();
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    if let Ok(cache) = state.note_cache.lock() {
        if let Some(entry) = cache.entries.get(path) {
            if entry.len == len && entry.modified == modified {
                return Ok((title, entry.html.clone()));
            }
        }
    }

    let source = fs::read_to_string(path)?;
    let relative = path.strip_prefix(&state.root).unwrap_or(path);
    let html = render_note(&state.root, &state.index, relative, &title, &source);

    // Notes that embed other Markdown are rendered on every request so changes
    // in the embedded note show up without restarting. Other notes are safe to
    // cache because their output only depends on their own file.
    let has_embeds = has_markdown_embed(&state.root, &state.index, relative, &source);
    // Keep the most recently inserted rendered notes up to a fixed memory cap.
    // Oversized notes still render normally but do not displace useful cache entries.
    if !has_embeds && html.len() <= CACHE_LIMIT {
        if let Ok(mut cache) = state.note_cache.lock() {
            if let Some(previous) = cache.entries.remove(path) {
                cache.bytes = cache.bytes.saturating_sub(previous.html.len());
                cache.oldest.retain(|candidate| candidate != path);
            }
            while cache.bytes.saturating_add(html.len()) > CACHE_LIMIT {
                let Some(oldest_path) = cache.oldest.pop_front() else {
                    break;
                };
                if let Some(evicted) = cache.entries.remove(&oldest_path) {
                    cache.bytes = cache.bytes.saturating_sub(evicted.html.len());
                }
            }
            cache.bytes = cache.bytes.saturating_add(html.len());
            cache.oldest.push_back(path.to_path_buf());
            cache.entries.insert(
                path.to_path_buf(),
                CachedNote {
                    modified,
                    len,
                    html: html.clone(),
                },
            );
        }
    }
    Ok((title, html))
}

fn clean_relative(base: &Path, target: &Path) -> Option<String> {
    let joined = if target.is_absolute() {
        return None;
    } else {
        base.join(target)
    };
    let mut parts: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

fn find_case_insensitive(index: &VaultIndex, target: &str) -> Option<PathBuf> {
    let normalized_target = target.replace('\\', "/");
    if let Some(found) = index
        .by_relative_path
        .get(&normalized_target.to_lowercase())
    {
        return Some(found.clone());
    }
    let target_name = Path::new(&normalized_target)
        .file_name()?
        .to_string_lossy()
        .to_lowercase();
    index
        .by_name
        .get(&target_name)
        .and_then(|matches| matches.first())
        .cloned()
}

fn resolve_file(root: &Path, from: &Path, target: &str, index: &VaultIndex) -> Option<PathBuf> {
    let target = target.trim();
    let path_part = target
        .split('#')
        .next()
        .unwrap_or(target)
        .split('?')
        .next()
        .unwrap_or("")
        .trim();
    if path_part.is_empty() {
        return None;
    }
    let decoded = percent_decode_str(path_part).decode_utf8().ok()?;
    let normalized = decoded.replace('\\', "/");
    let target_path = Path::new(&normalized);

    // First resolve relative to the current note, then from the vault root.
    let from_parent = from.parent().unwrap_or(Path::new(""));
    for base in [from_parent, Path::new("")] {
        if let Some(key) = clean_relative(base, target_path) {
            if let Some(found) = index.by_relative_path.get(&key.to_lowercase()) {
                if found.starts_with(root) {
                    return Some(found.clone());
                }
            }
        }
    }
    find_case_insensitive(index, &normalized).filter(|found| found.starts_with(root))
}

fn resolve_markdown_target(
    root: &Path,
    from: &Path,
    target: &str,
    index: &VaultIndex,
) -> Option<PathBuf> {
    if let Some(found) = resolve_file(root, from, target, index) {
        return Some(found);
    }

    // In Obsidian, periods are allowed in note names. A target such as
    // "Project.v2" may therefore refer to "Project.v2.md", not an extension.
    let path_part = target
        .split('#')
        .next()
        .unwrap_or(target)
        .split('?')
        .next()
        .unwrap_or(target)
        .trim();
    if path_part.is_empty() || path_part.to_ascii_lowercase().ends_with(".md") {
        return None;
    }
    let with_extension = format!("{}.md", path_part);
    resolve_file(root, from, &with_extension, index)
}

fn resolve_embed_target(
    root: &Path,
    from: &Path,
    target: &str,
    index: &VaultIndex,
) -> Option<PathBuf> {
    if let Some(found) = resolve_file(root, from, target, index) {
        return Some(found);
    }
    // Also allow embedded notes whose names contain periods, including
    // names that happen to end in a familiar suffix such as ".pdf".
    if let Some(found) = resolve_markdown_target(root, from, target, index) {
        return Some(found);
    }
    let path_part = target.split('#').next().unwrap_or(target).trim();
    let decoded = percent_decode_str(path_part).decode_utf8().ok()?;
    let normalized = decoded.replace('\\', "/");
    let requested = Path::new(&normalized);
    let known_extension = requested
        .extension()
        .and_then(|value| value.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "svg"
                    | "webp"
                    | "avif"
                    | "bmp"
                    | "ico"
                    | "tif"
                    | "tiff"
                    | "pdf"
                    | "md"
                    | "txt"
                    | "html"
                    | "htm"
                    | "css"
                    | "js"
                    | "mp3"
                    | "wav"
                    | "ogg"
                    | "mp4"
                    | "webm"
                    | "mov"
                    | "json"
                    | "xml"
            )
        })
        .unwrap_or(false);
    if known_extension {
        return None;
    }

    let requested_name = requested.file_name()?.to_string_lossy().to_lowercase();
    let requested_parent = requested.parent().unwrap_or(Path::new(""));
    let note_parent = from.parent().unwrap_or(Path::new(""));
    let preferred_parent = if requested_parent.as_os_str().is_empty() {
        note_parent
    } else {
        requested_parent
    };
    let matches = index.by_stem.get(&requested_name)?;
    let same_folder = matches.iter().find(|candidate| {
        let relative = candidate.strip_prefix(root).unwrap_or(candidate);
        relative
            .parent()
            .map(|parent| {
                normalized_rel(parent).eq_ignore_ascii_case(&normalized_rel(preferred_parent))
            })
            .unwrap_or(false)
    });
    same_folder
        .and_then(|candidate| {
            matches
                .iter()
                .find(|path| is_image(path) && path.parent() == candidate.parent())
                .cloned()
        })
        .or_else(|| same_folder.cloned())
        .or_else(|| matches.iter().find(|path| is_image(path)).cloned())
        .or_else(|| matches.first().cloned())
}

fn resolve_wiki_target(
    root: &Path,
    from: &Path,
    target: &str,
    index: &VaultIndex,
) -> Option<PathBuf> {
    resolve_markdown_target(root, from, target, index)
}

fn build_backlink_index(
    root: &Path,
    index: &VaultIndex,
) -> (
    HashMap<PathBuf, Vec<PathBuf>>,
    HashMap<PathBuf, Vec<PathBuf>>,
) {
    let mut by_target: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut by_source: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for source_path in index.files.iter().filter(|path| is_markdown(path)) {
        let Ok(source) = fs::read_to_string(source_path) else {
            continue;
        };
        let relative_source = source_path.strip_prefix(root).unwrap_or(source_path);
        let mut targets_from_note = HashSet::new();

        for captures in wikilink_regex().captures_iter(&source) {
            let raw = captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|m| m.as_str())
                .unwrap_or("");
            let target = raw.split('|').next().unwrap_or("").trim();
            if target.is_empty() {
                continue;
            }
            if let Some(target_path) = resolve_wiki_target(root, relative_source, target, index) {
                if is_markdown(&target_path) && target_path.as_path() != source_path.as_path() {
                    targets_from_note.insert(target_path);
                }
            }
        }

        for captures in markdown_link_regex().captures_iter(&source) {
            let raw = captures.get(2).map(|m| m.as_str()).unwrap_or("");
            let target = markdown_destination(raw);
            if target.is_empty()
                || target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
                || target.starts_with("data:")
                || target.starts_with('/')
                || target.starts_with('#')
            {
                continue;
            }
            if let Some(target_path) = resolve_file(root, relative_source, &target, index) {
                if is_markdown(&target_path) && target_path.as_path() != source_path.as_path() {
                    targets_from_note.insert(target_path);
                }
            }
        }

        let mut targets: Vec<PathBuf> = targets_from_note.into_iter().collect();
        targets.sort();
        for target_path in &targets {
            by_target
                .entry(target_path.clone())
                .or_default()
                .push(source_path.clone());
        }
        by_source.insert(source_path.clone(), targets);
    }
    for sources in by_target.values_mut() {
        sources.sort();
        sources.dedup();
    }
    (by_target, by_source)
}

fn render_link_list(root: &Path, links: &[PathBuf], empty: &str) -> String {
    if links.is_empty() {
        return format!("<span class=\"muted\">{}</span>", empty);
    }
    let mut output = String::from("<div class=\"backlink-list\">");
    for path in links {
        let relative = path
            .strip_prefix(root)
            .map(normalized_rel)
            .unwrap_or_default();
        let encoded = url_path(&relative);
        let label = path.file_stem().unwrap_or_default().to_string_lossy();
        output.push_str(&format!(
            "<a class=\"backlink-item\" href=\"/note/{}\" title=\"Open {}\">{}</a>",
            encoded,
            esc(&label),
            esc(&label)
        ));
    }
    output.push_str("</div>");
    output
}

fn render_backlinks(root: &Path, backlinks: &[PathBuf], outgoing: &[PathBuf]) -> String {
    format!(
        "<section id=\"backlinks\" class=\"backlinks\"><table class=\"link-relations\"><thead><tr><th>Backlinks</th><th>Outgoing links</th></tr></thead><tbody><tr><td>{}</td><td>{}</td></tr></tbody></table></section>",
        render_link_list(root, backlinks, "No backlinks"),
        render_link_list(root, outgoing, "No outgoing links")
    )
}

fn is_video_target(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("https://www.youtube.com/")
        || lower.starts_with("https://youtube.com/")
        || lower.starts_with("https://youtu.be/")
        || lower.starts_with("https://vimeo.com/")
        || [".mp4", ".webm", ".mov", ".m4v", ".ogv"].iter().any(|ext| {
            lower
                .split(|ch| ch == '?' || ch == '#')
                .next()
                .unwrap_or(&lower)
                .ends_with(ext)
        })
}

fn relative_url(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(url_path(&normalized_rel(relative)))
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "avif" | "bmp" | "ico" | "tif" | "tiff"
    )
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn is_base(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("base"))
        .unwrap_or(false)
}

fn is_canvas(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("canvas"))
        .unwrap_or(false)
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Guards against pathological notes that embed themselves, each other, or
/// form very large diamond-shaped embed graphs.
const MAX_EMBED_DEPTH: usize = 8;
const MAX_EMBED_BYTES: usize = 4 * 1024 * 1024;

struct EmbedScope {
    /// Canonical paths that are currently being rendered, used to stop direct
    /// and indirect recursion.
    stack: Vec<PathBuf>,
    depth: usize,
    /// Total embedded source bytes allowed for one top-level note.
    budget: usize,
    next_token: usize,
}

impl EmbedScope {
    fn new(root: &Path, relative_note: &Path) -> Self {
        let mut stack = Vec::new();
        if let Ok(canonical) = root.join(relative_note).canonicalize() {
            stack.push(canonical);
        }
        Self {
            stack,
            depth: 0,
            budget: MAX_EMBED_BYTES,
            next_token: 0,
        }
    }

    fn token(&mut self) -> String {
        self.next_token += 1;
        format!("VLXEMBEDX{}X", self.next_token)
    }
}

/// Short, safe explanation inserted in place of an embed that cannot be expanded.
fn embed_notice(root: &Path, path: &Path, title: &str, message: &str) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let encoded = url_path(&normalized_rel(relative));
    format!(
        "<section class=\"note-embed note-embed-error\"><div class=\"note-embed-title\"><a href=\"/note/{}\">{}</a></div><p>{}</p></section>",
        encoded,
        esc(title),
        esc(message)
    )
}

/// Render the body of an embedded Markdown note. The calling scope keeps the
/// chain of notes being expanded, so a note can never embed itself directly or
/// through another note.
fn embed_note(root: &Path, index: &VaultIndex, path: &Path, scope: &mut EmbedScope) -> String {
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if scope.depth >= MAX_EMBED_DEPTH {
        return embed_notice(root, path, &title, "Embed depth limit reached.");
    }
    if scope.stack.iter().any(|entry| entry == &canonical) {
        return embed_notice(root, path, &title, "Recursive embed stopped.");
    }
    let Ok(metadata) = fs::metadata(path) else {
        return embed_notice(root, path, &title, "Unable to read embedded note.");
    };
    if metadata.len() > scope.budget as u64 {
        return embed_notice(root, path, &title, "Embed size limit reached.");
    }
    let Ok(source) = fs::read_to_string(path) else {
        return embed_notice(root, path, &title, "Unable to read embedded note.");
    };
    if source.len() > scope.budget {
        return embed_notice(root, path, &title, "Embed size limit reached.");
    }
    scope.budget = scope.budget.saturating_sub(source.len());
    scope.stack.push(canonical);
    scope.depth += 1;

    let relative = path.strip_prefix(root).unwrap_or(path);
    let (_properties, body) = split_frontmatter(&source);
    let content = render_body_with_embeds(root, index, relative, body, scope);

    scope.depth -= 1;
    scope.stack.pop();
    let encoded = url_path(&normalized_rel(relative));
    format!(
        "<section class=\"note-embed\"><div class=\"note-embed-title\"><a href=\"/note/{}\">{}</a></div><div class=\"note-embed-body\">{}</div></section>",
        encoded,
        esc(&title),
        content
    )
}

/// True when a note embeds another Markdown note. Such notes are not cached
/// because their rendered HTML depends on other files. Image-only embeds do
/// not reference changeable content, so they can still be cached.
fn has_markdown_embed(root: &Path, index: &VaultIndex, relative_note: &Path, source: &str) -> bool {
    if !source.contains("![[") {
        return false;
    }
    wikilink_regex().captures_iter(source).any(|captures| {
        let Some(embedded) = captures.get(1) else {
            return false;
        };
        let target = embedded.as_str().split('|').next().unwrap_or("").trim();
        resolve_embed_target(root, relative_note, target, index)
            .map(|path| is_markdown(&path))
            .unwrap_or(false)
    })
}

fn rewrite_wikilinks<'a>(
    source: &'a str,
    root: &Path,
    index: &VaultIndex,
    relative_note: &Path,
    scope: &mut EmbedScope,
    embeds: &mut Vec<(String, String)>,
) -> Cow<'a, str> {
    if !source.contains("[[") {
        return Cow::Borrowed(source);
    }
    wikilink_regex()
        .replace_all(source, |captures: &regex::Captures| {
            let embedded = captures.get(1).is_some();
            let raw = captures
                .get(1)
                .or_else(|| captures.get(2))
                .unwrap()
                .as_str();
            let mut parts = raw.splitn(2, '|');
            let target = parts.next().unwrap_or("").trim();
            let option = parts.next().unwrap_or("").trim();
            let fragment = target
                .find('#')
                .map(|index| &target[index..])
                .unwrap_or("");

            if let Some(path) = if embedded { resolve_embed_target(root, relative_note, target, index) } else { resolve_wiki_target(root, relative_note, target, index) } {
                // Markdown embeds are expanded after the surrounding Markdown
                // has been rendered. A token keeps the embed out of the TOC and
                // lets nested formatting render normally.
                if embedded && is_markdown(&path) {
                    let token = scope.token();
                    let html = embed_note(root, index, &path, scope);
                    embeds.push((token.clone(), html));
                    return token;
                }
                let Some(encoded) = relative_url(root, &path) else {
                    return format!("<span class=\"missing\">{}</span>", esc(target));
                };
                if embedded {
                    if is_image(&path) {
                        let filename = path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| target.to_string());
                        let (alt, width) = if option.is_empty() {
                            (filename, None)
                        } else if option.chars().all(|ch| ch.is_ascii_digit()) {
                            (filename, Some(option.to_string()))
                        } else {
                            (option.to_string(), None)
                        };
                        let style = width
                            .map(|value| format!(" style=\"width:{}px;max-width:100%\"", value))
                            .unwrap_or_default();
                        format!(
                            "<img class=\"vault-image\" src=\"/asset/{}\" alt=\"{}\" loading=\"lazy\"{}>",
                            encoded,
                            esc(&alt),
                            style
                        )
                    } else if is_base(&path) {
                        format!("<a href=\"/base/{}\">{}</a>", encoded, esc(if option.is_empty() { target } else { option }))
                    } else if is_canvas(&path) {
                        format!("<a href=\"/canvas/{}\">{}</a>", encoded, esc(if option.is_empty() { target } else { option }))
                    } else {
                        let route = if is_image(&path) { "/image/" } else { "/file/" };
                        let target_attr = if is_pdf(&path) { " target=\"_blank\" rel=\"noopener\"" } else { "" };
                        format!(
                            "<a href=\"{}{}\"{}>{}</a>",
                            route,
                            encoded,
                            target_attr,
                            esc(if option.is_empty() { target } else { option })
                        )
                    }
                } else if path
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(|value| value.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
                {
                    format!(
                        "[{}](/note/{}{})",
                        esc(if option.is_empty() { target } else { option }),
                        encoded,
                        fragment
                    )
                } else if is_pdf(&path) {
                    format!(
                        "<a href=\"/file/{}{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                        encoded,
                        fragment,
                        esc(if option.is_empty() { target } else { option })
                    )
                } else if is_base(&path) {
                    format!("[{}](/base/{})", esc(if option.is_empty() { target } else { option }), encoded)
                } else if is_canvas(&path) {
                    format!("[{}](/canvas/{})", esc(if option.is_empty() { target } else { option }), encoded)
                } else if is_image(&path) {
                    format!("[{}](/image/{})", esc(if option.is_empty() { target } else { option }), encoded)
                } else {
                    format!(
                        "[{}](/file/{}{})",
                        esc(if option.is_empty() { target } else { option }),
                        encoded,
                        fragment
                    )
                }
            } else {
                format!(
                    "<span class=\"missing\">{}</span>",
                    esc(if option.is_empty() { target } else { option })
                )
            }
        })
}

fn local_vault_route(target: &str) -> Option<(String, String)> {
    let target = target.trim();
    let remainder = if let Some(value) = target.strip_prefix("http://") {
        value
    } else if let Some(value) = target.strip_prefix("https://") {
        value
    } else {
        return None;
    };
    let slash = remainder.find('/')?;
    let host = &remainder[..slash];
    if !(host.starts_with("127.0.0.1:")
        || host.starts_with("localhost:")
        || host == "127.0.0.1"
        || host == "localhost")
    {
        return None;
    }
    let route = &remainder[slash..];
    let (kind, encoded) = if let Some(value) = route.strip_prefix("/file/") {
        ("file", value)
    } else if let Some(value) = route.strip_prefix("/note/") {
        ("note", value)
    } else {
        return None;
    };
    let without_query = encoded.split('?').next().unwrap_or(encoded);
    let (encoded_path, fragment) = without_query.split_once('#').unwrap_or((without_query, ""));
    Some((kind.to_string(), format!("{}{}", encoded_path, fragment)))
}

fn rewrite_markdown_paths<'a>(
    source: &'a str,
    root: &Path,
    index: &VaultIndex,
    relative_note: &Path,
) -> Cow<'a, str> {
    if !source.contains("![") && !source.contains("](") {
        return Cow::Borrowed(source);
    }
    let with_images = if source.contains("![") {
        markdown_image_regex().replace_all(source, |captures: &regex::Captures| {
            let alt = captures.get(1).map(|value| value.as_str()).unwrap_or("");
            let raw_target = captures.get(2).map(|value| value.as_str()).unwrap_or("");
            let target = markdown_destination(raw_target);
            // Markdown image syntax is sometimes used for video embeds. Browsers do
            // not render YouTube/Vimeo URLs (or video files) as <img>; make these
            // ordinary links so the target opens naturally instead of showing a
            // broken image placeholder.
            if is_video_target(&target) {
                return format!("[{}]({})", alt, target);
            }
            if let Some((_kind, local_path)) = local_vault_route(&target) {
                if let Some(path) = resolve_file(root, relative_note, &local_path, index) {
                    if let Some(encoded) = relative_url(root, &path) {
                        return format!("![{}](/asset/{})", alt, encoded);
                    }
                }
            }
            if target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("data:")
                || target.starts_with('/')
                || target.starts_with('#')
            {
                return captures.get(0).unwrap().as_str().to_string();
            }
            if let Some(path) = resolve_file(root, relative_note, &target, index) {
                if let Some(encoded) = relative_url(root, &path) {
                    return format!("![{}](/asset/{})", alt, encoded);
                }
            }
            captures.get(0).unwrap().as_str().to_string()
        })
    } else {
        Cow::Borrowed(source)
    };

    if !with_images.contains("](") {
        return with_images;
    }
    let linked = markdown_link_regex().replace_all(&with_images, |captures: &regex::Captures| {
        let label = captures.get(1).map(|value| value.as_str()).unwrap_or("");
        let raw_target = captures.get(2).map(|value| value.as_str()).unwrap_or("");
        let target = markdown_destination(raw_target);
        if let Some((_kind, local_path)) = local_vault_route(&target) {
            if let Some(path) = resolve_file(root, relative_note, &local_path, index) {
                if let Some(encoded) = relative_url(root, &path) {
                    let fragment = local_path.find('#').map(|i| &local_path[i..]).unwrap_or("");
                    let is_md = path
                        .extension()
                        .and_then(|v| v.to_str())
                        .map(|v| v.eq_ignore_ascii_case("md"))
                        .unwrap_or(false);
                    let route = if is_md {
                        "note/"
                    } else if is_base(&path) {
                        "base/"
                    } else if is_canvas(&path) {
                        "canvas/"
                    } else if is_image(&path) {
                        "image/"
                    } else {
                        "file/"
                    };
                    let display = if label == target {
                        path.file_stem()
                            .or_else(|| path.file_name())
                            .map(|value| value.to_string_lossy().to_string())
                            .unwrap_or_else(|| label.to_string())
                    } else {
                        label.to_string()
                    };
                    if is_pdf(&path) {
                        return format!(
                            "<a href=\"/{}{}{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                            route,
                            encoded,
                            fragment,
                            esc(&display)
                        );
                    }
                    return format!("[{}](/{}{}{})", esc(&display), route, encoded, fragment);
                }
            }
        }
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
            || target.starts_with("data:")
            || target.starts_with('/')
            || target.starts_with('#')
        {
            return captures.get(0).unwrap().as_str().to_string();
        }
        if let Some(path) = resolve_markdown_target(root, relative_note, &target, index) {
            if let Some(encoded) = relative_url(root, &path) {
                let fragment = target.find('#').map(|index| &target[index..]).unwrap_or("");
                if is_markdown(&path) {
                    return format!("[{}](/note/{}{})", label, encoded, fragment);
                }
                if is_base(&path) {
                    return format!("[{}](/base/{})", label, encoded);
                }
                if is_canvas(&path) {
                    return format!("[{}](/canvas/{})", label, encoded);
                }
                if is_image(&path) {
                    return format!("[{}](/image/{})", label, encoded);
                }
                if is_pdf(&path) {
                    return format!(
                        "<a href=\"/file/{}{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                        encoded, fragment, label
                    );
                }
                return format!("[{}](/file/{}{})", label, encoded, fragment);
            }
        }
        captures.get(0).unwrap().as_str().to_string()
    });
    let unchanged = matches!(&with_images, Cow::Borrowed(_)) && matches!(&linked, Cow::Borrowed(_));
    let linked = linked.into_owned();
    if unchanged {
        Cow::Borrowed(source)
    } else {
        Cow::Owned(linked)
    }
}

fn split_frontmatter<'a>(source: &'a str) -> (String, &'a str) {
    let normalized = source.trim_start_matches('\u{feff}');
    let mut lines = normalized.split_inclusive('\n');
    let Some(first_line) = lines.next() else {
        return (String::new(), source);
    };
    if first_line.trim() != "---" {
        return (String::new(), source);
    }

    let mut properties = Vec::new();
    let mut body_start = first_line.len();
    let mut found_end = false;
    for line in lines {
        body_start += line.len();
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." {
            found_end = true;
            break;
        }
        properties.push(line.trim_end_matches(&['\r', '\n'][..]).to_string());
    }
    if !found_end {
        return (String::new(), source);
    }

    let mut rendered = String::new();
    let mut count = 0usize;
    for line in properties {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || line
                .chars()
                .next()
                .map(char::is_whitespace)
                .unwrap_or(false)
            || trimmed.starts_with('-')
        {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches(&['"', '\''][..]);
            if key.is_empty() {
                continue;
            }
            count += 1;
            let short_value: String = value.chars().take(500).collect();
            rendered.push_str(&format!(
                "<div class=\"property\"><dt>{}</dt><dd>{}</dd></div>",
                esc(key),
                esc(&short_value)
            ));
        }
    }

    let body = &normalized[body_start..];
    if count == 0 {
        (String::new(), body)
    } else {
        (
            format!(
                "<details class=\"properties\"><summary>Properties <span>{}</span></summary><dl>{}</dl></details>",
                count, rendered
            ),
            body,
        )
    }
}

fn slug(value: &str) -> String {
    let mut output = String::new();
    let mut dash = false;
    for character in value.to_lowercase().chars() {
        if character.is_alphanumeric() {
            output.push(character);
            dash = false;
        } else if !dash && !output.is_empty() {
            output.push('-');
            dash = true;
        }
    }
    output.trim_end_matches('-').to_string()
}

fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options
}

fn render_markdown_fragment(source: &str) -> String {
    if !source.contains("[!") {
        let parser = Parser::new_ext(source, markdown_options());
        let mut rendered = String::with_capacity(source.len().saturating_add(source.len() / 3));
        html::push_html(&mut rendered, parser);
        return rendered;
    }
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut transformed = String::new();
    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !trimmed.starts_with('>') {
            transformed.push_str(lines[i]);
            i += 1;
            continue;
        }

        let start = i;
        let mut quoted = Vec::new();
        while i < lines.len() {
            let current = lines[i].trim_start();
            if !current.starts_with('>') {
                break;
            }
            let content = current
                .strip_prefix('>')
                .unwrap_or("")
                .strip_prefix(' ')
                .unwrap_or_else(|| current.strip_prefix('>').unwrap_or(""));
            quoted.push(content.trim_end_matches(&['\r', '\n'][..]));
            i += 1;
        }

        let first = quoted.iter().position(|line| !line.trim().is_empty());
        let callout_info = first.and_then(|idx| {
            let line = quoted[idx].trim();
            let rest = line.strip_prefix("[!")?;
            let close = rest.find(']')?;
            let kind = rest[..close].trim().to_ascii_lowercase();
            if kind.is_empty() {
                return None;
            }
            let mut tail = rest[close + 1..].trim().to_string();
            let fold = if let Some(v) = tail.strip_prefix('-') {
                tail = v.trim().to_string();
                Some(false)
            } else if let Some(v) = tail.strip_prefix('+') {
                tail = v.trim().to_string();
                Some(true)
            } else {
                None
            };
            let title = if tail.is_empty() {
                kind.split('-')
                    .map(|part| {
                        let mut chars = part.chars();
                        chars
                            .next()
                            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                tail
            };
            Some((idx, kind, title, fold))
        });

        if let Some((header_idx, kind, title, fold)) = callout_info {
            let body_lines = quoted
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != header_idx)
                .map(|(_, line)| *line)
                .collect::<Vec<_>>();
            let body_source = body_lines.join("\n");
            let body_html = render_markdown_fragment(&body_source);
            let mut title_rendered = String::new();
            html::push_html(
                &mut title_rendered,
                Parser::new_ext(&title, markdown_options()),
            );
            let title_rendered = title_rendered
                .trim()
                .strip_prefix("<p>")
                .unwrap_or(title_rendered.trim())
                .strip_suffix("</p>\n")
                .or_else(|| {
                    title_rendered
                        .trim()
                        .strip_prefix("<p>")
                        .unwrap_or(title_rendered.trim())
                        .strip_suffix("</p>")
                })
                .unwrap_or(title_rendered.trim())
                .to_string();
            let title_html = format!(
                "<div class=\"callout-title\"><span class=\"callout-icon\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#{}\"/></svg></span><span class=\"callout-label\">{}</span></div>",
                callout_icon(&kind),
                title_rendered
            );
            if let Some(open) = fold {
                let open_attr = if open { " open" } else { "" };
                transformed.push_str(&format!("<details class=\"callout callout-{}\"{}>\n<summary>{}</summary>\n<div class=\"callout-content\">\n{}\n</div>\n</details>\n\n", esc(&kind), open_attr, title_html, body_html));
            } else {
                transformed.push_str(&format!("<div class=\"callout callout-{}\">\n{}\n<div class=\"callout-content\">\n{}\n</div>\n</div>\n\n", esc(&kind), title_html, body_html));
            }
        } else {
            for line in &lines[start..i] {
                transformed.push_str(line);
            }
        }
    }

    let parser = Parser::new_ext(&transformed, markdown_options());
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    rendered
}

/// Symbol identifiers from the inline SVG sprite in `assets/page.html`.
fn callout_icon(kind: &str) -> &'static str {
    match kind.split('-').next().unwrap_or(kind) {
        "note" => "i-file-text",
        "abstract" | "summary" | "tldr" => "i-list",
        "info" => "i-info",
        "todo" => "i-check",
        "tip" | "hint" | "important" => "i-bulb",
        "success" | "check" | "done" => "i-check",
        "question" | "help" | "faq" => "i-help",
        "warning" | "caution" | "attention" => "i-alert",
        "failure" | "fail" | "missing" | "danger" | "error" => "i-x",
        "bug" => "i-bug",
        "example" => "i-list",
        "quote" | "cite" => "i-quote",
        _ => "i-info",
    }
}

/// Render a note body to HTML and collect any Markdown embeds that still need
/// to be substituted. Callers decide when to expand them so embedded headings
/// do not leak into the top-level table of contents.
fn render_body(
    root: &Path,
    index: &VaultIndex,
    relative_note: &Path,
    body: &str,
    scope: &mut EmbedScope,
) -> (String, Vec<(String, String)>) {
    let mut embeds = Vec::new();
    let rewritten_wiki = rewrite_wikilinks(body, root, index, relative_note, scope, &mut embeds);
    let rewritten = rewrite_markdown_paths(&rewritten_wiki, root, index, relative_note);
    let rendered = render_markdown_fragment(&rewritten);
    (rendered, embeds)
}

fn apply_embeds(mut rendered: String, embeds: Vec<(String, String)>) -> String {
    for (token, html) in embeds {
        let paragraph = format!("<p>{}</p>", token);
        if rendered.contains(&paragraph) {
            rendered = rendered.replace(&paragraph, &html);
        } else {
            // The token may sit inside a list item or another element.
            rendered = rendered.replace(&token, &html);
        }
    }
    rendered
}

fn render_body_with_embeds(
    root: &Path,
    index: &VaultIndex,
    relative_note: &Path,
    body: &str,
    scope: &mut EmbedScope,
) -> String {
    let (rendered, embeds) = render_body(root, index, relative_note, body, scope);
    apply_embeds(rendered, embeds)
}

fn render_note(
    root: &Path,
    index: &VaultIndex,
    relative_note: &Path,
    title: &str,
    source: &str,
) -> String {
    let (properties, body) = split_frontmatter(source);
    let mut scope = EmbedScope::new(root, relative_note);
    let (rendered, embeds) = render_body(root, index, relative_note, body, &mut scope);

    let heading_pattern = heading_regex();
    let tag_pattern = html_tag_regex();
    struct TocHeading {
        level: usize,
        id: String,
        text: String,
    }
    let mut headings = Vec::<TocHeading>::new();
    let mut slug_counts = std::collections::HashMap::<String, usize>::new();
    let with_ids = heading_pattern
        .replace_all(&rendered, |captures: &regex::Captures| {
            let heading_text = tag_pattern.replace_all(&captures[2], "").to_string();
            let base = slug(&heading_text);
            let count = slug_counts.entry(base.clone()).or_insert(0);
            *count += 1;
            let id = if *count == 1 {
                base
            } else {
                format!("{}-{}", base, count)
            };
            let level = captures[1].parse::<usize>().unwrap_or(1).clamp(1, 6);
            headings.push(TocHeading {
                level,
                id: id.clone(),
                text: heading_text,
            });
            format!(
                "<h{} id=\"{}\">{}</h{}>",
                &captures[1], id, &captures[2], &captures[1]
            )
        })
        .to_string();
    let with_ids = apply_embeds(with_ids, embeds);

    // The table of contents is built after the article so each entry knows
    // whether it has nested headings. Entries carry their level and id so the
    // client can collapse sections and highlight the heading in view.
    let mut toc = String::new();
    for (position, heading) in headings.iter().enumerate() {
        let has_children = headings
            .get(position + 1)
            .map(|next| next.level > heading.level)
            .unwrap_or(false);
        let guides = (1..heading.level).map(|_| "<i></i>").collect::<String>();
        toc.push_str(&format!(
            "<div class=\"toc-entry toc-level-{level}\" data-level=\"{level}\" data-id=\"{id}\"><button class=\"toc-toggle\" type=\"button\" data-toc-toggle aria-expanded=\"true\" aria-label=\"Collapse section\" style=\"visibility:{visibility}\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-chevron-right\"/></svg></button><a class=\"toc-link\" href=\"#{id}\"><span class=\"toc-guides\" aria-hidden=\"true\">{guides}</span><span class=\"toc-label\">{text}</span></a></div>",
            level = heading.level,
            id = esc(&heading.id),
            visibility = if has_children { "visible" } else { "hidden" },
            guides = guides,
            text = esc(&heading.text)
        ));
    }

    let backlink_route = url_path(&normalized_rel(relative_note));
    let mut output = String::with_capacity(with_ids.len() + toc.len() + 1400);
    output.push_str("<div class=\"layout\"><main><h1 class=\"note-title\">");
    output.push_str(&esc(title));
    output.push_str("</h1>");
    output.push_str(&properties);
    output.push_str("<article>");
    output.push_str(&with_ids);
    output.push_str("</article>");
    output.push_str("<section id=\"backlinks\" class=\"backlinks\" data-note-path=\"");
    output.push_str(&esc(&backlink_route));
    output.push_str("\"><table class=\"link-relations\"><thead><tr><th>Backlinks</th><th>Outgoing links</th></tr></thead><tbody><tr><td class=\"muted\">Loading…</td><td class=\"muted\">Loading…</td></tr></tbody></table></section></main><aside aria-label=\"Table of contents\"><div class=\"toc-heading\"><b>On this page</b><div class=\"toc-actions\"><button class=\"icon-button\" data-toc-resize=\"larger\" type=\"button\" title=\"Increase right sidebar width\" aria-label=\"Increase right sidebar width\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-chevron-left\"/></svg></button><button class=\"icon-button\" data-toc-resize=\"smaller\" type=\"button\" title=\"Decrease right sidebar width\" aria-label=\"Decrease right sidebar width\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-chevron-right\"/></svg></button><button class=\"icon-button\" data-toc-level-one=\"true\" type=\"button\" title=\"Show only level 1 headings\" aria-label=\"Show only level 1 headings\" aria-pressed=\"false\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-levels\"/></svg></button><button class=\"icon-button\" data-wrap-control=\"right\" type=\"button\" title=\"Toggle wrapping in table of contents\" aria-label=\"Toggle wrapping in table of contents\" aria-pressed=\"false\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-wrap\"/></svg></button><button class=\"icon-button\" id=\"hide-right-sidebar\" type=\"button\" title=\"Hide right sidebar\" aria-label=\"Hide right sidebar\" data-command=\"toggle-right-sidebar\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-panel-right-close\"/></svg></button></div></div><input id=\"heading-search\" type=\"search\" placeholder=\"Find headings…\" autocomplete=\"off\" aria-label=\"Search headings in this note\"><div class=\"toc\">");
    if toc.is_empty() {
        output.push_str("<span class=\"muted\">No headings</span>");
    } else {
        output.push_str(&toc);
    }
    output.push_str("</div></aside></div>");
    output
}

fn frontmatter_property(path: &Path, wanted: &str) -> String {
    if !is_markdown(path) {
        return String::new();
    }
    let Ok(file) = fs::File::open(path) else {
        return String::new();
    };
    let mut source = String::new();
    if file.take(256 * 1024).read_to_string(&mut source).is_err() {
        return String::new();
    }
    let source = source.trim_start_matches('\u{feff}');
    let mut lines = source.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            if key.trim().eq_ignore_ascii_case(wanted) {
                return value
                    .trim()
                    .trim_matches(|ch| ch == '"' || ch == '\'')
                    .to_string();
            }
        }
    }
    String::new()
}

fn base_field(path: &Path, root: &Path, field: &str) -> String {
    let relative = path
        .strip_prefix(root)
        .map(normalized_rel)
        .unwrap_or_default();
    let name = path
        .file_stem()
        .or_else(|| path.file_name())
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    match field.trim() {
        "file.name" => name,
        "file.path" => relative,
        "file.folder" => path
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(normalized_rel)
            .unwrap_or_default(),
        "file.ext" => ext,
        "file.size" => fs::metadata(path)
            .map(|m| m.len().to_string())
            .unwrap_or_default(),
        "file.mtime" => fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default(),
        value if value.starts_with("note.") => frontmatter_property(path, &value[5..]),
        value if !value.contains('.') => frontmatter_property(path, value),
        _ => String::new(),
    }
}

fn render_base(root: &Path, index: &VaultIndex, path: &Path) -> String {
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let source = fs::read_to_string(path).unwrap_or_default();
    let base: serde_yaml::Value = serde_yaml::from_str(&source).unwrap_or(serde_yaml::Value::Null);
    let first_view = base
        .get("views")
        .and_then(serde_yaml::Value::as_sequence)
        .and_then(|views| views.first());
    let view_name = first_view
        .and_then(|view| view.get("name"))
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("All files");
    let view_type = first_view
        .and_then(|view| view.get("type"))
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("table");
    let fields: Vec<String> = first_view
        .and_then(|view| view.get("order"))
        .and_then(serde_yaml::Value::as_sequence)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_yaml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .filter(|values: &Vec<String>| !values.is_empty())
        .unwrap_or_else(|| {
            vec![
                "file.name".into(),
                "file.path".into(),
                "file.ext".into(),
                "file.size".into(),
            ]
        });
    let limit = first_view
        .and_then(|view| view.get("limit"))
        .and_then(serde_yaml::Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 500) as usize;
    let has_filters =
        base.get("filters").is_some() || first_view.and_then(|view| view.get("filters")).is_some();
    let mut html_out = format!("<div class=\"layout single\"><main><h1 class=\"note-title\">{}</h1><section class=\"base-view\"><p class=\"base-note\">{} · Showing up to {} files</p>", esc(&title), esc(view_name), limit);
    if has_filters {
        html_out.push_str("<p class=\"base-note\">This lightweight viewer displays the base columns but does not evaluate Bases filter expressions or formulas.</p>");
    }
    let files: Vec<&PathBuf> = index
        .files
        .iter()
        .filter(|candidate| candidate.as_path() != path)
        .take(limit)
        .collect();
    if view_type.eq_ignore_ascii_case("list") {
        html_out.push_str("<ul class=\"base-list\">");
        for file in &files {
            let relative = file
                .strip_prefix(root)
                .map(normalized_rel)
                .unwrap_or_default();
            let encoded = url_path(&relative);
            let href = if is_markdown(file) {
                format!("/note/{}", encoded)
            } else if is_image(file) {
                format!("/image/{}", encoded)
            } else if is_base(file) {
                format!("/base/{}", encoded)
            } else if is_canvas(file) {
                format!("/canvas/{}", encoded)
            } else {
                format!("/file/{}", encoded)
            };
            let label = file
                .file_stem()
                .or_else(|| file.file_name())
                .map(|v| v.to_string_lossy().to_string())
                .unwrap_or_default();
            let target = if is_pdf(file) {
                " target=\"_blank\" rel=\"noopener\""
            } else {
                ""
            };
            html_out.push_str(&format!(
                "<li><a href=\"{}\"{}>{}</a></li>",
                href,
                target,
                esc(&label)
            ));
        }
        html_out.push_str("</ul>");
    } else if view_type.eq_ignore_ascii_case("cards") || view_type.eq_ignore_ascii_case("kanban") {
        html_out.push_str("<div class=\"base-cards\">");
        for file in &files {
            let relative = file
                .strip_prefix(root)
                .map(normalized_rel)
                .unwrap_or_default();
            let encoded = url_path(&relative);
            let href = if is_markdown(file) {
                format!("/note/{}", encoded)
            } else if is_image(file) {
                format!("/image/{}", encoded)
            } else if is_base(file) {
                format!("/base/{}", encoded)
            } else if is_canvas(file) {
                format!("/canvas/{}", encoded)
            } else {
                format!("/file/{}", encoded)
            };
            let label = file
                .file_stem()
                .or_else(|| file.file_name())
                .map(|v| v.to_string_lossy().to_string())
                .unwrap_or_default();
            let target = if is_pdf(file) {
                " target=\"_blank\" rel=\"noopener\""
            } else {
                ""
            };
            html_out.push_str(&format!("<div class=\"base-card\"><a href=\"{}\"{}>{}</a><div class=\"base-note\">{}</div></div>", href, target, esc(&label), esc(&relative)));
        }
        html_out.push_str("</div>");
    } else {
        html_out.push_str("<div style=\"overflow-x:auto\"><table class=\"base-table\"><thead><tr>");
        for field in &fields {
            html_out.push_str(&format!(
                "<th>{}</th>",
                esc(field.strip_prefix("note.").unwrap_or(field))
            ));
        }
        html_out.push_str("</tr></thead><tbody>");
        for file in &files {
            let relative = file
                .strip_prefix(root)
                .map(normalized_rel)
                .unwrap_or_default();
            let encoded = url_path(&relative);
            let href = if is_markdown(file) {
                format!("/note/{}", encoded)
            } else if is_image(file) {
                format!("/image/{}", encoded)
            } else if is_base(file) {
                format!("/base/{}", encoded)
            } else if is_canvas(file) {
                format!("/canvas/{}", encoded)
            } else {
                format!("/file/{}", encoded)
            };
            html_out.push_str("<tr>");
            for (column_index, field) in fields.iter().enumerate() {
                let value = base_field(file, root, field);
                if field == "file.name" || column_index == 0 {
                    let target = if is_pdf(file) {
                        " target=\"_blank\" rel=\"noopener\""
                    } else {
                        ""
                    };
                    let label = if value.is_empty() {
                        file.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    } else {
                        value
                    };
                    html_out.push_str(&format!(
                        "<td><a href=\"{}\"{}>{}</a></td>",
                        href,
                        target,
                        esc(&label)
                    ));
                } else {
                    html_out.push_str(&format!("<td>{}</td>", esc(&value)));
                }
            }
            html_out.push_str("</tr>");
        }
        html_out.push_str("</tbody></table></div>");
    }
    html_out.push_str("</section></main></div>");
    html_out
}

fn render_image(root: &Path, path: &Path) -> String {
    let title = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let relative = path
        .strip_prefix(root)
        .map(normalized_rel)
        .unwrap_or_default();
    let encoded = url_path(&relative);
    format!("<div class=\"layout single\"><main><h1 class=\"note-title\">{}</h1><article class=\"file-preview\"><img src=\"/asset/{}\" alt=\"{}\" decoding=\"async\"></article></main></div>", esc(&title), encoded, esc(&title))
}

/// Canvas files are rendered client-side from their JSON source so the board
/// can be panned and zoomed without shipping a large HTML tree from Rust.
fn render_canvas(root: &Path, path: &Path) -> String {
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let relative = path
        .strip_prefix(root)
        .map(normalized_rel)
        .unwrap_or_default();
    let encoded = url_path(&relative);
    format!(
        "<div class=\"layout single\"><main><h1 class=\"note-title\">{}</h1><div class=\"canvas-host\" id=\"canvas-host\" data-canvas-url=\"/canvas-data/{}\"><div class=\"canvas-status\">Loading canvas…</div></div></main></div>",
        esc(&title),
        encoded
    )
}

/// Symbol identifiers from the inline SVG sprite in `assets/page.html`.
fn file_icon(path: &Path) -> &'static str {
    if is_image(path) {
        "i-image"
    } else if is_markdown(path) {
        "i-file-text"
    } else if is_base(path) {
        "i-table"
    } else if is_canvas(path) {
        "i-canvas"
    } else {
        "i-file"
    }
}

fn nav(root: &Path, current: &Path) -> String {
    fn walk(root: &Path, dir: &Path, current: &Path, output: &mut String) {
        if let Ok(entries) = fs::read_dir(dir) {
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|entry| {
                let path = entry.path();
                let is_dir = path.is_dir();
                (!is_dir, entry.file_name())
            });
            for entry in entries {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_symlink() {
                    continue;
                }
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let relative_text = normalized_rel(relative);
                if path.is_dir() {
                    output.push_str(&format!(
                        "<details class=\"folder\" data-name=\"{}\" data-path=\"{}\"><summary><span class=\"entry-icon\"><svg class=\"icon folder-closed\" aria-hidden=\"true\"><use href=\"#i-folder\"/></svg><svg class=\"icon folder-open\" aria-hidden=\"true\"><use href=\"#i-folder-open\"/></svg></span><span class=\"entry-label\">{}</span></summary><div class=\"folder-children\">",
                        esc(&name.to_lowercase()),
                        esc(&relative_text.to_lowercase()),
                        esc(&name)
                    ));
                    walk(root, &path, current, output);
                    output.push_str("</div></details>");
                } else if path.is_file() {
                    let markdown_file = is_markdown(&path);
                    let canvas_file = is_canvas(&path);
                    let is_current = (markdown_file || canvas_file) && relative == current;
                    let destination = if markdown_file {
                        format!("/note/{}", url_path(&relative_text))
                    } else if is_image(&path) {
                        format!("/image/{}", url_path(&relative_text))
                    } else if is_base(&path) {
                        format!("/base/{}", url_path(&relative_text))
                    } else if canvas_file {
                        format!("/canvas/{}", url_path(&relative_text))
                    } else {
                        format!("/file/{}", url_path(&relative_text))
                    };
                    let target_attr = if is_pdf(&path) {
                        " target=\"_blank\" rel=\"noopener\""
                    } else {
                        ""
                    };
                    let label = if markdown_file || canvas_file {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    } else {
                        name.clone()
                    };
                    output.push_str(&format!(
                        "<a class=\"file{}\" data-name=\"{}\" data-path=\"{}\" href=\"{}\"{}><span class=\"entry-icon\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#{}\"/></svg></span><span class=\"entry-label\">{}</span></a>",
                        if is_current { " active" } else { "" },
                        esc(&label.to_lowercase()),
                        esc(&relative_text.to_lowercase()),
                        destination,
                        target_attr,
                        file_icon(&path),
                        esc(&label)
                    ));
                }
            }
        }
    }

    let mut output = String::new();
    walk(root, root, current, &mut output);
    output
}

const PAGE_TEMPLATE: &str = include_str!("assets/page.html");
const APP_CSS: &str = include_str!("assets/app.css");
const APP_JS: &str = include_str!("assets/app.js");

fn page(title: &str, content: &str, nav_html: &str, config_json: &str) -> String {
    PAGE_TEMPLATE
        .replace("__CSS__", APP_CSS)
        .replace("__JS__", APP_JS)
        .replace("__CONFIG__", config_json)
        .replace("__TITLE__", &esc(title))
        .replace("__NAV__", nav_html)
        .replace("__CONTENT__", content)
}
fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "json" => "application/json",
        "canvas" => "application/json; charset=utf-8",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}

fn send_text(request: tiny_http::Request, status: u16, body: String, mime: &str) {
    let mut response = Response::from_string(body).with_status_code(StatusCode(status));
    if let Ok(header) = Header::from_bytes("Content-Type", mime) {
        response = response.with_header(header);
    }
    let _ = request.respond(response);
}

fn send_inline_image(request: tiny_http::Request, path: &Path) {
    match fs::File::open(path) {
        Ok(file) => {
            // Stream image bytes from disk rather than loading large attachments
            // into a temporary Vec for every request.
            let mut response = Response::from_file(file);
            if let Ok(header) = Header::from_bytes("Content-Type", content_type(path)) {
                response = response.with_header(header);
            }
            if let Ok(header) = Header::from_bytes("Content-Disposition", "inline") {
                response = response.with_header(header);
            }
            if let Ok(header) = Header::from_bytes("X-Content-Type-Options", "nosniff") {
                response = response.with_header(header);
            }
            if let Ok(header) = Header::from_bytes("Cache-Control", "public, max-age=3600") {
                response = response.with_header(header);
            }
            let _ = request.respond(response);
        }
        Err(_) => send_text(
            request,
            500,
            "Unable to read image".into(),
            "text/plain; charset=utf-8",
        ),
    }
}

fn api_authorized(request: &tiny_http::Request, state: &AppState) -> bool {
    request.headers().iter().any(|header| {
        header
            .field
            .as_str()
            .as_str()
            .eq_ignore_ascii_case("x-vaultlite-token")
            && header.value.as_str() == state.token
    })
}

fn open_folder(path: &Path) -> std::io::Result<()> {
    let child = if cfg!(target_os = "windows") {
        process::Command::new("explorer").arg(path).spawn()
    } else if cfg!(target_os = "macos") {
        process::Command::new("open").arg(path).spawn()
    } else {
        process::Command::new("xdg-open").arg(path).spawn()
    };
    child.map(|_| ())
}

/// Local endpoints for actions that require filesystem or process access.
/// They are POST-only and require the per-session token embedded in the page,
/// which keeps other websites from triggering them through the localhost port.
fn respond_api(request: tiny_http::Request, state: &AppState, route: &str) {
    if !matches!(request.method(), tiny_http::Method::Post) {
        send_text(
            request,
            405,
            "Method not allowed".into(),
            "text/plain; charset=utf-8",
        );
        return;
    }
    if !api_authorized(&request, state) {
        send_text(
            request,
            403,
            "Invalid session token".into(),
            "text/plain; charset=utf-8",
        );
        return;
    }
    match route {
        "/api/open-config-folder" => {
            let folder = config::config_dir();
            let result = fs::create_dir_all(&folder).and_then(|_| open_folder(&folder));
            match result {
                Ok(()) => send_text(
                    request,
                    200,
                    "{\"ok\":true}".into(),
                    "application/json; charset=utf-8",
                ),
                Err(error) => send_text(
                    request,
                    500,
                    format!("{{\"ok\":false,\"error\":\"{}\"}}", esc(&error.to_string())),
                    "application/json; charset=utf-8",
                ),
            }
        }
        "/api/quit" => {
            send_text(
                request,
                200,
                "{\"ok\":true}".into(),
                "application/json; charset=utf-8",
            );
            // Let the response reach the browser before the process exits.
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(150));
                process::exit(0);
            });
        }
        _ => send_text(
            request,
            404,
            "Not found".into(),
            "text/plain; charset=utf-8",
        ),
    }
}

fn respond(request: tiny_http::Request, state: &AppState) {
    let url = request.url().split('?').next().unwrap_or("/").to_string();

    if url.starts_with("/api/") {
        respond_api(request, state, &url);
        return;
    }

    // Raw Markdown is used by the "Copy note" command.
    if let Some(encoded_path) = url.strip_prefix("/raw/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file() && is_markdown(&path) {
                match fs::read_to_string(&path) {
                    Ok(source) => {
                        send_text(request, 200, source, "text/plain; charset=utf-8");
                        return;
                    }
                    Err(_) => {
                        send_text(
                            request,
                            500,
                            "Unable to read note".into(),
                            "text/plain; charset=utf-8",
                        );
                        return;
                    }
                }
            }
        }
        send_text(
            request,
            404,
            "Not found".into(),
            "text/plain; charset=utf-8",
        );
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/backlinks/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if !is_markdown(&path) {
                send_text(
                    request,
                    404,
                    "Not a Markdown note".into(),
                    "text/plain; charset=utf-8",
                );
                return;
            }
            match state.backlinks.read() {
                Ok(index) if !index.ready => send_text(request, 202,
                    "<table class=\"link-relations\"><thead><tr><th>Backlinks</th><th>Outgoing links</th></tr></thead><tbody><tr><td class=\"muted\">Preparing index…</td><td class=\"muted\">Preparing index…</td></tr></tbody></table>".into(), "text/html; charset=utf-8"),
                Ok(index) => {
                    let sources = index.by_target.get(&path).map(Vec::as_slice).unwrap_or(&[]);
                    let outgoing = index.by_source.get(&path).map(Vec::as_slice).unwrap_or(&[]);
                    send_text(request, 200, render_backlinks(&state.root, sources, outgoing), "text/html; charset=utf-8");
                }
                Err(_) => send_text(request, 500, "Unable to read backlink index".into(), "text/plain; charset=utf-8"),
            }
        } else {
            send_text(
                request,
                404,
                "Not found".into(),
                "text/plain; charset=utf-8",
            );
        }
        return;
    }

    // Canvas JSON is fetched by the client-side board renderer.
    if let Some(encoded_path) = url.strip_prefix("/canvas-data/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file() && is_canvas(&path) {
                match fs::read_to_string(&path) {
                    Ok(source) => {
                        send_text(request, 200, source, "application/json; charset=utf-8");
                        return;
                    }
                    Err(_) => {
                        send_text(
                            request,
                            500,
                            "Unable to read canvas".into(),
                            "text/plain; charset=utf-8",
                        );
                        return;
                    }
                }
            }
        }
        send_text(
            request,
            404,
            "Not found".into(),
            "text/plain; charset=utf-8",
        );
        return;
    }
    // Fragment responses preserve the app shell and are used for quick in-page
    // navigation among notes, images, Bases views, and canvases.
    if let Some(fragment_route) = url.strip_prefix("/fragment/") {
        if let Some((kind, encoded_path)) = fragment_route.split_once('/') {
            if let Some(path) = safe_join(&state.root, encoded_path) {
                if !path.is_file() {
                    send_text(
                        request,
                        404,
                        "Not found".into(),
                        "text/plain; charset=utf-8",
                    );
                    return;
                }
                match kind {
                    "note" if is_markdown(&path) => match cached_note(state, &path) {
                        Ok((_title, content)) => {
                            send_text(request, 200, content, "text/html; charset=utf-8")
                        }
                        Err(_) => send_text(
                            request,
                            500,
                            "Unable to render note".into(),
                            "text/plain; charset=utf-8",
                        ),
                    },
                    "image" if is_image(&path) => {
                        let content = render_image(&state.root, &path);
                        send_text(request, 200, content, "text/html; charset=utf-8");
                    }
                    "base" if is_base(&path) => {
                        let content = render_base(&state.root, &state.index, &path);
                        send_text(request, 200, content, "text/html; charset=utf-8");
                    }
                    "canvas" if is_canvas(&path) => {
                        let content = render_canvas(&state.root, &path);
                        send_text(request, 200, content, "text/html; charset=utf-8");
                    }
                    _ => send_text(
                        request,
                        404,
                        "Not found".into(),
                        "text/plain; charset=utf-8",
                    ),
                }
            } else {
                send_text(
                    request,
                    404,
                    "Not found".into(),
                    "text/plain; charset=utf-8",
                );
            }
        } else {
            send_text(
                request,
                404,
                "Not found".into(),
                "text/plain; charset=utf-8",
            );
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/asset/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file() && is_image(&path) {
                send_inline_image(request, &path);
            } else {
                send_text(
                    request,
                    404,
                    "Not an image".into(),
                    "text/plain; charset=utf-8",
                );
            }
        } else {
            send_text(
                request,
                404,
                "Not found".into(),
                "text/plain; charset=utf-8",
            );
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/file/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file() {
                if is_markdown(&path) {
                    match cached_note(state, &path) {
                        Ok((title, content)) => {
                            let body = page(&title, &content, &state.nav_html, &state.config_json);
                            send_text(request, 200, body, "text/html; charset=utf-8");
                        }
                        Err(_) => send_text(
                            request,
                            500,
                            "Unable to render note".into(),
                            "text/plain; charset=utf-8",
                        ),
                    }
                    return;
                }
                if is_base(&path) {
                    let content = render_base(&state.root, &state.index, &path);
                    let title = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let body = page(&title, &content, &state.nav_html, &state.config_json);
                    send_text(request, 200, body, "text/html; charset=utf-8");
                    return;
                }
                if is_canvas(&path) {
                    let content = render_canvas(&state.root, &path);
                    let title = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let body = page(&title, &content, &state.nav_html, &state.config_json);
                    send_text(request, 200, body, "text/html; charset=utf-8");
                    return;
                }
                if is_image(&path) {
                    send_inline_image(request, &path);
                    return;
                }
                match fs::read(&path) {
                    Ok(bytes) => {
                        let mut response =
                            Response::from_data(bytes).with_status_code(StatusCode(200));
                        if let Ok(header) = Header::from_bytes("Content-Type", content_type(&path))
                        {
                            response = response.with_header(header);
                        }
                        if let Ok(header) = Header::from_bytes("X-Content-Type-Options", "nosniff")
                        {
                            response = response.with_header(header);
                        }
                        if let Ok(header) =
                            Header::from_bytes("Cache-Control", "public, max-age=3600")
                        {
                            response = response.with_header(header);
                        }
                        if is_pdf(&path) {
                            if let Ok(header) = Header::from_bytes("Content-Disposition", "inline")
                            {
                                response = response.with_header(header);
                            }
                        }
                        let _ = request.respond(response);
                        return;
                    }
                    Err(_) => {
                        send_text(
                            request,
                            500,
                            "Unable to read file".into(),
                            "text/plain; charset=utf-8",
                        );
                        return;
                    }
                }
            }
        }
        send_text(
            request,
            404,
            "Not found".into(),
            "text/plain; charset=utf-8",
        );
        return;
    }

    if url == "/" {
        if let Some(index_path) = &state.index_page {
            let full_path = state.root.join(index_path);
            match cached_note(state, &full_path) {
                Ok((title, content)) => {
                    let body = page(&title, &content, &state.nav_html, &state.config_json);
                    send_text(request, 200, body, "text/html; charset=utf-8");
                }
                Err(_) => send_text(
                    request,
                    500,
                    "Unable to render index note".into(),
                    "text/plain; charset=utf-8",
                ),
            }
            return;
        }
        let content = "<div class=\"layout single\"><main><h1 class=\"note-title\">Vault</h1><p class=\"empty\">Browse folders and files from the navigator, or search above. Press Ctrl+P for commands or Ctrl+O to search.</p></main></div>";
        let body = page("Vault", content, &state.nav_html, &state.config_json);
        send_text(request, 200, body, "text/html; charset=utf-8");
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/image/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if is_image(&path) {
                let content = render_image(&state.root, &path);
                let title = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let body = page(&title, &content, &state.nav_html, &state.config_json);
                send_text(request, 200, body, "text/html; charset=utf-8");
            } else {
                send_text(
                    request,
                    404,
                    "Not an image".into(),
                    "text/plain; charset=utf-8",
                );
            }
        } else {
            send_text(
                request,
                404,
                "Not found".into(),
                "text/plain; charset=utf-8",
            );
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/base/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if is_base(&path) {
                let content = render_base(&state.root, &state.index, &path);
                let title = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let body = page(&title, &content, &state.nav_html, &state.config_json);
                send_text(request, 200, body, "text/html; charset=utf-8");
            } else {
                send_text(
                    request,
                    404,
                    "Not a Bases file".into(),
                    "text/plain; charset=utf-8",
                );
            }
        } else {
            send_text(
                request,
                404,
                "Not found".into(),
                "text/plain; charset=utf-8",
            );
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/canvas/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if is_canvas(&path) {
                let content = render_canvas(&state.root, &path);
                let title = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let body = page(&title, &content, &state.nav_html, &state.config_json);
                send_text(request, 200, body, "text/html; charset=utf-8");
            } else {
                send_text(
                    request,
                    404,
                    "Not a canvas file".into(),
                    "text/plain; charset=utf-8",
                );
            }
        } else {
            send_text(
                request,
                404,
                "Not found".into(),
                "text/plain; charset=utf-8",
            );
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/note/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file()
                && path
                    .extension()
                    .and_then(|v| v.to_str())
                    .map(|v| v.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            {
                match cached_note(state, &path) {
                    Ok((title, content)) => {
                        let body = page(&title, &content, &state.nav_html, &state.config_json);
                        send_text(request, 200, body, "text/html; charset=utf-8");
                    }
                    Err(_) => send_text(
                        request,
                        500,
                        "Unable to render note".into(),
                        "text/plain; charset=utf-8",
                    ),
                }
            } else {
                send_text(
                    request,
                    404,
                    "Not found".into(),
                    "text/plain; charset=utf-8",
                );
            }
        } else {
            send_text(
                request,
                404,
                "Not found".into(),
                "text/plain; charset=utf-8",
            );
        }
        return;
    }

    send_text(
        request,
        404,
        "Not found".into(),
        "text/plain; charset=utf-8",
    );
}

fn run_cli() {
    let mut arguments = env::args().skip(1);
    let mut vault = PathBuf::from(".");
    let mut index: Option<PathBuf> = None;
    let mut port = 0u16;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--vault" | "-v" => {
                if let Some(value) = arguments.next() {
                    vault = PathBuf::from(value);
                }
            }
            "--index" | "-i" => {
                if let Some(value) = arguments.next() {
                    index = Some(PathBuf::from(value));
                }
            }
            "--port" | "-p" => {
                if let Some(value) = arguments.next() {
                    port = value.parse().unwrap_or(0);
                }
            }
            "--help" | "-h" => {
                println!("vaultlite [--vault PATH] [--index NOTE.md] [--port PORT]");
                return;
            }
            _ => {
                eprintln!("Unknown argument: {argument}");
                process::exit(2);
            }
        }
    }

    let root = match vault.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Cannot open vault: {error}");
            process::exit(1);
        }
    };
    if !root.is_dir() {
        eprintln!("Vault path is not a directory");
        process::exit(1);
    }
    if let Some(index_path) = &index {
        if !root.join(index_path).is_file() {
            eprintln!("Index note not found: {}", index_path.display());
            process::exit(1);
        }
    }

    let vault_index = VaultIndex::build(&root);
    let nav_html = nav(&root, Path::new(""));
    eprintln!("Indexed {} vault files", vault_index.files.len());
    let token = session_token();
    let config_json = UiConfig::load().to_json_with(&vault_name(&root), &token);
    let state = Arc::new(AppState {
        root: root.clone(),
        index: vault_index,
        nav_html,
        index_page: index,
        note_cache: Mutex::new(NoteCache::default()),
        backlinks: RwLock::new(BacklinkIndexState::default()),
        config_json,
        token,
    });

    // Build backlinks off the request path; note rendering stays responsive while
    // this one-time index is prepared in the background.
    let backlink_state = Arc::clone(&state);
    thread::spawn(move || {
        let (by_target, by_source) =
            build_backlink_index(&backlink_state.root, &backlink_state.index);
        if let Ok(mut guard) = backlink_state.backlinks.write() {
            guard.by_target = by_target;
            guard.by_source = by_source;
            guard.ready = true;
        }
        eprintln!("Backlink index ready");
    });

    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|error| {
        eprintln!("Cannot bind localhost: {error}");
        process::exit(1);
    });
    let address = listener.local_addr().unwrap();
    drop(listener);

    let server = Server::http(address).expect("start local HTTP server");
    let url = format!("http://{}/", address);
    println!("VaultLite serving {} at {}", root.display(), url);
    open_default_browser(&url);

    for request in server.incoming_requests() {
        let state = Arc::clone(&state);
        thread::spawn(move || respond(request, &state));
    }
}

struct RunningServer {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl RunningServer {
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn start_server_for_vault(vault: &Path, index_page: Option<&str>) -> Result<RunningServer, String> {
    let root = vault
        .canonicalize()
        .map_err(|e| format!("Cannot open vault: {e}"))?;
    if !root.is_dir() {
        return Err("Selected path is not a folder".into());
    }
    let index = if let Some(value) = index_page.filter(|v| !v.trim().is_empty()) {
        let relative = PathBuf::from(value.trim());
        if relative.is_absolute()
            || relative.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err("Homepage must be a path inside the selected vault".into());
        }
        if !root.join(&relative).is_file() {
            return Err(format!("Homepage note not found: {}", relative.display()));
        }
        Some(relative)
    } else {
        None
    };

    let vault_index = VaultIndex::build(&root);
    let nav_html = nav(&root, Path::new(""));
    let token = session_token();
    let config_json = UiConfig::load().to_json_with(&vault_name(&root), &token);
    let state = Arc::new(AppState {
        root: root.clone(),
        index: vault_index,
        nav_html,
        index_page: index,
        note_cache: Mutex::new(NoteCache::default()),
        backlinks: RwLock::new(BacklinkIndexState::default()),
        config_json,
        token,
    });
    let backlink_state = Arc::clone(&state);
    thread::spawn(move || {
        let (by_target, by_source) =
            build_backlink_index(&backlink_state.root, &backlink_state.index);
        if let Ok(mut guard) = backlink_state.backlinks.write() {
            guard.by_target = by_target;
            guard.by_source = by_source;
            guard.ready = true;
        }
    });

    // Bind port zero to let the OS choose a free localhost port.
    let probe =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("Cannot bind localhost: {e}"))?;
    let address = probe.local_addr().map_err(|e| e.to_string())?;
    drop(probe);
    let server = Server::http(address).map_err(|e| format!("Cannot start local server: {e}"))?;
    let url = format!("http://{}/", address);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_worker = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        while !stop_worker.load(Ordering::Acquire) {
            match server.recv_timeout(Duration::from_millis(150)) {
                Ok(Some(request)) => {
                    let request_state = Arc::clone(&state);
                    thread::spawn(move || respond(request, &request_state));
                }
                Ok(None) => {}
                Err(_) if stop_worker.load(Ordering::Acquire) => break,
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        }
    });
    Ok(RunningServer {
        url,
        stop,
        worker: Some(worker),
    })
}

fn preferences_path() -> PathBuf {
    config::config_dir().join("launcher.conf")
}

fn is_valid_vault(path: &Path) -> bool {
    path.is_dir() && path.join(".obsidian").is_dir()
}

#[derive(Default)]
struct LauncherPreferences {
    recent_vaults: Vec<PathBuf>,
    recent_homepages: Vec<String>,
    last_vault: Option<PathBuf>,
    last_homepage: String,
}
impl LauncherPreferences {
    fn load() -> Self {
        let mut p = Self::default();
        let Ok(text) = fs::read_to_string(preferences_path()) else {
            return p;
        };
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("vault=") {
                p.last_vault = Some(PathBuf::from(v));
            } else if let Some(v) = line.strip_prefix("homepage=") {
                p.last_homepage = v.to_string();
            } else if let Some(v) = line.strip_prefix("recent_vault=") {
                let path = PathBuf::from(v);
                if is_valid_vault(&path) && !p.recent_vaults.contains(&path) {
                    p.recent_vaults.push(path);
                }
            } else if let Some(v) = line.strip_prefix("recent_homepage=") {
                if !v.is_empty() && !p.recent_homepages.iter().any(|x| x == v) {
                    p.recent_homepages.push(v.to_string());
                }
            }
        }
        if let Some(v) = p.last_vault.clone() {
            if is_valid_vault(&v) {
                p.recent_vaults.retain(|x| x != &v);
                p.recent_vaults.insert(0, v);
            }
        }
        if !p.last_homepage.is_empty() {
            let v = p.last_homepage.clone();
            p.recent_homepages.retain(|x| x != &v);
            p.recent_homepages.insert(0, v);
        }
        p.recent_vaults.truncate(8);
        p.recent_homepages.truncate(8);
        p
    }
    fn save(&self) {
        let path = preferences_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut out = String::new();
        if let Some(v) = &self.last_vault {
            out.push_str(&format!("vault={}\n", v.display()));
        }
        out.push_str(&format!("homepage={}\n", self.last_homepage));
        for v in &self.recent_vaults {
            out.push_str(&format!("recent_vault={}\n", v.display()));
        }
        for v in &self.recent_homepages {
            out.push_str(&format!("recent_homepage={}\n", v));
        }
        let _ = fs::write(path, out);
    }
    fn remember_vault(&mut self, path: PathBuf) {
        self.recent_vaults.retain(|v| v != &path);
        self.recent_vaults.insert(0, path.clone());
        self.recent_vaults.truncate(8);
        self.last_vault = Some(path);
        self.save();
    }
    fn remember_homepage(&mut self, value: String) {
        self.recent_homepages.retain(|v| v != &value);
        if !value.is_empty() {
            self.recent_homepages.insert(0, value.clone());
            self.recent_homepages.truncate(8);
        }
        self.last_homepage = value;
        self.save();
    }
}

struct VaultLiteDesktop {
    vault: Option<PathBuf>,
    homepage: String,
    server: Option<RunningServer>,
    status: String,
    preferences: LauncherPreferences,
}
impl Default for VaultLiteDesktop {
    fn default() -> Self {
        let preferences = LauncherPreferences::load();
        let vault = preferences.last_vault.clone().filter(|p| is_valid_vault(p));
        let homepage = preferences.last_homepage.clone();
        Self {
            vault,
            homepage,
            server: None,
            status: "Choose an Obsidian vault or select a recent one.".into(),
            preferences,
        }
    }
}
impl VaultLiteDesktop {
    fn choose_vault(&mut self) {
        let start = self
            .vault
            .as_ref()
            .or(self.preferences.last_vault.as_ref())
            .and_then(|p| p.to_str())
            .unwrap_or("");
        if let Some(selected) =
            tinyfiledialogs::select_folder_dialog("Select Obsidian vault folder", start)
        {
            let path = PathBuf::from(selected);
            if is_valid_vault(&path) {
                self.vault = Some(path.clone());
                self.preferences.remember_vault(path);
                self.status = "Vault selected. Start the server when ready.".into();
            } else {
                self.status = "This folder is not an Obsidian vault: it does not contain a .obsidian folder. It was not added to Recent vaults.".into();
            }
        }
    }
    fn choose_homepage(&mut self) {
        let start = self.vault.as_ref().and_then(|p| p.to_str()).unwrap_or("");
        if let Some(selected) = tinyfiledialogs::open_file_dialog(
            "Select homepage Markdown file",
            start,
            Some((&["*.md", "*.markdown"], "Markdown files")),
        ) {
            let path = PathBuf::from(selected);
            let value = self
                .vault
                .as_ref()
                .and_then(|v| path.strip_prefix(v).ok())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            self.homepage = value.clone();
            self.preferences.remember_homepage(value);
            self.status = "Homepage selected.".into();
        }
    }
}
impl eframe::App for VaultLiteDesktop {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        use eframe::egui;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("VaultLite");
                ui.label("A minimal, read-only browser for your Obsidian vault");
            });
            ui.add_space(14.0);
            let running = self.server.is_some();
            ui.label("Vault folder");
            ui.horizontal(|ui| {
                let display = self.vault.as_ref().map(|p| p.display().to_string())
                    .unwrap_or_else(|| "No vault selected".into());
                ui.add_sized([ui.available_width() - 112.0, 30.0], egui::Label::new(display).truncate());
                if ui.add_enabled(!running, egui::Button::new("Browse…"))
                    .on_hover_text("Choose a vault folder; recent selections are remembered").clicked() {
                    self.choose_vault();
                }
            });
            if !self.preferences.recent_vaults.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Recent vaults:").small().weak());
                    for path in self.preferences.recent_vaults.clone() {
                        let label = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        if ui.add_enabled(!running, egui::Button::new(label))
                            .on_hover_text(path.display().to_string()).clicked() {
                            self.vault = Some(path.clone()); self.preferences.remember_vault(path);
                            self.status = "Recent vault selected.".into();
                        }
                    }
                });
            }
            ui.add_space(10.0);
            ui.label("Homepage note (optional)");
            ui.horizontal(|ui| {
                ui.add_enabled(!running, egui::TextEdit::singleline(&mut self.homepage)
                    .desired_width(ui.available_width() - 105.0).hint_text("Home.md"));
                if ui.add_enabled(!running && self.vault.is_some(), egui::Button::new("Browse…"))
                    .on_hover_text("Choose a Markdown homepage from the selected vault").clicked() {
                    self.choose_homepage();
                }
            });
            if !self.preferences.recent_homepages.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Recent pages:").small().weak());
                    for entry in self.preferences.recent_homepages.clone() {
                        if ui.add_enabled(!running, egui::Button::new(entry.clone()))
                            .on_hover_text("Reuse a previously selected homepage").clicked() {
                            self.homepage = entry.clone(); self.preferences.remember_homepage(entry);
                        }
                    }
                });
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!running && self.vault.is_some(), egui::Button::new("▶  Start server")).clicked() {
                    if let Some(vault) = self.vault.as_ref() {
                        self.preferences.remember_vault(vault.clone());
                        self.preferences.remember_homepage(self.homepage.clone());
                        match start_server_for_vault(vault, Some(&self.homepage)) {
                            Ok(server) => {
                                open_default_browser(&server.url);
                                self.status = format!("Running at {}", server.url);
                                self.server = Some(server);
                            }
                            Err(error) => self.status = error,
                        }
                    }
                }
                if ui.add_enabled(running, egui::Button::new("■  Stop server")).clicked() {
                    self.server.take(); self.status = "Server stopped.".into();
                }
                if let Some(server) = self.server.as_ref() {
                    if ui.button("Open browser").clicked() { open_default_browser(&server.url); }
                }
            });
            ui.add_space(12.0); ui.separator(); ui.label(&self.status); ui.add_space(6.0);
            ui.label(egui::RichText::new("The server is available only on this computer (127.0.0.1). Your vault is not uploaded anywhere.").small().weak());
        });
    }
}

fn run_gui() {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([560.0, 390.0])
            .with_min_inner_size([480.0, 330.0]),
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "VaultLite",
        options,
        Box::new(|_creation_context| Ok(Box::new(VaultLiteDesktop::default()))),
    ) {
        eprintln!("Unable to start VaultLite window: {error}");
    }
}

#[cfg(target_os = "windows")]
fn attach_cli_console() {
    use windows_sys::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        // A GUI-subsystem executable has no console by default. Reattach to the
        // invoking terminal for CLI use, or allocate one when launched directly.
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            let _ = AllocConsole();
        }
    }
}

fn main() {
    // No arguments opens the desktop launcher. Passing CLI flags keeps the
    // original terminal workflow available for scripts and power users.
    if env::args_os().len() > 1 {
        #[cfg(target_os = "windows")]
        attach_cli_console();
        run_cli();
    } else {
        run_gui();
    }
}

#[cfg(test)]
mod page_tests {
    use super::*;

    #[test]
    fn page_injects_assets_navigation_content_and_bindings() {
        let config_json = UiConfig::default().to_json();
        let html = page("Note", "<p>body</p>", "<div>nav</div>", &config_json);
        assert!(html.contains("<style>"));
        assert!(html.contains("<svg class=\"icon-sprite\""));
        assert!(html.contains("\"toggle-right-sidebar\":\"Alt+X\""));
        assert!(html.contains("<div>nav</div>"));
        assert!(html.contains("<p>body</p>"));
        for placeholder in [
            "__TITLE__",
            "__NAV__",
            "__CONTENT__",
            "__CONFIG__",
            "__CSS__",
            "__JS__",
        ] {
            assert!(!html.contains(placeholder), "unreplaced {placeholder}");
        }
    }

    #[test]
    fn canvas_files_have_a_dedicated_icon() {
        assert!(is_canvas(Path::new("Board.canvas")));
        assert!(is_canvas(Path::new("FOLDER/BOARD.CANVAS")));
        assert_eq!(file_icon(Path::new("Board.canvas")), "i-canvas");
        assert!(!is_canvas(Path::new("Board.md")));
    }

    #[test]
    fn toc_entries_carry_collapse_metadata_and_width_buttons_are_ordered() {
        let dir = std::env::temp_dir().join(format!("vaultlite-toc-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Toc.md"), "# A\n\n## B\n\n### C\n\n## D\n").unwrap();

        let root = dir.canonicalize().unwrap();
        let index = VaultIndex::build(&root);
        let source = fs::read_to_string(root.join("Toc.md")).unwrap();
        let html = render_note(&root, &index, Path::new("Toc.md"), "Toc", &source);

        assert!(html.contains("class=\"toc-entry toc-level-1\" data-level=\"1\" data-id=\"a\""));
        assert!(html.contains("class=\"toc-entry toc-level-2\" data-level=\"2\" data-id=\"b\""));
        assert!(html.contains("class=\"toc-entry toc-level-3\" data-level=\"3\" data-id=\"c\""));
        assert!(html.contains("data-toc-toggle"));
        // The wider button comes first now, followed by the narrower button.
        // A right-docked panel grows leftward, so the widen button points left.
        let larger = html.find("data-toc-resize=\"larger\"").unwrap();
        let smaller = html.find("data-toc-resize=\"smaller\"").unwrap();
        assert!(larger < smaller);
        assert!(html.contains(
            "data-toc-resize=\"larger\" type=\"button\" title=\"Increase right sidebar width\" aria-label=\"Increase right sidebar width\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-chevron-left\"/>"
        ));
        assert!(html.contains(
            "data-toc-resize=\"smaller\" type=\"button\" title=\"Decrease right sidebar width\" aria-label=\"Decrease right sidebar width\"><svg class=\"icon\" aria-hidden=\"true\"><use href=\"#i-chevron-right\"/>"
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_defaults_to_dark_theme() {
        let config_json = UiConfig::default().to_json();
        let html = page("Note", "", "", &config_json);
        assert!(html.contains("<html lang=\"en\" data-theme=\"dark\">"));
    }

    #[test]
    fn embedded_notes_render_and_stop_recursion() {
        let dir = std::env::temp_dir().join(format!("vaultlite-embed-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("A.md"), "# A\n\n![[B.md]]\n\n![[A.md]]\n").unwrap();
        fs::write(
            dir.join("B.md"),
            "# B\n\nSome **bold** text.\n\n![[A.md]]\n",
        )
        .unwrap();

        let root = dir.canonicalize().unwrap();
        let index = VaultIndex::build(&root);
        let source = fs::read_to_string(root.join("A.md")).unwrap();
        let html = render_note(&root, &index, Path::new("A.md"), "A", &source);

        assert!(
            html.contains("class=\"note-embed\""),
            "embed was not rendered"
        );
        assert!(
            html.contains("href=\"/note/B%2Emd\""),
            "embed title is missing"
        );
        assert!(
            html.contains("<strong>bold</strong>"),
            "embed Markdown was not rendered"
        );
        // One notice for A embedding itself, one for B embedding A.
        assert_eq!(html.matches("Recursive embed stopped").count(), 2);
        // Embedded headings must not leak into the top-level table of contents.
        assert!(html.contains("href=\"#a\""));
        assert!(!html.contains("href=\"#b\""));

        let _ = fs::remove_dir_all(&dir);
    }
}
