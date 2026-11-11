#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use pulldown_cmark::{html, Options, Parser};
use regex::Regex;
use std::{
    env, fs,
    net::TcpListener,
    path::{Component, Path, PathBuf},
    process,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex, OnceLock, RwLock},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};
use std::{borrow::Cow, collections::{HashMap, HashSet, VecDeque}, io::Read};
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
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() { continue; }
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
            let Ok(canonical) = candidate.canonicalize() else { continue };
            if !canonical.starts_with(root) || !canonical.is_file() { continue; }
            let Some(relative) = canonical.strip_prefix(root).ok() else { continue };
            let relative_key = normalized_rel(relative).to_lowercase();
            let name_key = canonical.file_name().map(|v| v.to_string_lossy().to_lowercase()).unwrap_or_default();
            let stem_key = canonical.file_stem().map(|v| v.to_string_lossy().to_lowercase()).unwrap_or_default();
            index.by_relative_path.insert(relative_key, canonical.clone());
            index.by_name.entry(name_key).or_default().push(canonical.clone());
            index.by_stem.entry(stem_key).or_default().push(canonical.clone());
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
}

fn cached_note(state: &AppState, path: &Path) -> std::io::Result<(String, String)> {
    const CACHE_LIMIT: usize = 32 * 1024 * 1024;
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified().ok();
    let len = metadata.len();
    let title = path.file_stem().unwrap_or_default().to_string_lossy().to_string();

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

    // Keep the most recently inserted rendered notes up to a fixed memory cap.
    // Oversized notes still render normally but do not displace useful cache entries.
    if html.len() <= CACHE_LIMIT {
        if let Ok(mut cache) = state.note_cache.lock() {
            if let Some(previous) = cache.entries.remove(path) {
                cache.bytes = cache.bytes.saturating_sub(previous.html.len());
                cache.oldest.retain(|candidate| candidate != path);
            }
            while cache.bytes.saturating_add(html.len()) > CACHE_LIMIT {
                let Some(oldest_path) = cache.oldest.pop_front() else { break; };
                if let Some(evicted) = cache.entries.remove(&oldest_path) {
                    cache.bytes = cache.bytes.saturating_sub(evicted.html.len());
                }
            }
            cache.bytes = cache.bytes.saturating_add(html.len());
            cache.oldest.push_back(path.to_path_buf());
            cache.entries.insert(path.to_path_buf(), CachedNote { modified, len, html: html.clone() });
        }
    }
    Ok((title, html))
}

