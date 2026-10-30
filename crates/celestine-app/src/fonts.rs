use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// Use the same bundled text faces on every platform. System fonts remain
/// available for color emoji and scripts not covered by JetBrains Mono.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "celestine_mono".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/JetBrainsMono-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "celestine_mono_bold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../../../assets/fonts/JetBrainsMono-Bold.ttf"
        ))),
    );
    fonts.families.insert(
        FontFamily::Name("celestine_mono_bold".into()),
        vec!["celestine_mono_bold".into()],
    );

    // Drop stock monochrome emoji faces so the system color-emoji provider
    // owns those codepoints (otherwise OpenMoji/Noto outline wins and looks flat).
    let emoji_keys: Vec<String> = fonts
        .font_data
        .keys()
        .filter(|k| {
            let l = k.to_ascii_lowercase();
            l.contains("emoji") || l.contains("openmoji")
        })
        .cloned()
        .collect();
    for key in emoji_keys {
        fonts.font_data.remove(&key);
    }
    strip_emoji_faces_from_families(&mut fonts);

    prepend_mono(&mut fonts, FontFamily::Proportional);
    prepend_mono(&mut fonts, FontFamily::Monospace);

    fonts
}

fn strip_emoji_faces_from_families(fonts: &mut FontDefinitions) {
    for family in fonts.families.values_mut() {
        family.retain(|name| {
            let l = name.to_ascii_lowercase();
            !l.contains("emoji") && !l.contains("openmoji")
        });
    }
}

fn prepend_mono(fonts: &mut FontDefinitions, family: FontFamily) {
    let Some(existing) = fonts.families.get(&family).cloned() else {
        if fonts.font_data.contains_key("celestine_mono") {
            fonts.families.insert(family, vec!["celestine_mono".into()]);
        }
        return;
    };
    let mut ordered = Vec::new();
    if fonts.font_data.contains_key("celestine_mono") {
        ordered.push("celestine_mono".into());
    }
    for name in existing {
        if !ordered.iter().any(|n| n == &name) {
            ordered.push(name);
        }
    }
    fonts.families.insert(family, ordered);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_faces_lead_both_text_families_without_system_font_discovery() {
        let fonts = definitions();
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            assert_eq!(fonts.families[&family][0], "celestine_mono");
        }
        assert_eq!(
            fonts.families[&FontFamily::Name("celestine_mono_bold".into())],
            vec!["celestine_mono_bold"]
        );
    }
}
