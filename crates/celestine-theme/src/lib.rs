//! CSS custom-property theme packs for CelestineMD.

mod omarchy;
mod parse;

pub use omarchy::load_omarchy_terminal;
pub use parse::{parse_css_vars, ThemeTokens};

use std::path::{Path, PathBuf};

pub const BUILTIN_THEME_NAMES: &[&str] = &[
    "obsidian-dark",
    "obsidian-light",
    "catppuccin",
    "tokyo-night",
    "gruvbox-dark",
    "gruvbox-light",
    "nord",
    "solarized-dark",
    "solarized-light",
    "dracula",
    "rose-pine",
    "everforest-dark",
    "neovoid",
    "one-dark",
    "monokai",
    "kanagawa",
    "ayu-dark",
    "oxocarbon",
    "high-contrast",
    "terminal",
];

/// Builtin CSS embedded at compile time so themes work even when the install
/// layout doesn't put `themes/` on disk (notably Homebrew's `share/celestine-md`).
const BUILTIN_THEME_CSS: &[(&str, &str)] = &[
    ("obsidian-dark", include_str!("../../../themes/obsidian-dark.css")),
    ("obsidian-light", include_str!("../../../themes/obsidian-light.css")),
    ("catppuccin", include_str!("../../../themes/catppuccin.css")),
    ("tokyo-night", include_str!("../../../themes/tokyo-night.css")),
    ("gruvbox-dark", include_str!("../../../themes/gruvbox-dark.css")),
    ("gruvbox-light", include_str!("../../../themes/gruvbox-light.css")),
    ("nord", include_str!("../../../themes/nord.css")),
    ("solarized-dark", include_str!("../../../themes/solarized-dark.css")),
    ("solarized-light", include_str!("../../../themes/solarized-light.css")),
    ("dracula", include_str!("../../../themes/dracula.css")),
    ("rose-pine", include_str!("../../../themes/rose-pine.css")),
    ("everforest-dark", include_str!("../../../themes/everforest-dark.css")),
    ("neovoid", include_str!("../../../themes/neovoid.css")),
    ("one-dark", include_str!("../../../themes/one-dark.css")),
    ("monokai", include_str!("../../../themes/monokai.css")),
    ("kanagawa", include_str!("../../../themes/kanagawa.css")),
    ("ayu-dark", include_str!("../../../themes/ayu-dark.css")),
    ("oxocarbon", include_str!("../../../themes/oxocarbon.css")),
    ("high-contrast", include_str!("../../../themes/high-contrast.css")),
];

fn builtin_theme_css(name: &str) -> Option<&'static str> {
    BUILTIN_THEME_CSS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, css)| *css)
}

fn dir_has_css(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .map(|rd| {
            rd.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("css"))
            })
        })
        .unwrap_or(false)
}