fn clean_relative(base: &Path, target: &Path) -> Option<String> {
    let joined = if target.is_absolute() { return None; } else { base.join(target) };
    let mut parts: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {},
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            Component::ParentDir => { parts.pop()?; },
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

fn find_case_insensitive(index: &VaultIndex, target: &str) -> Option<PathBuf> {
    let normalized_target = target.replace('\\', "/");
    if let Some(found) = index.by_relative_path.get(&normalized_target.to_lowercase()) {
        return Some(found.clone());
    }
    let target_name = Path::new(&normalized_target).file_name()?.to_string_lossy().to_lowercase();
    index.by_name.get(&target_name).and_then(|matches| matches.first()).cloned()
}


fn resolve_file(root: &Path, from: &Path, target: &str, index: &VaultIndex) -> Option<PathBuf> {
    let target = target.trim();
    let path_part = target.split('#').next().unwrap_or(target).split('?').next().unwrap_or("").trim();
    if path_part.is_empty() { return None; }
    let decoded = percent_decode_str(path_part).decode_utf8().ok()?;
    let normalized = decoded.replace('\\', "/");
    let target_path = Path::new(&normalized);

    // First resolve relative to the current note, then from the vault root.
    let from_parent = from.parent().unwrap_or(Path::new(""));
    for base in [from_parent, Path::new("")] {
        if let Some(key) = clean_relative(base, target_path) {
            if let Some(found) = index.by_relative_path.get(&key.to_lowercase()) {
                if found.starts_with(root) { return Some(found.clone()); }
            }
        }
    }
    find_case_insensitive(index, &normalized).filter(|found| found.starts_with(root))
}

fn resolve_markdown_target(root: &Path, from: &Path, target: &str, index: &VaultIndex) -> Option<PathBuf> {
    if let Some(found) = resolve_file(root, from, target, index) {
        return Some(found);
    }

    // In Obsidian, periods are allowed in note names. A target such as
    // "Project.v2" may therefore refer to "Project.v2.md", not an extension.
    let path_part = target.split('#').next().unwrap_or(target).split('?').next().unwrap_or(target).trim();
    if path_part.is_empty() || path_part.to_ascii_lowercase().ends_with(".md") {
        return None;
    }
    let with_extension = format!("{}.md", path_part);
    resolve_file(root, from, &with_extension, index)
}

fn resolve_embed_target(root: &Path, from: &Path, target: &str, index: &VaultIndex) -> Option<PathBuf> {
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
    let known_extension = requested.extension().and_then(|value| value.to_str()).map(|extension| {
        matches!(extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "avif" | "bmp" | "ico" | "tif" | "tiff" |
            "pdf" | "md" | "txt" | "html" | "htm" | "css" | "js" | "mp3" | "wav" | "ogg" | "mp4" | "webm" | "mov" | "json" | "xml")
    }).unwrap_or(false);
    if known_extension { return None; }

    let requested_name = requested.file_name()?.to_string_lossy().to_lowercase();
    let requested_parent = requested.parent().unwrap_or(Path::new(""));
    let note_parent = from.parent().unwrap_or(Path::new(""));
    let preferred_parent = if requested_parent.as_os_str().is_empty() { note_parent } else { requested_parent };
    let matches = index.by_stem.get(&requested_name)?;
    let same_folder = matches.iter().find(|candidate| {
        let relative = candidate.strip_prefix(root).unwrap_or(candidate);
        relative.parent().map(|parent| normalized_rel(parent).eq_ignore_ascii_case(&normalized_rel(preferred_parent))).unwrap_or(false)
    });
    same_folder.and_then(|candidate| {
        matches.iter().find(|path| is_image(path) && path.parent() == candidate.parent()).cloned()
    }).or_else(|| same_folder.cloned()).or_else(|| matches.iter().find(|path| is_image(path)).cloned()).or_else(|| matches.first().cloned())
}

fn resolve_wiki_target(root: &Path, from: &Path, target: &str, index: &VaultIndex) -> Option<PathBuf> {
    resolve_markdown_target(root, from, target, index)
}

fn build_backlink_index(root: &Path, index: &VaultIndex) -> (HashMap<PathBuf, Vec<PathBuf>>, HashMap<PathBuf, Vec<PathBuf>>) {
    let mut by_target: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut by_source: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for source_path in index.files.iter().filter(|path| is_markdown(path)) {
        let Ok(source) = fs::read_to_string(source_path) else { continue; };
        let relative_source = source_path.strip_prefix(root).unwrap_or(source_path);
        let mut targets_from_note = HashSet::new();

        for captures in wikilink_regex().captures_iter(&source) {
            let raw = captures.get(1).or_else(|| captures.get(2)).map(|m| m.as_str()).unwrap_or("");
            let target = raw.split('|').next().unwrap_or("").trim();
            if target.is_empty() { continue; }
            if let Some(target_path) = resolve_wiki_target(root, relative_source, target, index) {
                if is_markdown(&target_path) && target_path.as_path() != source_path.as_path() {
                    targets_from_note.insert(target_path);
                }
            }
        }

        for captures in markdown_link_regex().captures_iter(&source) {
            let raw = captures.get(2).map(|m| m.as_str()).unwrap_or("");
            let target = markdown_destination(raw);
            if target.is_empty() || target.starts_with("http://") || target.starts_with("https://") ||
                target.starts_with("mailto:") || target.starts_with("data:") || target.starts_with('/') || target.starts_with('#') {
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
            by_target.entry(target_path.clone()).or_default().push(source_path.clone());
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
    if links.is_empty() { return format!("<span class=\"muted\">{}</span>", empty); }
    let mut output = String::from("<div class=\"backlink-list\">");
    for path in links {
        let relative = path.strip_prefix(root).map(normalized_rel).unwrap_or_default();
        let encoded = url_path(&relative);
        let label = path.file_stem().unwrap_or_default().to_string_lossy();
        output.push_str(&format!("<a class=\"backlink-item\" href=\"/note/{}\" title=\"Open {}\">{}</a>", encoded, esc(&label), esc(&label)));
    }
    output.push_str("</div>");
    output
}

fn render_backlinks(root: &Path, backlinks: &[PathBuf], outgoing: &[PathBuf]) -> String {
    format!(
        "<section id=\"backlinks\" class=\"backlinks main-backlinks\"><table class=\"link-relations\"><thead><tr><th>Backlinks</th><th>Outgoing links</th></tr></thead><tbody><tr><td>{}</td><td>{}</td></tr></tbody></table></section>",
        render_link_list(root, backlinks, "No backlinks"),
        render_link_list(root, outgoing, "No outgoing links")
    )
}

fn is_video_target(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("https://www.youtube.com/") || lower.starts_with("https://youtube.com/") ||
    lower.starts_with("https://youtu.be/") || lower.starts_with("https://vimeo.com/") ||
    [".mp4", ".webm", ".mov", ".m4v", ".ogv"].iter().any(|ext| lower.split(|ch| ch == '?' || ch == '#').next().unwrap_or(&lower).ends_with(ext))
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
    path.extension().and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("md")).unwrap_or(false)
}

fn is_base(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("base")).unwrap_or(false)
}

fn is_pdf(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("pdf")).unwrap_or(false)
}

fn rewrite_wikilinks<'a>(source: &'a str, root: &Path, index: &VaultIndex, relative_note: &Path) -> Cow<'a, str> {
    if !source.contains("[[") { return Cow::Borrowed(source); }
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
    if !(host.starts_with("127.0.0.1:") || host.starts_with("localhost:") || host == "127.0.0.1" || host == "localhost") {
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

fn rewrite_markdown_paths<'a>(source: &'a str, root: &Path, index: &VaultIndex, relative_note: &Path) -> Cow<'a, str> {
    if !source.contains("![") && !source.contains("](") { return Cow::Borrowed(source); }
    let with_images = if source.contains("![") { markdown_image_regex().replace_all(source, |captures: &regex::Captures| {
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

    if !with_images.contains("](") { return with_images; }
    let linked = markdown_link_regex()
        .replace_all(&with_images, |captures: &regex::Captures| {
            let label = captures.get(1).map(|value| value.as_str()).unwrap_or("");
            let raw_target = captures.get(2).map(|value| value.as_str()).unwrap_or("");
            let target = markdown_destination(raw_target);
            if let Some((_kind, local_path)) = local_vault_route(&target) {
                if let Some(path) = resolve_file(root, relative_note, &local_path, index) {
                    if let Some(encoded) = relative_url(root, &path) {
                        let fragment = local_path.find('#').map(|i| &local_path[i..]).unwrap_or("");
                        let is_md = path.extension().and_then(|v| v.to_str()).map(|v| v.eq_ignore_ascii_case("md")).unwrap_or(false);
                        let route = if is_md { "note/" } else if is_base(&path) { "base/" } else if is_image(&path) { "image/" } else { "file/" };
                        let display = if label == target {
                            path.file_stem().or_else(|| path.file_name()).map(|value| value.to_string_lossy().to_string()).unwrap_or_else(|| label.to_string())
                        } else {
                            label.to_string()
                        };
                        if is_pdf(&path) {
                            return format!("<a href=\"/{}{}{}\" target=\"_blank\" rel=\"noopener\">{}</a>", route, encoded, fragment, esc(&display));
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
                    let fragment = target
                        .find('#')
                        .map(|index| &target[index..])
                        .unwrap_or("");
                    if is_markdown(&path) {
                        return format!("[{}](/note/{}{})", label, encoded, fragment);
                    }
                    if is_base(&path) {
                        return format!("[{}](/base/{})", label, encoded);
                    }
                    if is_image(&path) {
                        return format!("[{}](/image/{})", label, encoded);
                    }
                    if is_pdf(&path) {
                        return format!("<a href=\"/file/{}{}\" target=\"_blank\" rel=\"noopener\">{}</a>", encoded, fragment, label);
                    }
                    return format!("[{}](/file/{}{})", label, encoded, fragment);
                }
            }
            captures.get(0).unwrap().as_str().to_string()
        });
    let unchanged = matches!(&with_images, Cow::Borrowed(_)) && matches!(&linked, Cow::Borrowed(_));
    let linked = linked.into_owned();
    if unchanged { Cow::Borrowed(source) } else { Cow::Owned(linked) }
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
            || line.chars().next().map(char::is_whitespace).unwrap_or(false)
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
            let content = current.strip_prefix('>').unwrap_or("").strip_prefix(' ').unwrap_or_else(|| current.strip_prefix('>').unwrap_or(""));
            quoted.push(content.trim_end_matches(&['\r', '\n'][..]));
            i += 1;
        }

        let first = quoted.iter().position(|line| !line.trim().is_empty());
        let callout_info = first.and_then(|idx| {
            let line = quoted[idx].trim();
            let rest = line.strip_prefix("[!")?;
            let close = rest.find(']')?;
            let kind = rest[..close].trim().to_ascii_lowercase();
            if kind.is_empty() { return None; }
            let mut tail = rest[close + 1..].trim().to_string();
            let fold = if let Some(v) = tail.strip_prefix('-') { tail = v.trim().to_string(); Some(false) }
                else if let Some(v) = tail.strip_prefix('+') { tail = v.trim().to_string(); Some(true) }
                else { None };
            let title = if tail.is_empty() { kind.split('-').map(|part| {
                let mut chars = part.chars(); chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
            }).collect::<Vec<_>>().join(" ") } else { tail };
            Some((idx, kind, title, fold))
        });

        if let Some((header_idx, kind, title, fold)) = callout_info {
            let body_lines = quoted.iter().enumerate().filter(|(idx, _)| *idx != header_idx).map(|(_, line)| *line).collect::<Vec<_>>();
            let body_source = body_lines.join("\n");
            let body_html = render_markdown_fragment(&body_source);
            let mut title_rendered = String::new();
            html::push_html(&mut title_rendered, Parser::new_ext(&title, markdown_options()));
            let title_rendered = title_rendered.trim().strip_prefix("<p>").unwrap_or(title_rendered.trim())
                .strip_suffix("</p>\n").or_else(|| title_rendered.trim().strip_prefix("<p>").unwrap_or(title_rendered.trim()).strip_suffix("</p>"))
                .unwrap_or(title_rendered.trim()).to_string();
            let title_html = format!("<div class=\"callout-title\"><span class=\"callout-icon\">{}</span><span class=\"callout-label\">{}</span></div>", callout_icon(&kind), title_rendered);
            if let Some(open) = fold {
                let open_attr = if open { " open" } else { "" };
                transformed.push_str(&format!("<details class=\"callout callout-{}\"{}>\n<summary>{}</summary>\n<div class=\"callout-content\">\n{}\n</div>\n</details>\n\n", esc(&kind), open_attr, title_html, body_html));
            } else {
                transformed.push_str(&format!("<div class=\"callout callout-{}\">\n{}\n<div class=\"callout-content\">\n{}\n</div>\n</div>\n\n", esc(&kind), title_html, body_html));
            }
        } else {
            for line in &lines[start..i] { transformed.push_str(line); }
        }
    }

    let parser = Parser::new_ext(&transformed, markdown_options());
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    rendered
}

fn callout_icon(kind: &str) -> &'static str {
    match kind.split('-').next().unwrap_or(kind) {
        "note" => "✎", "abstract" | "summary" | "tldr" => "▤", "info" => "ℹ",
        "todo" => "☑", "tip" | "hint" | "important" => "✦", "success" | "check" | "done" => "✓",
        "question" | "help" | "faq" => "?", "warning" | "caution" | "attention" => "⚠",
        "failure" | "fail" | "missing" | "danger" | "error" => "✕", "bug" => "⌕",
        "example" => "❖", "quote" | "cite" => "❞", _ => "◆",
    }
}

fn render_note(root: &Path, index: &VaultIndex, relative_note: &Path, title: &str, source: &str) -> String {
    let (properties, body) = split_frontmatter(source);
    let rewritten_wiki = rewrite_wikilinks(body, root, index, relative_note);
    let rewritten = rewrite_markdown_paths(&rewritten_wiki, root, index, relative_note);
    let rendered = render_markdown_fragment(&rewritten);

    let heading_pattern = heading_regex();
    let tag_pattern = html_tag_regex();
    let mut toc = String::new();
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
            let guides = (1..level).map(|_| "<i></i>").collect::<String>();
            toc.push_str(&format!(
                "<a class=\"toc-level-{}\" href=\"#{}\"><span class=\"toc-guides\" aria-hidden=\"true\">{}</span><span class=\"toc-label\">{}</span></a>",
                level,
                id,
                guides,
                esc(&heading_text)
            ));
            format!("<h{} id=\"{}\">{}</h{}>", &captures[1], id, &captures[2], &captures[1])
        })
        .to_string();

    let backlink_route = url_path(&normalized_rel(relative_note));
    format!(
        "<div class=\"layout\"><main><h1 class=\"note-title\">{}</h1>{}<article>{}</article></main><aside><div class=\"toc-heading\"><b>On this page</b><div class=\"toc-actions\"><button class=\"icon-button right-wrap-toggle\" data-wrap-control=\"right\" type=\"button\" title=\"Toggle wrapping in table of contents\" aria-label=\"Toggle wrapping in table of contents\">↵</button><button class=\"icon-button\" id=\"hide-right-sidebar\" type=\"button\" title=\"Hide right sidebar\" aria-label=\"Hide right sidebar\">◨</button></div></div><input id=\"heading-search\" type=\"search\" placeholder=\"Find headings…\" autocomplete=\"off\" aria-label=\"Search headings in this note\"><div class=\"toc\">{}</div></aside></div><section id=\"backlinks\" class=\"backlinks main-backlinks\" data-note-path=\"{}\"><table class=\"link-relations\"><thead><tr><th>Backlinks</th><th>Outgoing links</th></tr></thead><tbody><tr><td class=\"muted\">Loading…</td><td class=\"muted\">Loading…</td></tr></tbody></table></section>",
        esc(title),
        properties,
        with_ids,
        if toc.is_empty() { "<span class=\"muted\">No headings</span>" } else { &toc },
        backlink_route
    )
}

fn frontmatter_property(path: &Path, wanted: &str) -> String {
    if !is_markdown(path) { return String::new(); }
    let Ok(file) = fs::File::open(path) else { return String::new(); };
    let mut source = String::new();
    if file.take(256 * 1024).read_to_string(&mut source).is_err() { return String::new(); }
    let source = source.trim_start_matches('\u{feff}');
    let mut lines = source.lines();
    if lines.next().map(str::trim) != Some("---") { return String::new(); }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "..." { break; }
        if let Some((key, value)) = trimmed.split_once(':') {
            if key.trim().eq_ignore_ascii_case(wanted) {
                return value.trim().trim_matches(|ch| ch == '"' || ch == '\'').to_string();
            }
        }
    }
    String::new()
}

