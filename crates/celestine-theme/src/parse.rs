use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct ThemeTokens {
    pub name: String,
    pub background_primary: [u8; 3],
    pub background_secondary: [u8; 3],
    pub background_modifier_border: [u8; 3],
    pub text_normal: [u8; 3],
    pub text_muted: [u8; 3],
    pub text_accent: [u8; 3],
    pub interactive_accent: [u8; 3],
    pub selection: [u8; 3],
    /// When set (Omarchy bridge), folder rainbows use these colors instead of a
    /// synthetic HSV walk around the accent.
    pub folder_ramp: Option<[[u8; 3]; 10]>,
}

impl Default for ThemeTokens {
    fn default() -> Self {
        Self::obsidian_dark()
    }
}

impl ThemeTokens {
    pub fn obsidian_dark() -> Self {
        Self {
            name: "obsidian-dark".into(),
            background_primary: [0x1e, 0x1e, 0x1e],
            background_secondary: [0x25, 0x25, 0x25],
            background_modifier_border: [0x33, 0x33, 0x33],
            text_normal: [0xdc, 0xdd, 0xde],
            text_muted: [0x99, 0x99, 0x99],
            text_accent: [0x7f, 0x6d, 0xf2],
            interactive_accent: [0x7f, 0x6d, 0xf2],
            selection: [0x3a, 0x3a, 0x4a],
            folder_ramp: None,
        }
    }

    pub fn omarchy_fallback() -> Self {
        Self {
            name: "omarchy".into(),
            background_primary: [0x0c, 0x0c, 0x0c],
            background_secondary: [0x14, 0x14, 0x14],
            background_modifier_border: [0x33, 0x33, 0x33],
            text_normal: [0xd0, 0xd0, 0xd0],
            text_muted: [0x80, 0x80, 0x80],
            text_accent: [0x7a, 0xa2, 0xf7],
            interactive_accent: [0x7a, 0xa2, 0xf7],
            selection: [0x29, 0x2e, 0x42],
            folder_ramp: Some([
                [0xf7, 0x76, 0x8e], // red
                [0xeb, 0x92, 0x7b], // orange
                [0xe0, 0xaf, 0x68], // yellow
                [0x9e, 0xce, 0x6a], // green
                [0x44, 0x9d, 0xab], // cyan
                [0x7a, 0xa2, 0xf7], // blue
                [0xad, 0x8e, 0xe6], // magenta
                [0x75, 0x49, 0x3d], // brown
                [0xff, 0x7a, 0x93], // bright_red
                [0x7d, 0xa6, 0xff], // bright_blue
            ]),
        }
    }

    pub fn from_omarchy_colors(
        background: [u8; 3],
        lighter_background: [u8; 3],
        foreground: [u8; 3],
        dark_foreground: [u8; 3],
        accent: [u8; 3],
        selection: [u8; 3],
        muted: [u8; 3],
        folder_ramp: [[u8; 3]; 10],
    ) -> Self {
        Self {
            name: "omarchy".into(),
            background_primary: background,
            background_secondary: lighter_background,
            background_modifier_border: muted,
            text_normal: foreground,
            text_muted: dark_foreground,
            text_accent: accent,
            interactive_accent: accent,
            selection,
            folder_ramp: Some(folder_ramp),
        }
    }
}

fn var_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"--([a-zA-Z0-9-]+)\s*:\s*([^;]+);").expect("css var re")
    })
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some([r, g, b]);
        }
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            return Some([r, g, b]);
        }
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|x| x.strip_suffix(')')) {
        let parts: Vec<_> = inner.split(',').map(|p| p.trim().parse::<u8>().ok()).collect();
        if parts.len() >= 3 {
            return Some([parts[0]?, parts[1]?, parts[2]?]);
        }
    }
    None
}

pub fn parse_css_vars(css: &str) -> ThemeTokens {
    let mut t = ThemeTokens::obsidian_dark();
    for cap in var_re().captures_iter(css) {
        let name = cap.get(1).unwrap().as_str();
        let val = cap.get(2).unwrap().as_str();
        let Some(rgb) = parse_color(val) else {
            continue;
        };
        match name {
            "background-primary" => t.background_primary = rgb,
            "background-secondary" => t.background_secondary = rgb,
            "background-modifier-border" => t.background_modifier_border = rgb,
            "text-normal" => t.text_normal = rgb,
            "text-muted" => t.text_muted = rgb,
            "text-accent" => t.text_accent = rgb,
            "interactive-accent" => t.interactive_accent = rgb,
            "selection" | "text-selection" => t.selection = rgb,
            _ => {}
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_vars() {
        let css = r#"
:root {
  --background-primary: #1a1b26;
  --text-accent: #7aa2f7;
}
"#;
        let t = parse_css_vars(css);
        assert_eq!(t.background_primary, [0x1a, 0x1b, 0x26]);
        assert_eq!(t.text_accent, [0x7a, 0xa2, 0xf7]);
    }
}
