use celestine_theme::ThemeTokens;
use egui::style::WidgetVisuals;
use egui::{
    Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Shadow, Stroke, Style, TextStyle,
    Visuals,
};

fn margin_xy(x: f32, y: f32) -> Margin {
    Margin::symmetric(x.round().clamp(0.0, 127.0) as i8, y.round().clamp(0.0, 127.0) as i8)
}

fn shadow_drop(offset_y: i8, blur: u8, spread: u8, color: Color32) -> Shadow {
    Shadow {
        offset: [0, offset_y],
        blur,
        spread,
        color,
    }
}

pub fn color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Color32::from_rgba_unmultiplied(
        (a.r() as f32 * inv + b.r() as f32 * t) as u8,
        (a.g() as f32 * inv + b.g() as f32 * t) as u8,
        (a.b() as f32 * inv + b.b() as f32 * t) as u8,
        (a.a() as f32 * inv + b.a() as f32 * t) as u8,
    )
}

pub fn is_dark(t: &ThemeTokens) -> bool {
    luminance(t.background_primary) < 0.5
}

/// Derived colors that make each theme feel distinct (folders, headings, chrome tint).
#[derive(Clone)]
pub struct ThemePalette {
    pub name: String,
    pub dark: bool,
    pub accent: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub content_bg: Color32,
    pub panel_bg: Color32,
    pub folder: [Color32; 10],
    pub heading: [Color32; 6],
}

impl ThemePalette {
    pub fn from_tokens(t: &ThemeTokens) -> Self {
        let dark = is_dark(t);
        let accent = color(t.interactive_accent);
        let text_accent = color(t.text_accent);
        let text = color(t.text_normal);
        let muted = color(t.text_muted);
        let border = color(t.background_modifier_border);
        let content = content_bg(t);
        let panel = panel_bg(t);
        let seed = theme_hue_seed(&t.name);
        let (ah, asat, aval) = rgb_to_hsv(accent);
        // Seed dominates so packs with similar accents still diverge.
        let base_h = if asat < 0.18 {
            seed
        } else {
            ah * 0.28 + seed * 0.72
        };

        let folder = if let Some(ramp) = t.folder_ramp {
            // Omarchy (and similar): use the theme's own chromatics verbatim.
            std::array::from_fn(|i| color(ramp[i]))
        } else {
            std::array::from_fn(|i| {
                let h = (base_h + i as f32 * 36.0) % 360.0;
                let (s, v) = if dark {
                    (0.62_f32.max(asat * 0.9).min(0.92), 0.82_f32.max(aval).min(0.98))
                } else {
                    (0.68_f32.max(asat).min(0.94), 0.52_f32.min(aval.max(0.40)).max(0.36))
                };
                // Alternate saturation slightly so neighbors don't blend.
                let s = (s + if i % 2 == 0 { 0.08 } else { -0.05 }).clamp(0.45, 0.95);
                hsv_to_rgb(h, s, v)
            })
        };

        // Headings: H1 = accent, rest walk the theme's hue personality —
        // or step through Omarchy chromatics when a ramp is present.
        let (tah, tsat, _) = rgb_to_hsv(text_accent);
        let h1 = accent;
        let h2 = if tsat > 0.2 {
            text_accent
        } else if let Some(ramp) = t.folder_ramp {
            color(ramp[1])
        } else {
            hsv_to_rgb(
                (base_h + 32.0) % 360.0,
                if dark { 0.58 } else { 0.65 },
                if dark { 0.90 } else { 0.46 },
            )
        };
        let (h3, h4, h5) = if let Some(ramp) = t.folder_ramp {
            (color(ramp[2]), color(ramp[3]), color(ramp[5]))
        } else {
            (
                hsv_to_rgb(
                    (tah * 0.35 + base_h * 0.65 + 64.0) % 360.0,
                    if dark { 0.54 } else { 0.60 },
                    if dark { 0.92 } else { 0.48 },
                ),
                hsv_to_rgb(
                    (base_h + 118.0) % 360.0,
                    if dark { 0.56 } else { 0.64 },
                    if dark { 0.84 } else { 0.44 },
                ),
                hsv_to_rgb(
                    (base_h + 168.0) % 360.0,
                    if dark { 0.50 } else { 0.58 },
                    if dark { 0.86 } else { 0.46 },
                ),
            )
        };
        let h6 = mix(text, muted, 0.28);

        Self {
            name: t.name.clone(),
            dark,
            accent,
            text,
            muted,
            border,
            content_bg: content,
            panel_bg: panel,
            folder,
            heading: [h1, h2, h3, h4, h5, h6],
        }
    }

