//! Settings the user can change from the interface, kept in one small JSON file.
//!
//! Location: `hongshi.settings.json` next to the executable, falling back to the
//! user's home directory when that directory is not writable. Next to the
//! executable is deliberate — the shell is a portable download, and a setting that
//! travels with the folder it was unzipped into is easier to explain than one
//! hidden in `%APPDATA%`.
//!
//! There is no JSON dependency: the file is five flat values, and the shape is
//! fixed by [`Settings`]. Parsing is done by hand and every field falls back to
//! its default rather than failing, because a settings file is not worth refusing
//! to start over.

use std::path::PathBuf;

/// What the interface may change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Base URL of the official site, for the node list and the daily news.
    pub api_base: String,
    /// Proxy for those requests, e.g. `http://127.0.0.1:7897`. Empty means direct,
    /// or "use the system proxy" where the platform can tell us.
    pub proxy: String,
    /// Whether to honour the system's proxy settings when `proxy` is empty.
    pub use_system_proxy: bool,
    /// How long a fetched node list stays fresh, in seconds.
    pub node_cache_seconds: u64,
    /// Local game port the form starts with.
    pub default_game_port: u16,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            api_base: "https://hongshi.site".to_string(),
            proxy: String::new(),
            use_system_proxy: true,
            node_cache_seconds: 300,
            default_game_port: 25565,
        }
    }
}

impl Settings {
    /// Parse, filling in defaults for anything missing or unusable.
    pub fn from_json(text: &str) -> Settings {
        let mut settings = Settings::default();
        // Every field is optional: a settings file with one key is a settings
        // file, not a reason to fall back to the defaults wholesale.
        if let Some(api_base) = json_string(text, "api_base") {
            let normalized = normalize_base_url(&api_base);
            if !normalized.is_empty() {
                settings.api_base = normalized;
            }
        }
        if let Some(proxy) = json_string(text, "proxy") {
            settings.proxy = proxy;
        }
        if let Some(flag) = json_bool(text, "use_system_proxy") {
            settings.use_system_proxy = flag;
        }
        if let Some(seconds) = json_number(text, "node_cache_seconds") {
            settings.node_cache_seconds = seconds.clamp(30, 86_400);
        }
        if let Some(port) = json_number(text, "default_game_port") {
            if (1..=65535).contains(&port) {
                settings.default_game_port = port as u16;
            }
        }
        settings
    }

    pub fn to_json(&self) -> String {
        format!(
            "{{\n  \"api_base\": \"{}\",\n  \"proxy\": \"{}\",\n  \"use_system_proxy\": {},\n  \"node_cache_seconds\": {},\n  \"default_game_port\": {}\n}}\n",
            escape_json(&self.api_base),
            escape_json(&self.proxy),
            self.use_system_proxy,
            self.node_cache_seconds,
            self.default_game_port,
        )
    }

    /// Apply a partial update coming from the interface. Unknown keys are ignored
    /// and an absent key keeps its current value, so the page can send only what
    /// the user touched.
    pub fn apply_json(&mut self, text: &str) {
        if let Some(api_base) = json_string(text, "api_base") {
            let normalized = normalize_base_url(&api_base);
            if !normalized.is_empty() {
                self.api_base = normalized;
            }
        }
        if let Some(proxy) = json_string(text, "proxy") {
            self.proxy = proxy;
        }
        if let Some(flag) = json_bool(text, "use_system_proxy") {
            self.use_system_proxy = flag;
        }
        if let Some(seconds) = json_number(text, "node_cache_seconds") {
            self.node_cache_seconds = seconds.clamp(30, 86_400);
        }
        if let Some(port) = json_number(text, "default_game_port") {
            if (1..=65535).contains(&port) {
                self.default_game_port = port as u16;
            }
        }
    }
}

/// Strip trailing slashes so `base + "/api/…"` never doubles up.
pub fn normalize_base_url(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    // A base with no scheme would be interpreted as a path by an HTTP client.
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// The directory a host says the settings file belongs in.
///
/// "Next to the executable" is the right answer for a portable download and the
/// wrong one for an installed application: on Android the executable is
/// `/system/bin/app_process64`, whose parent is read-only, and the home directory
/// the fallback reaches for is not set either — so both candidates fail and the
/// user's settings silently never persist. A host that knows where its own data
/// goes says so here, once, at startup.
static APP_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Tell the shell where to keep its settings file. Called once, before startup.
pub fn install_app_dir(dir: PathBuf) -> Result<(), String> {
    APP_DIR
        .set(dir)
        .map_err(|_| "the application directory is already set".to_string())
}

/// The two directories this crate may write to, primary first.
///
/// Factored out because the session history ([`crate::sessions`]) writes a second
/// file with the same two-step policy, and the policy is the interesting part: the
/// primary is read-only on Android (`/system/bin`), so a host that knows where its
/// own data goes says so through [`install_app_dir`], and everything else falls back
/// to the home directory rather than failing to persist.
pub fn base_dirs() -> (PathBuf, PathBuf) {
    if let Some(dir) = APP_DIR.get() {
        // The host's directory is the writable one by construction, so the fallback
        // is only here to keep this function's shape: it is reported rather than
        // silently retried if the primary write ever fails.
        return (dir.clone(), std::env::temp_dir());
    }

    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    let home = home_dir().unwrap_or_else(std::env::temp_dir);
    (beside, home)
}

/// The user's home directory, if the environment names one.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Where the settings file lives, and where it would live if the first choice is
/// not writable.
pub fn settings_paths() -> (PathBuf, PathBuf) {
    let (primary, fallback) = base_dirs();
    (primary.join(SETTINGS_FILE), fallback.join(SETTINGS_FILE))
}

/// The file's name, in the one place both branches read it from.
pub const SETTINGS_FILE: &str = "hongshi.settings.json";

pub fn load() -> Settings {
    let (beside, home) = settings_paths();
    for path in [&beside, &home] {
        if let Ok(text) = std::fs::read_to_string(path) {
            let settings = Settings::from_json(&text);
            crate::log::debug_fields(
                "loaded settings",
                &[("path", &path.display().to_string())],
            );
            return settings;
        }
    }
    Settings::default()
}

/// Save, falling back to the home directory. Returns where it landed.
pub fn save(settings: &Settings) -> Result<PathBuf, String> {
    let (beside, home) = settings_paths();
    let text = settings.to_json();
    match std::fs::write(&beside, &text) {
        Ok(()) => Ok(beside),
        Err(beside_error) => match std::fs::write(&home, &text) {
            Ok(()) => Ok(home),
            Err(home_error) => Err(format!(
                "could not write settings to {} ({beside_error}) or {} ({home_error})",
                beside.display(),
                home.display()
            )),
        },
    }
}

/// The path [`save`] would try first, for display.
pub fn primary_path() -> PathBuf {
    settings_paths().0
}

// ---------------------------------------------------------------------------
// Minimal JSON field reading
// ---------------------------------------------------------------------------

/// Read a string value by key. Handles the escapes `to_json` writes; a settings
/// file is not a general JSON document and does not need a general parser.
fn json_string(text: &str, key: &str) -> Option<String> {
    let start = find_value(text, key)?;
    let rest = text[start..].strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                }
                other => out.push(other),
            },
            ch => out.push(ch),
        }
    }
    None
}