pub fn themes_dir_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(proj) = directories::ProjectDirs::from("md", "Celestine", "celestine") {
        dirs.push(proj.data_dir().join("themes"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/share/celestine/themes"));
        // Homebrew / macOS user data conventions
        dirs.push(home.join("Library/Application Support/celestine/themes"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.join("themes"));
            // Release tarball + Linux FHS: …/bin → …/share/celestine/themes
            dirs.push(parent.join("../share/celestine/themes"));
            // Homebrew formula name is celestine-md → pkgshare = share/celestine-md
            dirs.push(parent.join("../share/celestine-md/themes"));
            dirs.push(parent.join("../themes"));
            dirs.push(parent.join("../../themes"));
        }
    }
    // Common Homebrew prefixes when the binary is launched via a PATH shim
    for prefix in ["/opt/homebrew", "/usr/local"] {
        dirs.push(PathBuf::from(prefix).join("share/celestine-md/themes"));
        dirs.push(PathBuf::from(prefix).join("share/celestine/themes"));
    }
    dirs.push(PathBuf::from("themes"));
    dirs.push(PathBuf::from("../themes"));
    dirs.push(PathBuf::from("../../themes"));
    // CARGO_MANIFEST_DIR when running from source
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../themes"));
    dirs
}

pub fn find_themes_dir() -> Option<PathBuf> {
    // Prefer a directory that actually contains CSS (skip empty stubs).
    themes_dir_candidates()
        .into_iter()
        .find(|d| d.is_dir() && dir_has_css(d))
        .or_else(|| themes_dir_candidates().into_iter().find(|d| d.is_dir()))
}

pub fn load_theme(name: &str) -> Result<ThemeTokens, String> {
    if name == "terminal" {
        if let Some(t) = load_omarchy_terminal() {
            return Ok(t);
        }
        return Ok(ThemeTokens::terminal_fallback());
    }
    // Prefer on-disk packs (user overrides / packaged installs).
    if let Some(dir) = find_themes_dir() {
        let path = dir.join(format!("{name}.css"));
        if path.is_file() {
            let mut t = load_theme_file(&path)?;
            t.name = name.to_string();
            return Ok(t);
        }
    }
    // Embedded builtins — keeps Mac Homebrew / bare exe installs working.
    if let Some(css) = builtin_theme_css(name) {
        let mut t = parse_css_vars(css);
        t.name = name.to_string();
        return Ok(t);
    }
    Err(format!("theme not found: {name}"))
}

pub fn load_theme_file(path: &Path) -> Result<ThemeTokens, String> {
    let css = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut t = parse_css_vars(&css);
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        t.name = stem.to_string();
    }
    Ok(t)
}

pub fn list_available_themes() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_THEME_NAMES.iter().map(|s| (*s).to_string()).collect();
    if let Some(dir) = find_themes_dir() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("css") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        if !names.iter().any(|n| n == stem) {
                            names.push(stem.to_string());
                        }
                    }
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_builtins_parse() {
        for name in BUILTIN_THEME_NAMES {
            if *name == "terminal" {
                continue;
            }
            let t = load_theme(name).expect(name);
            assert_eq!(t.name, *name);
            // Sanity: not all-zero (would mean parse failed silently)
            assert!(
                t.background_primary != [0, 0, 0] || t.text_normal != [0, 0, 0],
                "{name} parsed to empty colors"
            );
        }
    }

    #[test]
    fn homebrew_share_path_is_candidate() {
        let c = themes_dir_candidates();
        assert!(
            c.iter().any(|p| p.ends_with("share/celestine-md/themes")),
            "missing Homebrew pkgshare candidate: {c:?}"
        );
    }

    #[test]
    fn linux_fhs_candidate_still_present() {
        let c = themes_dir_candidates();
        assert!(
            c.iter().any(|p| p.ends_with("share/celestine/themes")),
            "missing Linux FHS candidate: {c:?}"
        );
        assert!(
            c.iter().any(|p| p.ends_with(".local/share/celestine/themes")),
            "missing ~/.local/share candidate: {c:?}"
        );
    }

    #[test]
    fn disk_theme_preferred_over_embedded() {
        let dir = tempfile_themes_dir();
        let path = dir.join("catppuccin.css");
        // Distinct accent so we know we loaded disk, not embedded
        std::fs::write(
            &path,
            r#":root { --background-primary: #010203; --text-accent: #aabbcc; }"#,
        )
        .unwrap();
        // Point discovery at our temp dir by using CWD relative "themes"
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.parent().unwrap()).unwrap();
        let t = load_theme("catppuccin").expect("load");
        std::env::set_current_dir(prev).unwrap();
        assert_eq!(t.background_primary, [0x01, 0x02, 0x03], "should load on-disk override");
        assert_eq!(t.text_accent, [0xaa, 0xbb, 0xcc]);
    }

    fn tempfile_themes_dir() -> PathBuf {
        let root = std::env::temp_dir().join(format!("celestine-theme-test-{}", std::process::id()));
        let themes = root.join("themes");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&themes).unwrap();
        themes
    }
}
