use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

/// Commands that the web UI can perform. The identifiers are shared with
/// `assets/app.js`; the defaults are written to `commands.conf` on first run.
pub const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("open-command-palette", "Ctrl+P"),
    ("open-search", "Ctrl+O"),
    ("toggle-left-sidebar", "Alt+Z"),
    ("toggle-right-sidebar", "Alt+X"),
    ("toggle-toc-level-one", ""),
    ("toggle-theme", "Alt+T"),
    ("collapse-folders", "Alt+C"),
    ("expand-folders", "Alt+Shift+C"),
    ("font-increase", "Alt+="),
    ("font-decrease", "Alt+-"),
    ("reset-font-size", "Alt+0"),
    ("toggle-left-wrap", "Alt+W"),
    ("toggle-right-wrap", "Alt+Shift+W"),
    ("choose-palette", "Alt+P"),
    ("go-home", "Alt+H"),
    // These commands are available from the palette and have no default
    // shortcut, so they cannot be triggered accidentally.
    ("toggle-justify", ""),
    ("copy-note", ""),
    ("open-in-obsidian", ""),
    ("exit-app", ""),
    ("open-config-folder", ""),
];

const COMMANDS_FILE: &str = "commands.conf";

/// Returns the platform configuration directory used by VaultLite.
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    let base = env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    #[cfg(target_os = "macos")]
    let base = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join("Library").join("Application Support"))
        .unwrap_or_else(env::temp_dir);
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|path| PathBuf::from(path).join(".config")))
        .unwrap_or_else(env::temp_dir);
    base.join("vaultlite")
}

fn commands_path() -> PathBuf {
    env::var_os("VAULTLITE_COMMANDS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir().join(COMMANDS_FILE))
}

/// User-editable command bindings, loaded once per server start.
#[derive(Clone, Debug)]
pub struct UiConfig {
    bindings: BTreeMap<String, String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            bindings: DEFAULT_BINDINGS
                .iter()
                .map(|(id, keys)| ((*id).to_string(), (*keys).to_string()))
                .collect(),
        }
    }
}

impl UiConfig {
    /// Load `commands.conf`, creating a documented template when it is missing.
    pub fn load() -> Self {
        let path = commands_path();
        if !path.exists() {
            let _ = write_template(&path);
        }
        match fs::read_to_string(&path) {
            Ok(text) => Self::from_text(&text),
            Err(_) => Self::default(),
        }
    }

    fn from_text(text: &str) -> Self {
        let mut config = Self::default();
        let mut in_commands = true;
        for (index, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') {
                let section = line
                    .trim_matches(|character| character == '[' || character == ']')
                    .trim();
                in_commands = section.is_empty() || section.eq_ignore_ascii_case("commands");
                continue;
            }
            if !in_commands {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                eprintln!(
                    "commands.conf: expected `command = shortcut` on line {}",
                    index + 1
                );
                continue;
            };
            let id = key
                .trim()
                .strip_prefix("command.")
                .unwrap_or(key.trim())
                .to_ascii_lowercase();
            if !DEFAULT_BINDINGS.iter().any(|(known, _)| *known == id) {
                eprintln!(
                    "commands.conf: unknown command `{id}` on line {}",
                    index + 1
                );
                continue;
            }
            let value = value
                .trim()
                .trim_matches(|character| character == '"' || character == '\'');
            if is_disabled(value) {
                config.bindings.insert(id, String::new());
                continue;
            }
            match normalized_shortcut(value) {
                Some(shortcut) => {
                    config.bindings.insert(id, shortcut);
                }
                None => eprintln!(
                    "commands.conf: invalid shortcut `{value}` for `{id}` on line {}; keeping the default",
                    index + 1
                ),
            }
        }
        config
    }

    /// JSON consumed by the browser module. Values are re-escaped even though
    /// this module validates the structure, so manually edited files stay safe.
    #[cfg(test)]
    pub fn to_json(&self) -> String {
        self.to_json_with("", "")
    }

    /// Runtime configuration contains the shortcut bindings plus the vault
    /// name used by Obsidian URIs and the per-session token required by
    /// state-changing local endpoints.
    pub fn to_json_with(&self, vault_name: &str, token: &str) -> String {
        let mut output = String::from("{\"vaultName\":\"");
        output.push_str(&json_escape(vault_name));
        output.push_str("\",\"token\":\"");
        output.push_str(&json_escape(token));
        output.push_str("\",\"bindings\":{");
        for (index, (id, shortcut)) in self.bindings.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push('"');
            output.push_str(&json_escape(id));
            output.push_str("\":\"");
            output.push_str(&json_escape(shortcut));
            output.push('"');
        }
        output.push_str("}}");
        output
    }
}

fn is_disabled(value: &str) -> bool {
    value.is_empty()
        || value.eq_ignore_ascii_case("off")
        || value.eq_ignore_ascii_case("none")
        || value.eq_ignore_ascii_case("disabled")
}