    pub fn folder_at(&self, sibling_index: usize) -> Color32 {
        self.folder[sibling_index % self.folder.len()]
    }

    pub fn heading_at(&self, level: u8) -> Color32 {
        let i = level.saturating_sub(1).min(5) as usize;
        self.heading[i]
    }
}

/// Canonical hue personality per pack so accents alone don't make themes collide.
fn theme_hue_seed(name: &str) -> f32 {
    match name {
        "obsidian-dark" | "obsidian-light" => 265.0,
        // Mauve vs cyan-blue — keep these packs from collapsing into the same night UI.
        "catppuccin" => 280.0,
        "tokyo-night" => 205.0,
        "gruvbox-dark" | "gruvbox-light" => 35.0,
        "nord" => 200.0,
        "solarized-dark" | "solarized-light" => 175.0,
        "dracula" => 310.0,
        "rose-pine" => 290.0,
        "everforest-dark" => 145.0,
        "neovoid" => 195.0,
        "one-dark" => 250.0,
        "monokai" => 80.0,
        "kanagawa" => 220.0,
        "ayu-dark" => 35.0,
        "oxocarbon" => 330.0,
        "high-contrast" => 50.0,
        "omarchy" | "terminal" => 215.0,
        other => {
            let mut h = 0u32;
            for b in other.bytes() {
                h = h.wrapping_mul(31).wrapping_add(b as u32);
            }
            (h % 360) as f32
        }
    }
}

/// Sidebars / chrome — keep secondary, with a light accent wash for identity.
pub fn panel_bg(t: &ThemeTokens) -> Color32 {
    let base = color(t.background_secondary);
    let accent = color(t.interactive_accent);
    mix(base, accent, if is_dark(t) { 0.09 } else { 0.11 })
}

/// Editor surface — preserve theme primary (do not crush every pack to the same charcoal).
pub fn content_bg(t: &ThemeTokens) -> Color32 {
    let primary = color(t.background_primary);
    let accent = color(t.interactive_accent);
    if is_dark(t) {
        mix(mix(primary, Color32::BLACK, 0.08), accent, 0.07)
    } else {
        mix(mix(primary, Color32::WHITE, 0.06), accent, 0.06)
    }
}