fn json_bool(text: &str, key: &str) -> Option<bool> {
    let start = find_value(text, key)?;
    let rest = &text[start..];
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn json_number(text: &str, key: &str) -> Option<u64> {
    let start = find_value(text, key)?;
    let rest = &text[start..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Offset of the value that follows `"key"` and a colon.
fn find_value(text: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    let mut search = 0usize;
    while let Some(offset) = text[search..].find(&needle) {
        let after_key = search + offset + needle.len();
        let mut rest = text[after_key..].trim_start();
        if let Some(stripped) = rest.strip_prefix(':') {
            rest = stripped.trim_start();
            return Some(text.len() - rest.len());
        }
        search = after_key;
    }
    None
}

fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_used_when_a_file_is_missing_or_junk() {
        assert_eq!(Settings::from_json(""), Settings::default());
        assert_eq!(Settings::from_json("not json at all"), Settings::default());
        assert_eq!(Settings::from_json("{ }"), Settings::default());
    }

    #[test]
    fn every_field_round_trips() {
        let settings = Settings {
            api_base: "https://example.test".to_string(),
            proxy: "http://127.0.0.1:7897".to_string(),
            use_system_proxy: false,
            node_cache_seconds: 900,
            default_game_port: 30000,
        };
        let text = settings.to_json();
        assert_eq!(Settings::from_json(&text), settings);
    }

    #[test]
    fn a_host_without_a_scheme_gets_https() {
        assert_eq!(normalize_base_url("hongshi.site"), "https://hongshi.site");
        assert_eq!(normalize_base_url("hongshi.site/"), "https://hongshi.site");
        assert_eq!(normalize_base_url("  hongshi.site//  "), "https://hongshi.site");
        assert_eq!(normalize_base_url("http://127.0.0.1:3000/"), "http://127.0.0.1:3000");
        assert_eq!(normalize_base_url("   "), "");
    }

    #[test]
    fn a_partial_update_keeps_everything_it_does_not_mention() {
        let mut settings = Settings::default();
        let before = settings.clone();
        settings.apply_json(r#"{"proxy":"http://127.0.0.1:1080"}"#);
        assert_eq!(settings.proxy, "http://127.0.0.1:1080");
        assert_eq!(settings.api_base, before.api_base);
        assert_eq!(settings.default_game_port, before.default_game_port);
    }

    #[test]
    fn a_single_key_file_is_still_a_settings_file() {
        // The parser used to bail out entirely when `api_base` was absent, which
        // turned a file holding one unrelated preference into "everything is
        // default" — including the value that file actually set.
        let settings = Settings::from_json(r#"{"proxy":"http://127.0.0.1:7897"}"#);
        assert_eq!(settings.proxy, "http://127.0.0.1:7897");
        assert_eq!(settings.api_base, Settings::default().api_base);
    }

    #[test]
    fn nonsense_values_are_clamped_or_ignored() {
        let settings = Settings::from_json(
            r#"{"node_cache_seconds": 1, "default_game_port": 99999}"#,
        );
        assert_eq!(settings.node_cache_seconds, 30, "clamped up to the minimum");
        assert_eq!(settings.default_game_port, 25565, "out of range keeps the default");

        let settings = Settings::from_json(r#"{"node_cache_seconds": 99999999}"#);
        assert_eq!(settings.node_cache_seconds, 86_400, "clamped down to a day");
    }

    #[test]
    fn a_base_url_that_normalizes_to_nothing_is_refused_by_an_update() {
        let mut settings = Settings::default();
        settings.apply_json(r#"{"api_base":"////"}"#);
        assert_eq!(settings.api_base, "https://hongshi.site");
    }

    #[test]
    fn escaped_values_survive_a_round_trip() {
        let mut settings = Settings::default();
        settings.proxy = r#"http://user:pa"ss\word@127.0.0.1:8080"#.to_string();
        let text = settings.to_json();
        assert_eq!(Settings::from_json(&text).proxy, settings.proxy);
    }
}