/// Accept the modifier names users expect and reject typos early. `=` is used
/// for the plus key because `+` is the chord separator.
fn normalized_shortcut(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if is_disabled(raw) {
        return Some(String::new());
    }
    let mut parts = raw.split('+').map(str::trim).collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }
    let key = parts.pop()?;
    if key.is_empty() {
        return None;
    }
    for modifier in parts {
        if !matches!(
            modifier.to_ascii_lowercase().as_str(),
            "ctrl" | "control" | "alt" | "option" | "shift" | "meta" | "cmd" | "command" | "super"
        ) {
            return None;
        }
    }
    Some(raw.to_string())
}

fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '<' => output.push_str("\\u003c"),
            '>' => output.push_str("\\u003e"),
            '&' => output.push_str("\\u0026"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output
}

fn write_template(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::from(
        "# VaultLite keyboard shortcuts\n\
         #\n\
         # Edit this file and restart VaultLite to apply changes.\n\
         # Format: command-id = modifiers+key\n\
         # Modifiers: Ctrl, Alt, Shift, Meta. Use `=` for the plus key.\n\
         # Set a command to `off` to disable its shortcut.\n\
         #\n\
         # Available commands:\n\
         #   open-command-palette, open-search, toggle-left-sidebar,\n\
         #   toggle-right-sidebar, toggle-theme, collapse-folders,\n\
         #   expand-folders, font-increase, font-decrease, reset-font-size,\n\
         #   toggle-left-wrap, toggle-right-wrap, choose-palette, go-home,\n\
         #   toggle-toc-level-one, toggle-justify, copy-note,\n\
         #   open-in-obsidian, exit-app, open-config-folder\n\
         \n\
         [commands]\n",
    );
    for (id, keys) in DEFAULT_BINDINGS {
        output.push_str(id);
        output.push_str(" = ");
        // Empty defaults mean palette-only commands.
        output.push_str(if keys.is_empty() { "off" } else { keys });
        output.push('\n');
    }
    fs::write(path, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_required_shortcuts() {
        let config = UiConfig::default();
        assert_eq!(
            config
                .bindings
                .get("open-command-palette")
                .map(String::as_str),
            Some("Ctrl+P")
        );
        assert_eq!(
            config.bindings.get("open-search").map(String::as_str),
            Some("Ctrl+O")
        );
        assert_eq!(
            config
                .bindings
                .get("toggle-left-sidebar")
                .map(String::as_str),
            Some("Alt+Z")
        );
        assert_eq!(
            config
                .bindings
                .get("toggle-right-sidebar")
                .map(String::as_str),
            Some("Alt+X")
        );
    }

    #[test]
    fn overrides_and_disabling_are_supported() {
        let config = UiConfig::from_text(
            "[commands]\n\
             toggle-left-sidebar = Ctrl+Shift+L\n\
             toggle-right-sidebar = off\n",
        );
        assert_eq!(
            config
                .bindings
                .get("toggle-left-sidebar")
                .map(String::as_str),
            Some("Ctrl+Shift+L")
        );
        assert_eq!(
            config
                .bindings
                .get("toggle-right-sidebar")
                .map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn unknown_and_invalid_entries_keep_defaults() {
        let config = UiConfig::from_text(
            "[commands]\n\
             unknown-command = Ctrl+U\n\
             toggle-theme = NotARealModifier+T\n",
        );
        assert!(!config.bindings.contains_key("unknown-command"));
        assert_eq!(
            config.bindings.get("toggle-theme").map(String::as_str),
            Some("Alt+T")
        );
    }

    #[test]
    fn json_output_is_escaped() {
        let mut config = UiConfig::default();
        config
            .bindings
            .insert("toggle-theme".into(), "Ctrl+\"".into());
        let json = config.to_json();
        assert!(json.contains("Ctrl+\\\""));
    }

    #[test]
    fn runtime_json_includes_vault_and_token() {
        let config = UiConfig::default();
        let json = config.to_json_with("My \"Vault\"", "abc123");
        assert!(json.starts_with("{\"vaultName\":\"My \\\"Vault\\\"\""));
        assert!(json.contains("\"token\":\"abc123\""));
        assert!(json.contains("\"copy-note\":\"\""));
        assert!(json.contains("\"open-in-obsidian\":\"\""));
        assert!(json.contains("\"exit-app\":\"\""));
        assert!(json.contains("\"open-config-folder\":\"\""));
    }

    #[test]
    fn json_escapes_html_script_delimiters() {
        let config = UiConfig::default();
        let json = config.to_json_with("</script><script>", "a&b");
        assert!(!json.contains("</script>"));
        assert!(json.contains("\\u003c/script\\u003e\\u003cscript\\u003e"));
        assert!(json.contains("a\\u0026b"));
    }
}