/// Apply flea-like flat chrome: naked buttons, hairline rules, JetBrains sizes.
pub fn apply_theme(ctx: &egui::Context, t: &ThemeTokens, ui_font_scale: f32) {
    let dark = is_dark(t);
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    let content = content_bg(t);
    let panel = panel_bg(t);
    let text = color(t.text_normal);
    let muted = color(t.text_muted);
    let accent = color(t.interactive_accent);
    let border = color(t.background_modifier_border);
    let sel = color(t.selection);

    visuals.window_fill = content;
    visuals.panel_fill = panel;
    visuals.extreme_bg_color = content;
    visuals.faint_bg_color = mix(panel, content, 0.5);
    visuals.code_bg_color = mix(content, panel, 0.35);
    visuals.override_text_color = Some(text);
    visuals.selection.bg_fill = sel;
    visuals.selection.stroke = Stroke::new(1.0_f32, accent);
    visuals.hyperlink_color = accent;
    visuals.window_stroke = Stroke::NONE;
    visuals.window_corner_radius = CornerRadius::ZERO;
    visuals.menu_corner_radius = CornerRadius::same(2);
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = shadow_drop(4, 12, 0, Color32::from_black_alpha(if dark { 80 } else { 30 }));

    let naked = |fg: Color32| WidgetVisuals {
        bg_fill: Color32::TRANSPARENT,
        weak_bg_fill: Color32::TRANSPARENT,
        bg_stroke: Stroke::NONE,
        corner_radius: CornerRadius::ZERO,
        fg_stroke: Stroke::new(1.0_f32, fg),
        expansion: 0.0,
    };

    visuals.widgets.noninteractive = WidgetVisuals {
        bg_fill: panel,
        weak_bg_fill: Color32::TRANSPARENT,
        bg_stroke: Stroke::new(1.0_f32, border.gamma_multiply(0.6)),
        corner_radius: CornerRadius::ZERO,
        fg_stroke: Stroke::new(1.0_f32, muted),
        expansion: 0.0,
    };
    visuals.widgets.inactive = naked(text);
    visuals.widgets.hovered = naked(accent);
    visuals.widgets.active = naked(accent);
    visuals.widgets.open = WidgetVisuals {
        bg_fill: sel,
        weak_bg_fill: sel,
        bg_stroke: Stroke::NONE,
        corner_radius: CornerRadius::ZERO,
        fg_stroke: Stroke::new(1.0_f32, accent),
        expansion: 0.0,
    };

    let s = ui_font_scale.clamp(0.85, 1.6);
    let theme = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(theme);
    let mut style = Style {
        visuals,
        ..(*ctx.style_of(theme)).clone()
    };
    style.spacing.item_spacing = egui::vec2(6.0 * s, 3.0 * s);
    style.spacing.window_margin = margin_xy(8.0 * s, 6.0 * s);
    style.spacing.menu_margin = margin_xy(8.0 * s, 6.0 * s);
    style.spacing.button_padding = egui::vec2(4.0 * s, 2.0 * s);
    style.spacing.indent = 14.0 * s;
    style.spacing.scroll = egui::style::ScrollStyle::solid();

    let body = 13.0 * s;
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(body * 0.85, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(body, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(body, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(body * 1.15, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(body, FontFamily::Monospace),
    );

    ctx.set_style_of(theme, style);
}

pub fn overlay_frame(t: &ThemeTokens) -> Frame {
    let panel = panel_bg(t);
    let fill = if is_dark(t) {
        mix(panel, Color32::WHITE, 0.08)
    } else {
        mix(panel, Color32::WHITE, 0.45)
    };
    let border = color(t.background_modifier_border);
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, border.gamma_multiply(0.85)))
        .corner_radius(CornerRadius::same(2))
        .inner_margin(margin_xy(14.0, 10.0))
        .shadow(shadow_drop(
            8,
            24,
            0,
            Color32::from_black_alpha(if is_dark(t) { 160 } else { 50 }),
        ))
}

pub fn panel_frame(ctx: &egui::Context) -> Frame {
    Frame::side_top_panel(ctx.style_of(ctx.theme()).as_ref())
        .inner_margin(margin_xy(10.0, 6.0))
        .stroke(Stroke::NONE)
}

pub fn content_frame(t: &ThemeTokens) -> Frame {
    Frame::new()
        .fill(content_bg(t))
        .inner_margin(margin_xy(12.0, 8.0))
}

pub fn naked_button(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>) -> egui::Response {
    ui.add(egui::Button::new(label).frame(false))
}

pub fn naked_icon(
    ui: &mut egui::Ui,
    name: &str,
    active: bool,
    accent: Color32,
    muted: Color32,
) -> egui::Response {
    let color = if active { accent } else { muted };
    crate::icons::button(ui, name, 16.0, color)
}

pub fn sidebar_toggle(
    ui: &mut egui::Ui,
    label: &str,
    active: bool,
    accent: Color32,
    muted: Color32,
) -> egui::Response {
    let color = if active { accent } else { muted };
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .monospace()
                .size(12.5)
                .color(color),
        )
        .frame(false),
    )
}

fn luminance(rgb: [u8; 3]) -> f32 {
    luma(color(rgb))
}

fn luma(c: Color32) -> f32 {
    0.2126 * (c.r() as f32 / 255.0)
        + 0.7152 * (c.g() as f32 / 255.0)
        + 0.0722 * (c.b() as f32 / 255.0)
}

pub fn rgb_to_hsv(c: Color32) -> (f32, f32, f32) {
    let r = c.r() as f32 / 255.0;
    let g = c.g() as f32 / 255.0;
    let b = c.b() as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max <= 1e-6 { 0.0 } else { d / max };
    let h = if d <= 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / d) % 6.0)
    } else if (max - g).abs() < 1e-6 {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    (h, s, v)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color32 {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    Color32::from_rgb(
        ((r + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}
