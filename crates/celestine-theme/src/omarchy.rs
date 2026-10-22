use crate::parse::ThemeTokens;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// True when this machine looks like an Omarchy install.
pub fn is_omarchy() -> bool {
    Path::new("/usr/share/omarchy").is_dir() || std::env::var_os("OMARCHY_PATH").is_some()
}

fn parse_toml_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_matches('"');
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Omarchy stages the active theme under `~/.local/state/omarchy/current/theme`
/// and writes the slug to `theme.name` (see `omarchy-theme-set`).
fn omarchy_current_dir() -> Option<PathBuf> {
    let home = home_dir()?;
    let current = home.join(".local/state/omarchy/current");
    if current.is_dir() {
        return Some(current);
    }
    None
}

fn current_omarchy_theme_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OMARCHY_THEME_DIR") {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return Some(pb);
        }
    }

    if let Some(current) = omarchy_current_dir() {
        let theme = current.join("theme");
        if theme.is_dir() {
            return Some(theme);
        }
        // Older layouts / slug file at current-theme
        let name_file = current.join("theme.name");
        if let Ok(slug) = std::fs::read_to_string(&name_file) {
            if let Some(dir) = resolve_theme_slug(slug.trim()) {
                return Some(dir);
            }
        }
    }

    // Legacy / alternate pointers
    let home = home_dir()?;
    let candidates = [
        home.join(".config/omarchy/current/theme"),
        home.join(".local/state/omarchy/current-theme"),
    ];
    for c in candidates {
        if c.is_symlink() || c.is_dir() {
            if let Ok(canon) = std::fs::canonicalize(&c) {
                return Some(canon);
            }
            if c.is_dir() {
                return Some(c);
            }
        }
        if c.is_file() {
            if let Ok(slug) = std::fs::read_to_string(&c) {
                if let Some(dir) = resolve_theme_slug(slug.trim()) {
                    return Some(dir);
                }
            }
        }
    }

    let fallback = PathBuf::from("/usr/share/omarchy/themes/tokyo-night");
    if fallback.is_dir() {
        return Some(fallback);
    }
    None
}

fn resolve_theme_slug(slug: &str) -> Option<PathBuf> {
    if slug.is_empty() {
        return None;
    }
    let home = home_dir()?;
    let user = home.join(".config/omarchy/themes").join(slug);
    if user.is_dir() {
        return Some(user);
    }
    let stock = PathBuf::from("/usr/share/omarchy/themes").join(slug);
    if stock.is_dir() {
        return Some(stock);
    }
    None
}

/// Revision token for the active Omarchy theme (slug + colors.toml mtime).
/// Changes when `omarchy theme set` runs or colors are overwritten in place.
pub fn omarchy_revision() -> Option<String> {
    let current = omarchy_current_dir()?;
    let slug = std::fs::read_to_string(current.join("theme.name"))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let colors = current.join("theme").join("colors.toml");
    let mtime = std::fs::metadata(&colors)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let nanos = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(format!("{slug}:{nanos}"))
}

/// Load Omarchy `colors.toml` into theme tokens.
pub fn load_omarchy_theme() -> Option<ThemeTokens> {
    let dir = current_omarchy_theme_dir()?;
    let colors = dir.join("colors.toml");
    let text = std::fs::read_to_string(colors).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    let table = value.as_table()?;

    let get = |k: &str| -> Option<[u8; 3]> {
        table
            .get(k)
            .and_then(|v| v.as_str())
            .and_then(parse_toml_hex)
    };

    let accent = get("accent").unwrap_or([0x7a, 0xa2, 0xf7]);
    // Prefer Omarchy's named chromatics so folder rainbows match the desktop
    // (including narrow Aether ramps that are intentionally not a full spectrum).
    let pick = |key: &str, fallback: [u8; 3]| get(key).unwrap_or(fallback);
    let folder_ramp = [
        pick("red", accent),
        pick("orange", accent),
        pick("yellow", accent),
        pick("green", accent),
        pick("cyan", accent),
        pick("blue", accent),
        pick("magenta", accent),
        pick("brown", get("muted").unwrap_or(accent)),
        pick("bright_red", pick("red", accent)),
        pick("bright_blue", pick("blue", accent)),
    ];

    Some(ThemeTokens::from_omarchy_colors(
        get("background").unwrap_or([0x1a, 0x1b, 0x26]),
        get("lighter_background").unwrap_or([0x24, 0x28, 0x3b]),
        get("foreground").unwrap_or([0xa9, 0xb1, 0xd6]),
        get("dark_foreground").unwrap_or([0x56, 0x5f, 0x89]),
        accent,
        get("selection").unwrap_or([0x29, 0x2e, 0x42]),
        get("muted").unwrap_or([0x41, 0x48, 0x68]),
        folder_ramp,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise env mutations across tests in this module.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_hex_ok() {
        assert_eq!(parse_toml_hex("#f21650"), Some([0xf2, 0x16, 0x50]));
        assert_eq!(parse_toml_hex("\"#daccc1\""), Some([0xda, 0xcc, 0xc1]));
    }

    #[test]
    fn loads_from_omarchy_theme_dir_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "celestine-omarchy-theme-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("colors.toml"),
            r##"
background = "#010203"
lighter_background = "#040506"
foreground = "#aabbcc"
dark_foreground = "#112233"
accent = "#ff00aa"
selection = "#334455"
muted = "#667788"
red = "#111111"
orange = "#222222"
yellow = "#333333"
green = "#444444"
cyan = "#555555"
blue = "#666666"
magenta = "#777777"
brown = "#888888"
bright_red = "#999999"
bright_blue = "#aaaaaa"
"##,
        )
        .unwrap();
        std::env::set_var("OMARCHY_THEME_DIR", &root);
        let t = load_omarchy_theme().expect("load from OMARCHY_THEME_DIR");
        std::env::remove_var("OMARCHY_THEME_DIR");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(t.name, "omarchy");
        assert_eq!(t.background_primary, [0x01, 0x02, 0x03]);
        assert_eq!(t.text_accent, [0xff, 0x00, 0xaa]);
        let ramp = t.folder_ramp.expect("omarchy folder ramp");
        assert_eq!(ramp[0], [0x11, 0x11, 0x11]);
        assert_eq!(ramp[5], [0x66, 0x66, 0x66]);
        assert_eq!(ramp[9], [0xaa, 0xaa, 0xaa]);
    }
}