fn base_field(path: &Path, root: &Path, field: &str) -> String {
    let relative = path.strip_prefix(root).map(normalized_rel).unwrap_or_default();
    let name = path.file_stem().or_else(|| path.file_name()).map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
    match field.trim() {
        "file.name" => name,
        "file.path" => relative,
        "file.folder" => path.parent().and_then(|p| p.strip_prefix(root).ok()).map(normalized_rel).unwrap_or_default(),
        "file.ext" => ext,
        "file.size" => fs::metadata(path).map(|m| m.len().to_string()).unwrap_or_default(),
        "file.mtime" => fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs().to_string()).unwrap_or_default(),
        value if value.starts_with("note.") => frontmatter_property(path, &value[5..]),
        value if !value.contains('.') => frontmatter_property(path, value),
        _ => String::new(),
    }
}

fn render_base(root: &Path, index: &VaultIndex, path: &Path) -> String {
    let title = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let source = fs::read_to_string(path).unwrap_or_default();
    let base: serde_yaml::Value = serde_yaml::from_str(&source).unwrap_or(serde_yaml::Value::Null);
    let first_view = base.get("views").and_then(serde_yaml::Value::as_sequence).and_then(|views| views.first());
    let view_name = first_view.and_then(|view| view.get("name")).and_then(serde_yaml::Value::as_str).unwrap_or("All files");
    let view_type = first_view.and_then(|view| view.get("type")).and_then(serde_yaml::Value::as_str).unwrap_or("table");
    let fields: Vec<String> = first_view.and_then(|view| view.get("order")).and_then(serde_yaml::Value::as_sequence)
        .map(|values| values.iter().filter_map(serde_yaml::Value::as_str).map(str::to_string).collect())
        .filter(|values: &Vec<String>| !values.is_empty())
        .unwrap_or_else(|| vec!["file.name".into(), "file.path".into(), "file.ext".into(), "file.size".into()]);
    let limit = first_view.and_then(|view| view.get("limit")).and_then(serde_yaml::Value::as_u64).unwrap_or(200).clamp(1, 500) as usize;
    let has_filters = base.get("filters").is_some() || first_view.and_then(|view| view.get("filters")).is_some();
    let mut html_out = format!("<div class=\"layout\"><main><h1 class=\"note-title\">{}</h1><section class=\"base-view\"><p class=\"base-note\">{} · Showing up to {} files</p>", esc(&title), esc(view_name), limit);
    if has_filters {
        html_out.push_str("<p class=\"base-note\">This lightweight viewer displays the base columns but does not evaluate Bases filter expressions or formulas.</p>");
    }
    let files: Vec<&PathBuf> = index.files.iter().filter(|candidate| candidate.as_path() != path).take(limit).collect();
    if view_type.eq_ignore_ascii_case("list") {
        html_out.push_str("<ul class=\"base-list\">");
        for file in &files {
            let relative = file.strip_prefix(root).map(normalized_rel).unwrap_or_default();
            let encoded = url_path(&relative);
            let href = if is_markdown(file) { format!("/note/{}", encoded) } else if is_image(file) { format!("/image/{}", encoded) } else if is_base(file) { format!("/base/{}", encoded) } else { format!("/file/{}", encoded) };
            let label = file.file_stem().or_else(|| file.file_name()).map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
            let target = if is_pdf(file) { " target=\"_blank\" rel=\"noopener\"" } else { "" };
            html_out.push_str(&format!("<li><a href=\"{}\"{}>{}</a></li>", href, target, esc(&label)));
        }
        html_out.push_str("</ul>");
    } else if view_type.eq_ignore_ascii_case("cards") || view_type.eq_ignore_ascii_case("kanban") {
        html_out.push_str("<div class=\"base-cards\">");
        for file in &files {
            let relative = file.strip_prefix(root).map(normalized_rel).unwrap_or_default();
            let encoded = url_path(&relative);
            let href = if is_markdown(file) { format!("/note/{}", encoded) } else if is_image(file) { format!("/image/{}", encoded) } else if is_base(file) { format!("/base/{}", encoded) } else { format!("/file/{}", encoded) };
            let label = file.file_stem().or_else(|| file.file_name()).map(|v| v.to_string_lossy().to_string()).unwrap_or_default();
            let target = if is_pdf(file) { " target=\"_blank\" rel=\"noopener\"" } else { "" };
            html_out.push_str(&format!("<div class=\"base-card\"><a href=\"{}\"{}>{}</a><div class=\"base-note\">{}</div></div>", href, target, esc(&label), esc(&relative)));
        }
        html_out.push_str("</div>");
    } else {
        html_out.push_str("<div style=\"overflow-x:auto\"><table class=\"base-table\"><thead><tr>");
        for field in &fields { html_out.push_str(&format!("<th>{}</th>", esc(field.strip_prefix("note.").unwrap_or(field)))); }
        html_out.push_str("</tr></thead><tbody>");
        for file in &files {
            let relative = file.strip_prefix(root).map(normalized_rel).unwrap_or_default();
            let encoded = url_path(&relative);
            let href = if is_markdown(file) { format!("/note/{}", encoded) } else if is_image(file) { format!("/image/{}", encoded) } else if is_base(file) { format!("/base/{}", encoded) } else { format!("/file/{}", encoded) };
            html_out.push_str("<tr>");
            for (column_index, field) in fields.iter().enumerate() {
                let value = base_field(file, root, field);
                if field == "file.name" || column_index == 0 {
                    let target = if is_pdf(file) { " target=\"_blank\" rel=\"noopener\"" } else { "" };
                    let label = if value.is_empty() { file.file_name().unwrap_or_default().to_string_lossy().to_string() } else { value };
                    html_out.push_str(&format!("<td><a href=\"{}\"{}>{}</a></td>", href, target, esc(&label)));
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
    let title = path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let relative = path.strip_prefix(root).map(normalized_rel).unwrap_or_default();
    let encoded = url_path(&relative);
    format!("<div class=\"layout\"><main><h1 class=\"note-title\">{}</h1><article class=\"file-preview\"><img src=\"/asset/{}\" alt=\"{}\" decoding=\"async\"></article></main></div>", esc(&title), encoded, esc(&title))
}

fn file_icon(path: &Path) -> &'static str {
    if is_image(path) {
        "🖼️"
    } else if path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
    {
        "📄"
    } else {
        "🔗"
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
                let Ok(file_type) = entry.file_type() else { continue; };
                if file_type.is_symlink() { continue; }
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let relative_text = normalized_rel(relative);
                if path.is_dir() {
                    output.push_str(&format!(
                        "<details class=\"folder\" data-name=\"{}\" data-path=\"{}\"><summary><span class=\"entry-icon\">📁</span>{}</summary><div class=\"folder-children\">",
                        esc(&name.to_lowercase()),
                        esc(&relative_text.to_lowercase()),
                        esc(&name)
                    ));
                    walk(root, &path, current, output);
                    output.push_str("</div></details>");
                } else if path.is_file() {
                    let markdown_file = is_markdown(&path);
                    let is_current = markdown_file && relative == current;
                    let destination = if markdown_file {
                        format!("/note/{}", url_path(&relative_text))
                    } else if is_image(&path) {
                        format!("/image/{}", url_path(&relative_text))
                    } else if is_base(&path) {
                        format!("/base/{}", url_path(&relative_text))
                    } else {
                        format!("/file/{}", url_path(&relative_text))
                    };
                    let target_attr = if is_pdf(&path) { " target=\"_blank\" rel=\"noopener\"" } else { "" };
                    let label = if markdown_file {
                        path.file_stem().unwrap_or_default().to_string_lossy().to_string()
                    } else {
                        name.clone()
                    };
                    output.push_str(&format!(
                        "<a class=\"file{}\" data-name=\"{}\" data-path=\"{}\" href=\"{}\"{}><span class=\"entry-icon\">{}</span>{}</a>",
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

fn page(title: &str, content: &str, nav_html: &str) -> String {
    let template = r##"<!doctype html>
<html lang="en" data-theme="light">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>__TITLE__ · VaultLite</title>
<style>
:root{color-scheme:light;--bg:#fff;--panel:#fafafa;--fg:#222;--muted:#777;--line:#e7e7e7;--accent:#526fe5;--code:#f3f4f6;--hover:#f1f3f8;--bold-color:#334b91;--italic-color:#8a4e91;--strike-color:#888;--link-color:#365fc8;--h1-color:#334b91;--h2-color:#3d5fa5;--h3-color:#496da8;--h4-color:#607da8;--h5-color:#66788d;--h6-color:#77808b}
:root[data-theme="dark"]{color-scheme:dark;--bg:#171819;--panel:#1d1f21;--fg:#e6e6e6;--muted:#a0a0a0;--line:#343638;--accent:#9aafff;--code:#24272a;--hover:#282b2e;--bold-color:#b9c8ff;--italic-color:#e2b8e8;--strike-color:#999;--link-color:#a8bbff;--h1-color:#c2d0ff;--h2-color:#b4c6f5;--h3-color:#a9c0e9;--h4-color:#a6b7d5;--h5-color:#b2bfd1;--h6-color:#c0c7d0}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.65 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
a{color:var(--link-color)}article strong,article b{color:var(--bold-color)}article em,article i{color:var(--italic-color)}article del,article s,article strike{color:var(--strike-color)}article code{color:var(--h3-color)}article mark{background:color-mix(in srgb,var(--accent) 22%,transparent);color:var(--fg)}article blockquote{border-left-color:var(--h4-color)}article hr{border:0;border-top:1px solid var(--line)}article h1{color:var(--h1-color)}article h2{color:var(--h2-color)}article h3{color:var(--h3-color)}article h4{color:var(--h4-color)}article h5{color:var(--h5-color)}article h6{color:var(--h6-color)}.app{display:grid;grid-template-columns:var(--sidebar-width,270px) minmax(0,1fr);min-height:100vh}.app.nav-hidden{grid-template-columns:minmax(0,1fr)}.app.nav-hidden nav{display:none}
nav{border-right:1px solid var(--line);padding:14px 10px;overflow:auto;height:100vh;position:sticky;top:0;background:var(--panel)}
.nav-top{display:flex;align-items:center;justify-content:space-between;margin:0 7px 12px}.brand{font-weight:750;font-size:16px;color:var(--fg);text-decoration:none}
.nav-actions{display:flex;gap:5px;flex-wrap:wrap;justify-content:flex-end}.nav-tools{display:flex;gap:5px;flex-wrap:wrap;margin:0 0 10px}.icon-button{display:inline-grid;place-items:center;min-width:30px;height:30px;border:1px solid var(--line);background:var(--bg);color:var(--fg);border-radius:7px;padding:4px 6px;cursor:pointer;font:inherit;font-size:14px;line-height:1}.icon-button:hover{background:var(--hover);border-color:color-mix(in srgb,var(--accent) 40%,var(--line))}
#vault-search{width:100%;padding:8px 10px;margin:0 0 12px;border:1px solid var(--line);border-radius:7px;background:var(--bg);color:var(--fg);font:inherit;font-size:13px;outline:none}
#vault-search:focus{border-color:var(--accent)}.tree{font-size:13px}.tree details{padding-left:8px}.tree summary{cursor:pointer;list-style:none;padding:4px 7px;border-radius:5px;color:var(--muted);display:flex;gap:7px;align-items:center;overflow-wrap:anywhere}
.tree summary::-webkit-details-marker{display:none}.tree summary:hover,.tree a:hover{background:var(--hover);color:var(--fg)}.folder-children{margin-left:8px;border-left:1px solid var(--line);padding-left:5px}
.tree a.file{display:flex;gap:7px;align-items:flex-start;padding:4px 7px;margin:1px 0;border-radius:5px;text-decoration:none;color:var(--muted);overflow-wrap:anywhere}
.tree a.active{background:color-mix(in srgb,var(--accent) 14%,transparent);color:var(--fg)}.entry-icon{flex:0 0 18px;width:18px;text-align:center;font-size:13px;line-height:1.65}
.layout{display:grid;grid-template-columns:minmax(0,780px) 190px;gap:46px;padding:42px 5vw;justify-content:center;align-items:start}main{min-width:0}.note-title{font-size:2rem;line-height:1.25;font-weight:720;letter-spacing:-.035em;margin:0 0 20px;overflow-wrap:anywhere}
article{min-width:0;overflow-wrap:anywhere;font-size:var(--note-font-size,16px)}body.left-no-wrap .tree summary,body.left-no-wrap .tree a.file{white-space:nowrap;overflow-wrap:normal}article img,.vault-image{max-width:100%;height:auto;border-radius:5px}article a{overflow-wrap:anywhere}article h1,article h2,article h3{line-height:1.3;margin-top:1.7em}article h1:first-child,article h2:first-child{margin-top:0}
pre{overflow:auto;padding:14px;background:var(--code);border-radius:7px}code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace}blockquote{border-left:3px solid var(--line);margin-left:0;padding-left:16px;color:var(--muted)}
.callout{--callout-color:var(--accent);border:1px solid color-mix(in srgb,var(--callout-color) 35%,var(--line));border-left:2px solid var(--callout-color);border-radius:8px;margin:1.1em 0;padding:12px 16px;background:color-mix(in srgb,var(--callout-color) 7%,var(--bg));box-sizing:border-box}.callout-title{display:flex;align-items:center;gap:9px;min-height:24px;font-weight:700;color:var(--callout-color);margin:0 0 7px;line-height:24px}.callout-icon{display:flex;flex:0 0 22px;align-items:center;justify-content:center;box-sizing:border-box;width:22px;height:22px;margin:0;border-radius:50%;background:color-mix(in srgb,var(--callout-color) 16%,transparent);font-size:14px;line-height:1;text-align:center;vertical-align:middle}.callout-content>:first-child{margin-top:0}.callout-content>:last-child{margin-bottom:0}.callout>summary{cursor:pointer;list-style:none}.callout>summary::-webkit-details-marker{display:none}.callout>summary .callout-title{margin:0}.callout-note{--callout-color:#448aff}.callout-abstract,.callout-summary,.callout-tldr{--callout-color:#00a6a6}.callout-info,.callout-todo,.callout-question,.callout-help{--callout-color:#448aff}.callout-tip,.callout-hint,.callout-important,.callout-example{--callout-color:#00a878}.callout-success,.callout-check,.callout-done{--callout-color:#22a06b}.callout-warning,.callout-caution,.callout-attention{--callout-color:#d99a16}.callout-failure,.callout-fail,.callout-missing,.callout-danger,.callout-error,.callout-bug{--callout-color:#db5555}.callout-quote,.callout-cite{--callout-color:#999}
table{border-collapse:collapse;display:block;overflow-x:auto}td,th{border:1px solid var(--line);padding:6px 10px}.layout{transition:grid-template-columns .12s ease}.right-sidebar-hidden .layout{grid-template-columns:minmax(0,780px)}.right-sidebar-hidden .layout>aside{display:none}.toc-actions{display:flex;gap:5px}.backlinks{margin-top:24px;padding-top:13px;border-top:1px solid var(--line)}.main-backlinks{width:90%;max-width:1016px;margin:0 auto;padding:13px 0 28px}.backlinks h2{font-size:12px;letter-spacing:.02em;text-transform:uppercase;margin:0 0 8px;color:var(--muted)}.backlink-list{display:flex;flex-wrap:wrap;align-items:center;gap:5px}.link-relations{display:table;width:100%;border-collapse:collapse;table-layout:fixed}.link-relations th{width:50%;text-align:left;color:var(--muted);font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.03em}.link-relations td{vertical-align:top;width:50%;padding:8px 10px}.link-relations th,.link-relations td{border:1px solid var(--line)}.backlink-item{display:inline-flex;align-items:center;padding:3px 7px;border:1px solid var(--line);border-radius:6px;text-decoration:none;color:var(--fg);background:var(--panel);font-size:11px;line-height:1.45;max-width:100%;overflow-wrap:anywhere}.backlink-item:hover{background:var(--hover);border-color:var(--accent)}.global-tools{position:fixed;right:12px;top:12px;z-index:40;display:flex;gap:5px}.palette-menu{position:fixed;right:12px;top:50px;z-index:41;width:230px;padding:10px;background:var(--panel);color:var(--fg);border:1px solid var(--line);border-radius:9px;box-shadow:0 12px 36px #0002}.palette-menu[hidden]{display:none}.palette-menu strong{display:block;font-size:12px;margin:2px 4px 8px}.palette-option{display:flex;align-items:center;gap:9px;width:100%;padding:7px 8px;border:0;border-radius:6px;background:transparent;color:var(--fg);text-align:left;cursor:pointer;font:inherit;font-size:12px}.palette-option:hover,.palette-option[aria-pressed=true]{background:var(--hover)}.palette-dots{display:flex;gap:3px}.palette-dots i{display:block;width:10px;height:10px;border-radius:50%}.palette-help{font-size:10px;color:var(--muted);padding:6px 4px 2px}aside{position:sticky;top:28px;align-self:start;font-size:13px;color:var(--muted);max-height:85vh;overflow:auto}.toc-heading{display:flex;align-items:center;justify-content:space-between;gap:8px;margin-bottom:9px}#heading-search{width:100%;padding:6px 8px;margin:0 0 10px;border:1px solid var(--line);border-radius:6px;background:var(--bg);color:var(--fg);font:inherit;font-size:12px;outline:none}#heading-search:focus{border-color:var(--accent)}aside b{display:block;color:var(--fg);margin:0}aside a{display:block;color:inherit;text-decoration:none;margin:5px 0;overflow-wrap:anywhere}.toc a{display:flex;align-items:stretch;gap:0;min-width:0;margin:0;padding:0;min-height:1.9em}.toc-guides{display:inline-flex;align-self:stretch;flex:0 0 auto}.toc-guides i{position:relative;display:block;width:13px;height:100%;min-height:100%;border-left:1px solid var(--line)}.toc-guides i:last-child:after{content:"";position:absolute;left:0;top:50%;width:7px;border-top:1px solid var(--line)}.toc-label{min-width:0;padding:3px 0}.toc-level-1 .toc-guides{display:none}body.right-no-wrap aside{overflow-x:auto}body.right-no-wrap aside a{white-space:nowrap;overflow-wrap:normal}
.missing{color:#c66;text-decoration:line-through}.muted,.empty{color:var(--muted)}.properties{margin:0 0 22px;border:1px solid var(--line);border-radius:7px;font-size:12px;color:var(--muted);overflow:hidden}.properties summary{cursor:pointer;padding:6px 10px;background:var(--panel);user-select:none}.properties summary span{margin-left:4px;opacity:.7}.properties dl{margin:0;padding:6px 10px;display:grid;gap:4px}.property{display:grid;grid-template-columns:minmax(90px,150px) minmax(0,1fr);gap:12px}.property dt{font-weight:600;color:var(--muted)}.property dd{margin:0;color:var(--fg);overflow-wrap:anywhere}
.content.is-loading{opacity:.65;pointer-events:none;transition:opacity .08s}.file-preview{display:flex;justify-content:center;align-items:flex-start}.file-preview img{max-width:min(100%,1100px);max-height:calc(100vh - 150px);object-fit:contain}.base-view{min-width:0}.base-note{font-size:13px;color:var(--muted);margin:0 0 12px}.base-table{width:100%;border-collapse:collapse;font-size:13px}.base-table td,.base-table th{padding:7px 9px;text-align:left;vertical-align:top}.base-table th{position:sticky;top:0;background:var(--panel)}.base-table a{text-decoration:none}.base-list{padding-left:22px}.base-list li{margin:5px 0}.base-cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(190px,1fr));gap:12px}.base-card{border:1px solid var(--line);border-radius:8px;padding:12px;background:var(--panel)}.base-card a{font-weight:600;text-decoration:none}.floating-toggle{display:none;position:fixed;z-index:20;left:12px;top:12px;border:1px solid var(--line);background:var(--panel);color:var(--fg);border-radius:8px;padding:8px 11px;cursor:pointer}.app.nav-hidden~.floating-toggle{display:block}.floating-right-toggle{display:none;position:fixed;z-index:20;right:12px;bottom:12px;border:1px solid var(--line);background:var(--panel);color:var(--fg);border-radius:8px;padding:8px 11px;cursor:pointer}.right-sidebar-hidden .floating-right-toggle{display:block}
@media(max-width:1050px){.layout{grid-template-columns:minmax(0,1fr);gap:20px}aside{position:static;display:block;max-height:none;border-top:1px solid var(--line);padding-top:14px}}
@media(max-width:680px){.app{grid-template-columns:1fr}nav{height:auto;position:static;border-right:0;border-bottom:1px solid var(--line);max-height:36vh}.layout{padding:25px 20px}.note-title{font-size:1.7rem}}
</style>
</head>
<body>
<div class="app">
<nav>
  <div class="nav-top"><a class="brand" href="/">VaultLite</a></div>
  <div class="nav-tools">
    <button class="icon-button" id="collapse-folders" type="button" title="Collapse all folders" aria-label="Collapse all folders">⊟</button>
    <button class="icon-button" id="sidebar-smaller" type="button" title="Decrease left sidebar width" aria-label="Decrease left sidebar width">◂</button>
    <button class="icon-button" id="sidebar-larger" type="button" title="Increase left sidebar width" aria-label="Increase left sidebar width">▸</button>
    <button class="icon-button" data-wrap-control="left" type="button" title="Toggle wrapping in file navigator" aria-label="Toggle wrapping in file navigator">↵</button>
    <button class="icon-button" id="font-smaller" type="button" title="Decrease note text size" aria-label="Decrease note text size">A−</button>
    <button class="icon-button" id="font-larger" type="button" title="Increase note text size" aria-label="Increase note text size">A+</button>
    <button class="icon-button" id="hide-sidebar" type="button" title="Hide left sidebar" aria-label="Hide left sidebar">◧</button>
  </div>
  <input id="vault-search" type="search" placeholder="Search files (fuzzy)…" autocomplete="off" aria-label="Fuzzy search files by name">
  <div class="tree" id="vault-tree">__NAV__</div>
</nav>
<div class="content">__CONTENT__</div>
</div>
<button class="floating-toggle" id="show-sidebar" type="button" aria-label="Show left sidebar" title="Show left sidebar">☰</button>
<button class="floating-right-toggle" id="show-right-sidebar" type="button" aria-label="Show right sidebar" title="Show right sidebar">☷</button>
<div class="global-tools"><button class="icon-button" id="theme-toggle" type="button" aria-label="Switch to dark theme" title="Switch theme">◐</button><button class="icon-button" id="palette-toggle" type="button" title="Choose color palette" aria-label="Choose color palette">⚙</button></div>
<div class="palette-menu" id="palette-menu" role="dialog" aria-label="Color palettes" hidden>
  <strong>Color palette</strong>
  <button class="palette-option" type="button" data-palette="default"><span class="palette-dots"><i style="background:#fff"></i><i style="background:#526fe5"></i><i style="background:#24272a"></i></span>Default</button>
  <button class="palette-option" type="button" data-palette="nord"><span class="palette-dots"><i style="background:#eceff4"></i><i style="background:#5e81ac"></i><i style="background:#2e3440"></i></span>Nord</button>
  <button class="palette-option" type="button" data-palette="dracula"><span class="palette-dots"><i style="background:#f8f8f2"></i><i style="background:#bd93f9"></i><i style="background:#282a36"></i></span>Dracula</button>
  <button class="palette-option" type="button" data-palette="catppuccin"><span class="palette-dots"><i style="background:#eff1f5"></i><i style="background:#8839ef"></i><i style="background:#1e1e2e"></i></span>Catppuccin</button>
  <button class="palette-option" type="button" data-palette="gruvbox"><span class="palette-dots"><i style="background:#fbf1c7"></i><i style="background:#af3a03"></i><i style="background:#282828"></i></span>Gruvbox</button>
  <button class="palette-option" type="button" data-palette="solarized"><span class="palette-dots"><i style="background:#fdf6e3"></i><i style="background:#268bd2"></i><i style="background:#002b36"></i></span>Solarized</button>
  <button class="palette-option" type="button" data-palette="tokyo-night"><span class="palette-dots"><i style="background:#d5d6db"></i><i style="background:#7aa2f7"></i><i style="background:#1a1b26"></i></span>Tokyo Night</button>
  <button class="palette-option" type="button" data-palette="rose-pine"><span class="palette-dots"><i style="background:#faf4ed"></i><i style="background:#907aa9"></i><i style="background:#191724"></i></span>Rosé Pine</button>
  <div class="palette-help">Applies to headings, links, bold, italic, and strikethrough text.</div>
</div>
<script>
(() => {
  const root = document.documentElement;
  const app = document.querySelector('.app');
  const toggle = document.getElementById('theme-toggle');
  const savedTheme = localStorage.getItem('vaultlite-theme');
  if (savedTheme === 'dark' || savedTheme === 'light') root.dataset.theme = savedTheme;
  const paletteVars = ['bg','panel','fg','muted','line','accent','code','hover','bold-color','italic-color','strike-color','link-color','h1-color','h2-color','h3-color','h4-color','h5-color','h6-color'];
  const palettes = {
    'default': {light:['#ffffff','#fafafa','#222222','#777777','#e7e7e7','#526fe5','#f3f4f6','#f1f3f8','#334b91','#8a4e91','#888888','#365fc8','#334b91','#3d5fa5','#496da8','#607da8','#66788d','#77808b'],dark:['#171819','#1d1f21','#e6e6e6','#a0a0a0','#343638','#9aafff','#24272a','#282b2e','#b9c8ff','#e2b8e8','#999999','#a8bbff','#c2d0ff','#b4c6f5','#a9c0e9','#a6b7d5','#b2bfd1','#c0c7d0']},
    'nord': {light:['#eceff4','#e5e9f0','#2e3440','#616e88','#d8dee9','#5e81ac','#e5e9f0','#d8dee9','#3b4252','#b06b9d','#7b8498','#4c6f91','#3b4252','#4c566a','#5e6b82','#687a96','#7888a0','#8895a8'],dark:['#2e3440','#3b4252','#eceff4','#aeb9cc','#4c566a','#88c0d0','#434c5e','#434c5e','#ebcb8b','#b48ead','#9aa5b8','#88c0d0','#88c0d0','#81a1c1','#8fbcbb','#a3be8c','#d08770','#e5e9f0']},
    'dracula': {light:['#f8f8f2','#eeeeeb','#282a36','#666477','#dedee5','#7652b5','#eeeeeb','#e3e1ef','#59428c','#a14e83','#777777','#6441a5','#6441a5','#7652b5','#8b5fbf','#a16ac0','#78618d','#686078'],dark:['#282a36','#343746','#f8f8f2','#a9a6b8','#44475a','#bd93f9','#343746','#44475a','#f1fa8c','#ff79c6','#a9a6b8','#8be9fd','#ff79c6','#bd93f9','#8be9fd','#50fa7b','#ffb86c','#f8f8f2']},
    'catppuccin': {light:['#eff1f5','#e6e9ef','#4c4f69','#7c7f93','#ccd0da','#8839ef','#e6e9ef','#dce0e8','#1e66f5','#ea76cb','#8c8fa1','#1e66f5','#8839ef','#7287fd','#04a5e5','#179299','#40a02b','#df8e1d'],dark:['#1e1e2e','#181825','#cdd6f4','#a6adc8','#45475a','#cba6f7','#313244','#313244','#f9e2af','#f5c2e7','#9399b2','#89b4fa','#cba6f7','#b4befe','#89dceb','#94e2d5','#a6e3a1','#f9e2af']},
    'gruvbox': {light:['#fbf1c7','#f2e5bc','#3c3836','#7c6f64','#d5c4a1','#af3a03','#f2e5bc','#ebdbb2','#076678','#8f3f71','#928374','#076678','#9d0006','#b57614','#79740e','#427b58','#076678','#8f3f71'],dark:['#282828','#3c3836','#ebdbb2','#a89984','#504945','#fabd2f','#3c3836','#504945','#fabd2f','#d3869b','#928374','#83a598','#fb4934','#fabd2f','#b8bb26','#8ec07c','#83a598','#d3869b']},
    'solarized': {light:['#fdf6e3','#eee8d5','#586e75','#839496','#ddd6c1','#268bd2','#eee8d5','#eee8d5','#b58900','#d33682','#93a1a1','#268bd2','#b58900','#cb4b16','#859900','#2aa198','#268bd2','#6c71c4'],dark:['#002b36','#073642','#eee8d5','#93a1a1','#586e75','#268bd2','#073642','#073642','#b58900','#d33682','#657b83','#268bd2','#b58900','#cb4b16','#859900','#2aa198','#268bd2','#6c71c4']},
    'tokyo-night': {light:['#e1e2e7','#d5d6db','#343b58','#697098','#c4c8da','#34548a','#d5d6db','#c8cbe0','#34548a','#9854a6','#777f9b','#34548a','#34548a','#485eaf','#5a6da8','#4c7899','#6b6fa5','#777f9b'],dark:['#1a1b26','#24283b','#c0caf5','#9aa5ce','#3b4261','#7aa2f7','#24283b','#292e42','#e0af68','#bb9af7','#7982a9','#7dcfff','#7aa2f7','#bb9af7','#7dcfff','#9ece6a','#e0af68','#c0caf5']},
    'rose-pine': {light:['#faf4ed','#f2e9e1','#575279','#797593','#dfdad9','#907aa9','#f2e9e1','#f2e9e1','#286983','#d7827e','#9893a5','#56949f','#907aa9','#286983','#ea9d34','#56949f','#d7827e','#797593'],dark:['#191724','#1f1d2e','#e0def4','#908caa','#403d52','#c4a7e7','#26233a','#26233a','#f6c177','#ebbcba','#6e6a86','#c4a7e7','#c4a7e7','#9ccfd8','#f6c177','#ebbcba','#31748f','#e0def4']}
  };
  const paletteMenu = document.getElementById('palette-menu');
  const paletteToggle = document.getElementById('palette-toggle');
  const activePalette = () => localStorage.getItem('vaultlite-palette') || 'default';
  const applyPalette = name => {
    const palette = palettes[name] || palettes.default;
    const values = palette[root.dataset.theme === 'dark' ? 'dark' : 'light'];
    values.forEach((value, index) => root.style.setProperty('--' + paletteVars[index], value));
    localStorage.setItem('vaultlite-palette', name);
    paletteMenu.querySelectorAll('[data-palette]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.palette === name)));
  };
  const updateThemeButton = () => {
    const dark = root.dataset.theme === 'dark';
    toggle.textContent = dark ? '☀' : '☾';
    toggle.title = dark ? 'Switch to light theme' : 'Switch to dark theme';
    toggle.setAttribute('aria-label', toggle.title);
    applyPalette(activePalette());
  };
  updateThemeButton();
  toggle.addEventListener('click', () => {
    root.dataset.theme = root.dataset.theme === 'dark' ? 'light' : 'dark';
    localStorage.setItem('vaultlite-theme', root.dataset.theme);
    updateThemeButton();
  });
  paletteToggle.addEventListener('click', () => { paletteMenu.hidden = !paletteMenu.hidden; });
  paletteMenu.querySelectorAll('[data-palette]').forEach(button => button.addEventListener('click', () => {
    applyPalette(button.dataset.palette);
    paletteMenu.hidden = true;
  }));
  document.addEventListener('click', event => {
    if (!paletteMenu.hidden && !paletteMenu.contains(event.target) && !paletteToggle.contains(event.target)) paletteMenu.hidden = true;
  });

  const search = document.getElementById('vault-search');
  const tree = document.getElementById('vault-tree');
  const folders = () => Array.from(tree.querySelectorAll('details.folder'));
  const readFolderState = () => {
    try { return JSON.parse(localStorage.getItem('vaultlite-folder-state') || '{}'); }
    catch (_) { return {}; }
  };
  const folderState = readFolderState();
  const saveFolderState = () => {
    const state = {};
    folders().forEach(folder => { state[folder.dataset.path] = folder.open; });
    localStorage.setItem('vaultlite-folder-state', JSON.stringify(state));
  };
  folders().forEach(folder => {
    if (Object.prototype.hasOwnProperty.call(folderState, folder.dataset.path)) folder.open = !!folderState[folder.dataset.path];
    folder.addEventListener('toggle', saveFolderState);
  });
  document.getElementById('collapse-folders').addEventListener('click', () => {
    folders().forEach(folder => { folder.open = false; });
    saveFolderState();
  });

  let sidebarWidth = Math.max(190, Math.min(520, Number(localStorage.getItem('vaultlite-sidebar-width')) || 270));
  const setSidebarWidth = value => {
    sidebarWidth = Math.max(190, Math.min(520, value));
    root.style.setProperty('--sidebar-width', sidebarWidth + 'px');
    localStorage.setItem('vaultlite-sidebar-width', String(sidebarWidth));
  };
  setSidebarWidth(sidebarWidth);
  document.getElementById('sidebar-smaller').addEventListener('click', () => setSidebarWidth(sidebarWidth - 30));
  document.getElementById('sidebar-larger').addEventListener('click', () => setSidebarWidth(sidebarWidth + 30));

  let noteFontSize = Math.max(12, Math.min(24, Number(localStorage.getItem('vaultlite-note-font-size')) || 16));
  const setNoteFontSize = value => {
    noteFontSize = Math.max(12, Math.min(24, value));
    root.style.setProperty('--note-font-size', noteFontSize + 'px');
    localStorage.setItem('vaultlite-note-font-size', String(noteFontSize));
    document.getElementById('font-smaller').title = 'Decrease note text size (' + noteFontSize + 'px)';
    document.getElementById('font-larger').title = 'Increase note text size (' + noteFontSize + 'px)';
  };
  setNoteFontSize(noteFontSize);
  document.getElementById('font-smaller').addEventListener('click', () => setNoteFontSize(noteFontSize - 1));
  document.getElementById('font-larger').addEventListener('click', () => setNoteFontSize(noteFontSize + 1));

  const setSideWrap = (side, noWrap) => {
    const className = side + '-no-wrap';
    document.body.classList.toggle(className, noWrap);
    localStorage.setItem('vaultlite-' + side + '-no-wrap', noWrap ? '1' : '0');
    document.querySelectorAll('[data-wrap-control="' + side + '"]').forEach(button => {
      const short = side === 'left' ? 'L' : 'R';
      button.textContent = noWrap ? '↵' : '↔';
      button.title = noWrap ? 'Enable wrapping in the ' + (side === 'left' ? 'file navigator' : 'table of contents') : 'Disable wrapping in the ' + (side === 'left' ? 'file navigator' : 'table of contents');
      button.setAttribute('aria-label', button.title);
    });
  };
  setSideWrap('left', localStorage.getItem('vaultlite-left-no-wrap') === '1');
  setSideWrap('right', localStorage.getItem('vaultlite-right-no-wrap') === '1');
  document.addEventListener('click', event => {
    const control = event.target.closest && event.target.closest('[data-wrap-control]');
    if (!control) return;
    const side = control.dataset.wrapControl;
    const noWrap = !document.body.classList.contains(side + '-no-wrap');
    setSideWrap(side, noWrap);
  });

  if (localStorage.getItem('vaultlite-sidebar-hidden') === '1') app.classList.add('nav-hidden');
  document.getElementById('hide-sidebar').addEventListener('click', () => {
    app.classList.add('nav-hidden');
    localStorage.setItem('vaultlite-sidebar-hidden', '1');
  });
  document.getElementById('show-sidebar').addEventListener('click', () => {
    app.classList.remove('nav-hidden');
    localStorage.setItem('vaultlite-sidebar-hidden', '0');
  });

  const setRightSidebarHidden = hidden => {
    document.body.classList.toggle('right-sidebar-hidden', hidden);
    localStorage.setItem('vaultlite-right-sidebar-hidden', hidden ? '1' : '0');
  };
  setRightSidebarHidden(localStorage.getItem('vaultlite-right-sidebar-hidden') === '1');
  document.addEventListener('click', event => {
    if (event.target.closest && event.target.closest('#hide-right-sidebar')) setRightSidebarHidden(true);
    if (event.target.closest && event.target.closest('#show-right-sidebar')) setRightSidebarHidden(false);
  });

  const editDistanceWithin = (a, b, limit) => {
    if (a === b) return 0;
    if (Math.abs(a.length - b.length) > limit) return limit + 1;
    let previous = Array.from({length: b.length + 1}, (_, i) => i);
    for (let i = 1; i <= a.length; i++) {
      const current = Array(b.length + 1).fill(limit + 1);
      current[0] = i;
      const start = Math.max(1, i - limit);
      const end = Math.min(b.length, i + limit);
      let rowMin = current[0];
      for (let j = start; j <= end; j++) {
        current[j] = Math.min(current[j - 1] + 1, previous[j] + 1, previous[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
        rowMin = Math.min(rowMin, current[j]);
      }
      if (rowMin > limit) return limit + 1;
      previous = current;
    }
    return previous[b.length];
  };
  const subsequence = (needle, haystack) => {
    let at = 0;
    for (const char of haystack) if (char === needle[at]) at++;
    return at === needle.length;
  };
  const fuzzyMatch = (query, words) => {
    const queries = query.toLocaleLowerCase().split(/\s+/).filter(Boolean);
    return queries.every(part => words.some(word => {
      if (word.includes(part) || subsequence(part, word)) return true;
      const threshold = part.length < 4 ? 1 : Math.min(2, Math.floor(part.length * 0.2));
      return editDistanceWithin(part, word, threshold) <= threshold;
    }));
  };
  tree.querySelectorAll('a.file').forEach(node => {
    // This box searches filenames only; folders are retained solely as the
    // hierarchy needed to display matching files.
    node._vaultWords = (node.dataset.name || '').toLocaleLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  });
  const filterNode = (node, query) => {
    if (node.matches('a.file')) {
      const match = !query || fuzzyMatch(query, node._vaultWords || []);
      node.style.display = match ? '' : 'none';
      return match;
    }
    if (node.matches('details.folder')) {
      const children = node.querySelector(':scope > .folder-children');
      let childMatch = false;
      if (children) Array.from(children.children).forEach(child => { if (filterNode(child, query)) childMatch = true; });
      node.style.display = (!query || childMatch) ? '' : 'none';
      if (query && childMatch) node.open = true;
      return !query || childMatch;
    }
    return true;
  };
  const applySearch = () => {
    const query = search.value.trim();
    Array.from(tree.children).forEach(node => filterNode(node, query));
  };
  const setupHeadingSearch = () => {
    const headingSearch = document.getElementById('heading-search');
    const toc = document.querySelector('.toc');
    if (!headingSearch || !toc) return;
    headingSearch.addEventListener('input', () => {
      const query = headingSearch.value.trim().toLocaleLowerCase();
      toc.querySelectorAll('a').forEach(link => {
        link.style.display = !query || link.textContent.toLocaleLowerCase().includes(query) ? '' : 'none';
      });
    });
    headingSearch.addEventListener('keydown', event => {
      if (event.key === 'Escape') { headingSearch.value = ''; headingSearch.dispatchEvent(new Event('input')); headingSearch.blur(); }
    });
  };
  setupHeadingSearch();
  let searchTimer = 0;
  search.addEventListener('input', () => { window.clearTimeout(searchTimer); searchTimer = window.setTimeout(applySearch, 45); });
  search.addEventListener('keydown', event => {
    if (event.key === 'Escape') { search.value = ''; applySearch(); search.blur(); }
  });
  const syncActiveFile = () => {
    const current = location.pathname;
    tree.querySelectorAll('a.file').forEach(link => {
      let path = new URL(link.href, location.href).pathname;
      if (path.startsWith('/file/') && path.toLowerCase().endsWith('.md')) path = path.replace(/^\/file\//, '/note/');
      link.classList.toggle('active', path === current);
    });
  };
  let backlinkLoadToken = 0;
  const hydrateBacklinks = async encodedPath => {
    const token = ++backlinkLoadToken;
    const container = document.getElementById('backlinks');
    if (!container) return;
    const route = '/backlinks/' + encodedPath;
    for (let attempt = 0; attempt < 90; attempt++) {
      try {
        const response = await fetch(route, {cache: 'no-store'});
        const html = await response.text();
        if (token !== backlinkLoadToken || !document.getElementById('backlinks')) return;
        if (response.status === 202) {
          if (attempt % 5 === 0) container.innerHTML = html;
          await new Promise(resolve => setTimeout(resolve, 650));
          continue;
        }
        if (response.ok) container.outerHTML = html;
        else container.innerHTML = '<table class="link-relations"><tbody><tr><td>Unable to load links.</td></tr></tbody></table>';
        return;
      } catch (_) {
        if (token !== backlinkLoadToken) return;
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
    }
    if (token === backlinkLoadToken && document.getElementById('backlinks')) {
      container.innerHTML = '<table class="link-relations"><tbody><tr><td>Link index is still being prepared.</td></tr></tbody></table>';
    }
  };
  const initialNotePath = location.pathname.startsWith('/note/') ? location.pathname.slice('/note/'.length) :
    (location.pathname.startsWith('/file/') && location.pathname.toLowerCase().endsWith('.md') ? location.pathname.slice('/file/'.length) : null);
  if (initialNotePath) hydrateBacklinks(initialNotePath);

  let navigationController = null;
  const navigateNote = async (url, pushState) => {
    const target = new URL(url, location.href);
    let path = target.pathname;
    if (path.startsWith('/file/') && path.toLowerCase().endsWith('.md')) path = path.replace(/^\/file\//, '/note/');
    const kind = path.startsWith('/note/') ? 'note' : path.startsWith('/image/') ? 'image' : path.startsWith('/base/') ? 'base' : null;
    if (!kind) return false;
    if (navigationController) navigationController.abort();
    navigationController = new AbortController();
    const content = document.querySelector('.content');
    content.classList.add('is-loading');
    try {
      const encodedPath = path.replace(/^\/(note|image|base)\//, '');
      const fragmentPath = '/fragment/' + kind + '/' + encodedPath;
      const response = await fetch(fragmentPath, {signal: navigationController.signal, cache: 'default'});
      if (!response.ok) throw new Error('Unable to load note');
      const html = await response.text();
      content.innerHTML = html;
      content.classList.remove('is-loading');
      if (pushState) history.pushState({}, '', path + target.search + target.hash);
      const title = content.querySelector('.note-title')?.textContent || 'Vault';
      document.title = title + ' · VaultLite';
      setSideWrap('right', localStorage.getItem('vaultlite-right-no-wrap') === '1');
      setRightSidebarHidden(localStorage.getItem('vaultlite-right-sidebar-hidden') === '1');
      setupHeadingSearch();
      hydrateBacklinks(encodedPath);
      syncActiveFile();
      if (target.hash) {
        requestAnimationFrame(() => document.getElementById(decodeURIComponent(target.hash.slice(1)))?.scrollIntoView());
      } else {
        window.scrollTo(0, 0);
      }
      return true;
    } catch (error) {
      content.classList.remove('is-loading');
      if (error.name !== 'AbortError') location.assign(path + target.search + target.hash);
      return false;
    }
  };
  document.addEventListener('click', event => {
    const anchor = event.target.closest && event.target.closest('a[href]');
    if (!anchor || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || anchor.target) return;
    try {
      const target = new URL(anchor.href, location.href);
      const localHost = target.hostname === '127.0.0.1' || target.hostname === 'localhost' || target.hostname === '::1';
      if (localHost && (target.pathname.startsWith('/note/') || target.pathname.startsWith('/image/') || target.pathname.startsWith('/base/') || (target.pathname.startsWith('/file/') && target.pathname.toLowerCase().endsWith('.md')))) {
        event.preventDefault();
        navigateNote(target.href, true);
      }
    } catch (_) {}
  });
  window.addEventListener('popstate', () => {
    if (location.pathname.startsWith('/note/') || location.pathname.startsWith('/image/') || location.pathname.startsWith('/base/')) navigateNote(location.href, false);
    else location.reload();
  });
  syncActiveFile();
  document.addEventListener('keydown', event => {
    if (event.key === '/' && !['INPUT','TEXTAREA'].includes(document.activeElement.tagName)) {
      event.preventDefault(); search.focus();
    }
  });
})();
</script>
</body>
</html>"##;
    template
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
        Err(_) => send_text(request, 500, "Unable to read image".into(), "text/plain; charset=utf-8"),
    }
}

fn respond(request: tiny_http::Request, state: &AppState) {
    let url = request.url().split('?').next().unwrap_or("/");

    if let Some(encoded_path) = url.strip_prefix("/backlinks/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if !is_markdown(&path) {
                send_text(request, 404, "Not a Markdown note".into(), "text/plain; charset=utf-8");
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
            send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
        }
        return;
    }

    // Fragment responses preserve the app shell and are used for quick in-page
    // navigation among notes, images, and Bases views.
    if let Some(fragment_route) = url.strip_prefix("/fragment/") {
        if let Some((kind, encoded_path)) = fragment_route.split_once('/') {
            if let Some(path) = safe_join(&state.root, encoded_path) {
                if !path.is_file() {
                    send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
                    return;
                }
                match kind {
                    "note" if is_markdown(&path) => match cached_note(state, &path) {
                        Ok((_title, content)) => send_text(request, 200, content, "text/html; charset=utf-8"),
                        Err(_) => send_text(request, 500, "Unable to render note".into(), "text/plain; charset=utf-8"),
                    },
                    "image" if is_image(&path) => {
                        let content = render_image(&state.root, &path);
                        send_text(request, 200, content, "text/html; charset=utf-8");
                    }
                    "base" if is_base(&path) => {
                        let content = render_base(&state.root, &state.index, &path);
                        send_text(request, 200, content, "text/html; charset=utf-8");
                    }
                    _ => send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8"),
                }
            } else {
                send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
            }
        } else {
            send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/asset/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file() && is_image(&path) {
                send_inline_image(request, &path);
            } else {
                send_text(request, 404, "Not an image".into(), "text/plain; charset=utf-8");
            }
        } else {
            send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/file/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file() {
                if is_markdown(&path) {
                    match cached_note(state, &path) {
                        Ok((title, content)) => {
                            let body = page(&title, &content, &state.nav_html);
                            send_text(request, 200, body, "text/html; charset=utf-8");
                        }
                        Err(_) => send_text(request, 500, "Unable to render note".into(), "text/plain; charset=utf-8"),
                    }
                    return;
                }
                if is_base(&path) {
                    let content = render_base(&state.root, &state.index, &path);
                    let title = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    let body = page(&title, &content, &state.nav_html);
                    send_text(request, 200, body, "text/html; charset=utf-8");
                    return;
                }
                if is_image(&path) {
                    send_inline_image(request, &path);
                    return;
                }
                match fs::read(&path) {
                    Ok(bytes) => {
                        let mut response = Response::from_data(bytes).with_status_code(StatusCode(200));
                        if let Ok(header) = Header::from_bytes("Content-Type", content_type(&path)) {
                            response = response.with_header(header);
                        }
                        if let Ok(header) = Header::from_bytes("X-Content-Type-Options", "nosniff") {
                            response = response.with_header(header);
                        }
                        if let Ok(header) = Header::from_bytes("Cache-Control", "public, max-age=3600") {
                            response = response.with_header(header);
                        }
                        if is_pdf(&path) {
                            if let Ok(header) = Header::from_bytes("Content-Disposition", "inline") {
                                response = response.with_header(header);
                            }
                        }
                        let _ = request.respond(response);
                        return;
                    }
                    Err(_) => {
                        send_text(request, 500, "Unable to read file".into(), "text/plain; charset=utf-8");
                        return;
                    }
                }
            }
        }
        send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
        return;
    }

    if url == "/" {
        if let Some(index_path) = &state.index_page {
            let full_path = state.root.join(index_path);
            match cached_note(state, &full_path) {
                Ok((title, content)) => {
                    let body = page(&title, &content, &state.nav_html);
                    send_text(request, 200, body, "text/html; charset=utf-8");
                }
                Err(_) => send_text(request, 500, "Unable to render index note".into(), "text/plain; charset=utf-8"),
            }
            return;
        }
        let content = "<div class=\"layout\"><main><h1 class=\"note-title\">Vault</h1><p class=\"empty\">Browse folders and files from the navigator, or search above.</p></main></div>";
        let body = page("Vault", content, &state.nav_html);
        send_text(request, 200, body, "text/html; charset=utf-8");
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/image/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if is_image(&path) {
                let content = render_image(&state.root, &path);
                let title = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let body = page(&title, &content, &state.nav_html);
                send_text(request, 200, body, "text/html; charset=utf-8");
            } else {
                send_text(request, 404, "Not an image".into(), "text/plain; charset=utf-8");
            }
        } else {
            send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/base/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if is_base(&path) {
                let content = render_base(&state.root, &state.index, &path);
                let title = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                let body = page(&title, &content, &state.nav_html);
                send_text(request, 200, body, "text/html; charset=utf-8");
            } else {
                send_text(request, 404, "Not a Bases file".into(), "text/plain; charset=utf-8");
            }
        } else {
            send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
        }
        return;
    }

    if let Some(encoded_path) = url.strip_prefix("/note/") {
        if let Some(path) = safe_join(&state.root, encoded_path) {
            if path.is_file() && path.extension().and_then(|v| v.to_str()).map(|v| v.eq_ignore_ascii_case("md")).unwrap_or(false) {
                match cached_note(state, &path) {
                    Ok((title, content)) => {
                        let body = page(&title, &content, &state.nav_html);
                        send_text(request, 200, body, "text/html; charset=utf-8");
                    }
                    Err(_) => send_text(request, 500, "Unable to render note".into(), "text/plain; charset=utf-8"),
                }
            } else {
                send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
            }
        } else {
            send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
        }
        return;
    }

    send_text(request, 404, "Not found".into(), "text/plain; charset=utf-8");
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
    let state = Arc::new(AppState {
        root: root.clone(),
        index: vault_index,
        nav_html,
        index_page: index,
        note_cache: Mutex::new(NoteCache::default()),
        backlinks: RwLock::new(BacklinkIndexState::default()),
    });

    // Build backlinks off the request path; note rendering stays responsive while
    // this one-time index is prepared in the background.
    let backlink_state = Arc::clone(&state);
    thread::spawn(move || {
        let (by_target, by_source) = build_backlink_index(&backlink_state.root, &backlink_state.index);
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
    fn drop(&mut self) { self.stop(); }
}

fn start_server_for_vault(vault: &Path, index_page: Option<&str>) -> Result<RunningServer, String> {
    let root = vault.canonicalize().map_err(|e| format!("Cannot open vault: {e}"))?;
    if !root.is_dir() { return Err("Selected path is not a folder".into()); }
    let index = if let Some(value) = index_page.filter(|v| !v.trim().is_empty()) {
        let relative = PathBuf::from(value.trim());
        if relative.is_absolute() || relative.components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
            return Err("Homepage must be a path inside the selected vault".into());
        }
        if !root.join(&relative).is_file() { return Err(format!("Homepage note not found: {}", relative.display())); }
        Some(relative)
    } else { None };

    let vault_index = VaultIndex::build(&root);
    let nav_html = nav(&root, Path::new(""));
    let state = Arc::new(AppState {
        root: root.clone(), index: vault_index, nav_html, index_page: index,
        note_cache: Mutex::new(NoteCache::default()),
        backlinks: RwLock::new(BacklinkIndexState::default()),
    });
    let backlink_state = Arc::clone(&state);
    thread::spawn(move || {
        let (by_target, by_source) = build_backlink_index(&backlink_state.root, &backlink_state.index);
        if let Ok(mut guard) = backlink_state.backlinks.write() {
            guard.by_target = by_target;
            guard.by_source = by_source;
            guard.ready = true;
        }
    });

    // Bind port zero to let the OS choose a free localhost port.
    let probe = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("Cannot bind localhost: {e}"))?;
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
                Ok(None) => {},
                Err(_) if stop_worker.load(Ordering::Acquire) => break,
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        }
    });
    Ok(RunningServer { url, stop, worker: Some(worker) })
}


fn preferences_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    let base = env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(env::temp_dir);
    #[cfg(target_os = "macos")]
    let base = env::var_os("HOME").map(PathBuf::from)
        .map(|p| p.join("Library").join("Application Support")).unwrap_or_else(env::temp_dir);
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .unwrap_or_else(env::temp_dir);
    base.join("vaultlite").join("launcher.conf")
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
        let Ok(text) = fs::read_to_string(preferences_path()) else { return p; };
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("vault=") { p.last_vault = Some(PathBuf::from(v)); }
            else if let Some(v) = line.strip_prefix("homepage=") { p.last_homepage = v.to_string(); }
            else if let Some(v) = line.strip_prefix("recent_vault=") {
                let path = PathBuf::from(v);
                if is_valid_vault(&path) && !p.recent_vaults.contains(&path) { p.recent_vaults.push(path); }
            } else if let Some(v) = line.strip_prefix("recent_homepage=") {
                if !v.is_empty() && !p.recent_homepages.iter().any(|x| x == v) { p.recent_homepages.push(v.to_string()); }
            }
        }
        if let Some(v) = p.last_vault.clone() {
            if is_valid_vault(&v) { p.recent_vaults.retain(|x| x != &v); p.recent_vaults.insert(0, v); }
        }
        if !p.last_homepage.is_empty() {
            let v = p.last_homepage.clone();
            p.recent_homepages.retain(|x| x != &v); p.recent_homepages.insert(0, v);
        }
        p.recent_vaults.truncate(8); p.recent_homepages.truncate(8); p
    }
    fn save(&self) {
        let path = preferences_path();
        if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
        let mut out = String::new();
        if let Some(v) = &self.last_vault { out.push_str(&format!("vault={}\n", v.display())); }
        out.push_str(&format!("homepage={}\n", self.last_homepage));
        for v in &self.recent_vaults { out.push_str(&format!("recent_vault={}\n", v.display())); }
        for v in &self.recent_homepages { out.push_str(&format!("recent_homepage={}\n", v)); }
        let _ = fs::write(path, out);
    }
    fn remember_vault(&mut self, path: PathBuf) {
        self.recent_vaults.retain(|v| v != &path);
        self.recent_vaults.insert(0, path.clone()); self.recent_vaults.truncate(8);
        self.last_vault = Some(path); self.save();
    }
    fn remember_homepage(&mut self, value: String) {
        self.recent_homepages.retain(|v| v != &value);
        if !value.is_empty() { self.recent_homepages.insert(0, value.clone()); self.recent_homepages.truncate(8); }
        self.last_homepage = value; self.save();
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
        Self { vault, homepage, server: None,
            status: "Choose an Obsidian vault or select a recent one.".into(), preferences }
    }
}
impl VaultLiteDesktop {
    fn choose_vault(&mut self) {
        let start = self.vault.as_ref().or(self.preferences.last_vault.as_ref())
            .and_then(|p| p.to_str()).unwrap_or("");
        if let Some(selected) = tinyfiledialogs::select_folder_dialog("Select Obsidian vault folder", start) {
            let path = PathBuf::from(selected);
            if is_valid_vault(&path) {
                self.vault = Some(path.clone()); self.preferences.remember_vault(path);
                self.status = "Vault selected. Start the server when ready.".into();
            } else {
                self.status = "This folder is not an Obsidian vault: it does not contain a .obsidian folder. It was not added to Recent vaults.".into();
            }
        }
    }
    fn choose_homepage(&mut self) {
        let start = self.vault.as_ref().and_then(|p| p.to_str()).unwrap_or("");
        if let Some(selected) = tinyfiledialogs::open_file_dialog(
            "Select homepage Markdown file", start, Some((&["*.md", "*.markdown"], "Markdown files"))) {
            let path = PathBuf::from(selected);
            let value = self.vault.as_ref().and_then(|v| path.strip_prefix(v).ok())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            self.homepage = value.clone(); self.preferences.remember_homepage(value);
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
