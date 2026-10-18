use crate::media::{self, is_image_target};
use crate::theme_apply::mix;
use celestine_core::{has_uri_scheme, resolve_link, ResolvedLink, VaultIndex};
use egui::{Color32, FontId, Frame, Margin, RichText, Sense, Stroke, TextStyle, Ui, Vec2};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

pub struct PreviewOutcome {
    /// A link the user clicked this frame.
    pub link: Option<LinkTarget>,
    /// Heading text that was rendered this frame (for scroll targets).
    pub heading_rects: Vec<(usize, egui::Rect)>, // line index approx via counter
    /// True when a find/search scroll target was scrolled into view this frame.
    pub find_scrolled: bool,
}

/// Where a clicked preview link leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A vault note (`rel` ends in `.md`; it may not exist yet) and an
    /// optional heading to scroll to.
    Note { rel: String, heading: Option<String> },
    /// An image under the vault.
    Image(String),
    /// A heading in the note being previewed (`[[#H]]`, `[t](#anchor)`).
    Heading(String),
    /// Anything with a URI scheme: open with the system handler.
    External(String),
}

/// How a jump target is compared against the note's headings.
///
/// When some heading equals the target verbatim only that text matches, so
/// `# A B` and `# A-B` stay distinct. Otherwise GitHub-style slugs are
/// compared, which makes `[t](#my-heading)` reach `# My Heading`.
#[derive(Debug, Clone)]
pub enum HeadingMatch {
    Exact(String),
    Slug(String),
}

impl HeadingMatch {
    pub fn for_target(body: &str, want: &str) -> Self {
        let want = want.trim();
        let exact = body.lines().any(|line| {
            let t = line.trim_start();
            let hashes = t.bytes().take_while(|b| *b == b'#').count();
            (1..=6).contains(&hashes)
                && t[hashes..].starts_with([' ', '\t'])
                && t[hashes..].trim() == want
        });
        if exact {
            Self::Exact(want.to_string())
        } else {
            Self::Slug(slug(want))
        }
    }

    pub fn matches(&self, plain: &str) -> bool {
        match self {
            Self::Exact(want) => plain.trim() == want,
            Self::Slug(want) => slug(plain) == *want,
        }
    }
}

/// Does rendered heading text `plain` satisfy `want` by exact text or slug?
pub fn heading_matches(plain: &str, want: &str) -> bool {
    let plain = plain.trim();
    let want = want.trim();
    plain == want || slug(plain) == slug(want)
}

fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn apply_find_focus(
    ui: &Ui,
    rect: egui::Rect,
    plain: &str,
    scroll_needle: &Option<String>,
    flash_needle: &Option<String>,
    flash_alpha: f32,
    accent: Color32,
    find_scrolled: &mut bool,
) {
    let p = plain.to_lowercase();
    let flash_hit = flash_needle
        .as_ref()
        .is_some_and(|n| p.contains(n.as_str()));
    let scroll_hit = scroll_needle
        .as_ref()
        .is_some_and(|n| p.contains(n.as_str()));
    if flash_hit && flash_alpha > 0.01 {
        let a = (flash_alpha * 55.0).clamp(0.0, 255.0) as u8;
        ui.painter().rect_filled(
            rect.expand(3.0),
            4.0,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), a),
        );
    }
    if scroll_hit && !*find_scrolled {
        ui.scroll_to_rect(rect, Some(egui::Align::Center));
        *find_scrolled = true;
    }
}

struct HeadingColors {
    h1: Color32,
    h2: Color32,
    h3: Color32,
    h4: Color32,
    h5: Color32,
    h6: Color32,
}

impl HeadingColors {
    fn for_level(&self, level: u8) -> Color32 {
        match level {
            1 => self.h1,
            2 => self.h2,
            3 => self.h3,
            4 => self.h4,
            5 => self.h5,
            _ => self.h6,
        }
    }

    fn from_theme(headings: &[Color32; 6]) -> Self {
        Self {
            h1: headings[0],
            h2: headings[1],
            h3: headings[2],
            h4: headings[3],
            h5: headings[4],
            h6: headings[5],
        }
    }
}

fn wikilink_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"!?\[\[([^\[\]]+)\]\]").expect("wikilink"))
}

/// Preprocess Obsidian-ish body: wikilinks + `:shortcode:` emoji expansion.
fn preprocess(body: &str) -> String {
    let body = crate::emoji_util::expand_shortcodes(body);
    let mut out = String::with_capacity(body.len() + 32);
    let mut last = 0;
    for cap in wikilink_re().captures_iter(&body) {
        let m = cap.get(0).unwrap();
        out.push_str(&body[last..m.start()]);
        let inner = cap.get(1).unwrap().as_str();
        let embed = m.as_str().starts_with('!');
        let (target, display) = match inner.split_once('|') {
            Some((t, d)) => (t.trim(), d.trim()),
            None => (inner.trim(), inner.trim()),
        };
        let file = target.split('#').next().unwrap_or(target);
        if embed {
            if is_image_target(file) {
                // Render as a real markdown image so the preview can show pixels.
                out.push_str(&format!("![{display}](<{file}>)"));
            } else {
                out.push_str(&format!("*[embed: {display}]* (`wikilink:{file}`)"));
            }
        } else {
            // Keep `#Heading` so the click can scroll; `<…>` lets the
            // destination contain spaces.
            out.push_str(&format!("[{display}](<wikilink:{target}>)"));
        }
        last = m.end();
    }
    out.push_str(&body[last..]);
    out
}

/// Rich Markdown preview (tables, emphasis, headings by level, code, lists, blockquotes, tags).
pub fn show_preview(
    ui: &mut Ui,
    body: &str,
    note_rel: &str,
    vault_root: &Path,
    index: &VaultIndex,
    image_rels: &[String],
    accent: Color32,
    muted: Color32,
    text: Color32,
    font_size: f32,
    scroll_to_heading: Option<&str>,
    // One-shot: scroll so a block containing this text is centered.
    scroll_to_text: Option<&str>,
    // While non-empty and `flash_alpha` > 0, wash matching blocks.
    flash_text: Option<&str>,
    flash_alpha: f32,
    heading_palette: &[Color32; 6],
    image_resolver: &mut media::ImageResolver,
) -> PreviewOutcome {
    image_resolver.set_context(vault_root, note_rel, body);
    let headings = HeadingColors::from_theme(heading_palette);
    let heading_match = scroll_to_heading.map(|w| HeadingMatch::for_target(body, w));
    let prepared = preprocess(body);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(&prepared, options);
    let mut link: Option<LinkTarget> = None;
    let mut heading_rects = Vec::new();
    let mut heading_counter = 0usize;
    let mut find_scrolled = false;
    let scroll_needle = scroll_to_text
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase());
    let flash_needle = flash_text
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase());

    // Style state
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let code_span = false;
    let mut heading_level: Option<u8> = None;
    let mut link_dest: Option<String> = None;
    let mut list_depth: usize = 0;
    let mut ordered_list = false;
    let mut item_index: u32 = 0;
    let mut in_blockquote = false;
    let mut in_code_block = false;
    let mut code_buf = String::new();
    let mut table_row: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut in_table = false;
    let mut in_table_cell = false;
    let mut cell_buf = String::new();
    let mut paragraph_buf: Vec<(RichText, Option<String>)> = Vec::new();
    let mut image_dest: Option<String> = None;
    let mut image_alt = String::new();

    let flush_paragraph = |ui: &mut Ui,
                               buf: &mut Vec<(RichText, Option<String>)>,
                               open: &mut Option<LinkTarget>,
                               index: &VaultIndex,
                               note_rel: &str,
                               find_scrolled: &mut bool| {
        if buf.is_empty() {
            return;
        }
        let plain: String = buf.iter().map(|(r, _)| r.text().to_string()).collect();
        let resp = ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (rt, dest) in buf.drain(..) {
                if let Some(d) = dest {
                    let resp = ui.add(egui::Label::new(rt).sense(Sense::click()));
                    if resp.clicked() {
                        if let Some(r) = resolve_dest(index, note_rel, &d) {
                            *open = Some(r);
                        }
                    }
                    resp.on_hover_text(d);
                } else {
                    ui.label(rt);
                }
            }
        });
        apply_find_focus(
            ui,
            resp.response.rect,
            &plain,
            &scroll_needle,
            &flash_needle,
            flash_alpha,
            accent,
            find_scrolled,
        );
        // Breathing room between paragraphs / sentence blocks in preview.
        ui.add_space(20.0);
    };

    let style_text = |s: &str,
                      bold: bool,
                      italic: bool,
                      strike: bool,
                      code: bool,
                      heading: Option<u8>,
                      font_size: f32,
                      color: Color32,
                      headings: &HeadingColors|
     -> RichText {
        let mut size = font_size;
        let mut col = color;
        if let Some(level) = heading {
            col = headings.for_level(level);
            size = match level {
                1 => font_size * 1.75,
                2 => font_size * 1.45,
                3 => font_size * 1.25,
                4 => font_size * 1.12,
                _ => font_size * 1.05,
            };
        }
        let mut rt = RichText::new(s).size(size).color(col);
        if code {
            rt = rt
                .monospace()
                .background_color(Color32::from_black_alpha(60));
        } else {
            rt = rt.family(egui::FontFamily::Proportional);
        }
        if bold || heading.is_some() {
            rt = rt.strong();
        }
        if italic {
            rt = rt.italics();
        }
        if strike {
            rt = rt.strikethrough();
        }
        rt
    };

    ui.vertical(|ui| {
        for event in parser {
            match event {
                Event::Start(Tag::Paragraph) => {
                    paragraph_buf.clear();
                }
                Event::End(TagEnd::Paragraph) => {
                    if in_blockquote {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("│ ").color(muted).size(font_size));
                            ui.vertical(|ui| {
                                flush_paragraph(
                                    ui,
                                    &mut paragraph_buf,
                                    &mut link,
                                    index,
                                    note_rel,
                                    &mut find_scrolled,
                                );
                            });
                        });
                    } else {
                        flush_paragraph(
                            ui,
                            &mut paragraph_buf,
                            &mut link,
                            index,
                            note_rel,
                            &mut find_scrolled,
                        );
                    }
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    heading_level = Some(level as u8);
                    paragraph_buf.clear();
                }
                Event::End(TagEnd::Heading(_)) => {
                    let level = heading_level.unwrap_or(1);
                    let plain: String = paragraph_buf
                        .iter()
                        .map(|(r, _)| r.text())
                        .collect::<Vec<_>>()
                        .join("");
                    let rt = style_text(
                        &plain,
                        true,
                        false,
                        false,
                        false,
                        Some(level),
                        font_size,
                        text,
                        &headings,
                    );
                    let resp = ui.add(egui::Label::new(rt).sense(Sense::click()));
                    heading_rects.push((heading_counter, resp.rect));
                    heading_counter += 1;
                    if let Some(m) = &heading_match {
                        if m.matches(&plain) {
                            resp.scroll_to_me(Some(egui::Align::Center));
                        }
                    }
                    apply_find_focus(
                        ui,
                        resp.rect,
                        &plain,
                        &scroll_needle,
                        &flash_needle,
                        flash_alpha,
                        accent,
                        &mut find_scrolled,
                    );
                    ui.add_space(6.0);
                    paragraph_buf.clear();
                    heading_level = None;
                }
                Event::Start(Tag::Strong) => bold = true,
                Event::End(TagEnd::Strong) => bold = false,
                Event::Start(Tag::Emphasis) => italic = true,
                Event::End(TagEnd::Emphasis) => italic = false,
                Event::Start(Tag::Strikethrough) => strike = true,
                Event::End(TagEnd::Strikethrough) => strike = false,
                Event::Code(code) => {
                    let rt = style_text(
                        &code,
                        bold,
                        italic,
                        strike,
                        true,
                        heading_level,
                        font_size,
                        text,
                        &headings,
                    );
                    paragraph_buf.push((rt, None));
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    link_dest = Some(dest_url.to_string());
                }
                Event::End(TagEnd::Link) => {
                    link_dest = None;
                }
                Event::Start(Tag::Image { dest_url, .. }) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled);
                    image_dest = Some(dest_url.to_string());
                    image_alt.clear();
                }
                Event::End(TagEnd::Image) => {
                    let dest = image_dest.take().unwrap_or_default();
                    let alt = std::mem::take(&mut image_alt);
                    let max_w = ui.available_width().max(32.0);
                    if dest.starts_with("http://") || dest.starts_with("https://") {
                        ui.label(
                            RichText::new(format!("[image: {alt}] ({dest})"))
                                .size(font_size * 0.9)
                                .color(muted),
                        );
                    } else if let Some(resolved) = image_resolver.resolve(&dest, image_rels) {
                        let resp = media::show_fitted_image(ui, &resolved.abs, max_w);
                        let tip = if alt.is_empty() {
                            resolved.abs.display().to_string()
                        } else {
                            format!("{alt}\n{}", resolved.abs.display())
                        };
                        let resp = resp.on_hover_text(tip);
                        if resp.clicked() {
                            link = Some(LinkTarget::Image(resolved.rel));
                        }
                    } else {
                        ui.label(
                            RichText::new(format!("[missing image: {dest}]"))
                                .size(font_size * 0.9)
                                .color(muted),
                        );
                    }
                    ui.add_space(8.0);
                }
                Event::Text(raw) => {
                    let s = raw.as_ref();
                    if image_dest.is_some() {
                        image_alt.push_str(s);
                        continue;
                    }
                    if in_code_block {
                        code_buf.push_str(s);
                        continue;
                    }
                    if in_table_cell {
                        cell_buf.push_str(s);
                        continue;
                    }
                    // Inline #tags
                    if s.starts_with('#')
                        && !s.contains(' ')
                        && heading_level.is_none()
                        && link_dest.is_none()
                    {
                        let rt = RichText::new(s).size(font_size).color(accent).strong();
                        paragraph_buf.push((rt, None));
                        continue;
                    }
                    let color = if link_dest.is_some() { accent } else { text };
                    let rt = style_text(
                        s,
                        bold,
                        italic,
                        strike,
                        code_span,
                        heading_level,
                        font_size,
                        color,
                        &headings,
                    );
                    paragraph_buf.push((rt, link_dest.clone()));
                }
                Event::SoftBreak | Event::HardBreak => {
                    if in_code_block {
                        code_buf.push('\n');
                    } else if in_table_cell {
                        cell_buf.push(' ');
                    } else {
                        paragraph_buf.push((RichText::new(" ").size(font_size), None));
                    }
                }
                Event::Start(Tag::List(start)) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled);
                    list_depth += 1;
                    ordered_list = start.is_some();
                    item_index = start.unwrap_or(1) as u32;
                }
                Event::End(TagEnd::List(_)) => {
                    list_depth = list_depth.saturating_sub(1);
                    if list_depth == 0 {
                        ordered_list = false;
                    }
                    ui.add_space(2.0);
                }
                Event::Start(Tag::Item) => {
                    paragraph_buf.clear();
                }
                Event::End(TagEnd::Item) => {
                    let bullet = if ordered_list {
                        let n = item_index;
                        item_index += 1;
                        format!("{n}. ")
                    } else {
                        "• ".into()
                    };
                    let plain: String = paragraph_buf
                        .iter()
                        .map(|(r, _)| r.text().to_string())
                        .collect();
                    let resp = ui.horizontal_wrapped(|ui| {
                        ui.add_space((list_depth.saturating_sub(1) as f32) * 14.0);
                        ui.label(
                            RichText::new(bullet)
                                .monospace()
                                .color(muted)
                                .size(font_size),
                        );
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (rt, dest) in paragraph_buf.drain(..) {
                            if let Some(d) = dest {
                                let resp = ui.add(egui::Label::new(rt).sense(Sense::click()));
                                if resp.clicked() {
                                    if let Some(r) = resolve_dest(index, note_rel, &d) {
                                        link = Some(r);
                                    }
                                }
                            } else {
                                ui.label(rt);
                            }
                        }
                    });
                    apply_find_focus(
                        ui,
                        resp.response.rect,
                        &plain,
                        &scroll_needle,
                        &flash_needle,
                        flash_alpha,
                        accent,
                        &mut find_scrolled,
                    );
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled);
                    in_blockquote = true;
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    in_blockquote = false;
                    ui.add_space(4.0);
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled);
                    in_code_block = true;
                    code_buf.clear();
                    let _ = kind;
                }
                Event::End(TagEnd::CodeBlock) => {
                    in_code_block = false;
                    let code = std::mem::take(&mut code_buf);
                    let plain = code.trim_end().to_string();
                    let resp = egui::Frame::new()
                        .fill(Color32::from_black_alpha(80))
                        .inner_margin(8)
                        .stroke(Stroke::new(1.0_f32, muted.gamma_multiply(0.4)))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&plain)
                                        .monospace()
                                        .size(font_size * 0.92)
                                        .color(text),
                                )
                                .selectable(true),
                            );
                        });
                    apply_find_focus(
                        ui,
                        resp.response.rect,
                        &plain,
                        &scroll_needle,
                        &flash_needle,
                        flash_alpha,
                        accent,
                        &mut find_scrolled,
                    );
                    ui.add_space(8.0);
                }
                Event::Start(Tag::Table(_)) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled);
                    in_table = true;
                    table_rows.clear();
                }
                Event::End(TagEnd::Table) => {
                    in_table = false;
                    if !table_rows.is_empty() {
                        let cols = table_rows.iter().map(|r| r.len()).max().unwrap_or(1);
                        let content_bg = ui.visuals().extreme_bg_color;
                        let dark = (content_bg.r() as u16
                            + content_bg.g() as u16
                            + content_bg.b() as u16)
                            < 382;
                        let toward = if dark {
                            Color32::WHITE
                        } else {
                            Color32::BLACK
                        };
                        let (base_t, alt_t) = if dark {
                            (0.09, 0.15)
                        } else {
                            (0.07, 0.12)
                        };
                        let row_base = mix(content_bg, toward, base_t);
                        let row_alt = mix(content_bg, toward, alt_t);
                        let row_header = mix(row_base, accent, 0.12);
                        let col_rule = muted.gamma_multiply(0.65);

                        Frame::new()
                            .stroke(Stroke::new(1.0_f32, muted.gamma_multiply(0.55)))
                            .inner_margin(Margin::symmetric(4, 2))
                            .show(ui, |ui| {
                                egui::Grid::new(ui.scope_id().with("md-table"))
                                    .with_row_color(move |row, _| {
                                        Some(if row == 0 {
                                            row_header
                                        } else if row % 2 == 0 {
                                            row_base
                                        } else {
                                            row_alt
                                        })
                                    })
                                    .num_columns(cols)
                                    .spacing([14.0, 8.0])
                                    .show(ui, |ui| {
                                        for (ri, row) in table_rows.iter().enumerate() {
                                            for (ci, cell) in row.iter().enumerate() {
                                                let rt = if ri == 0 {
                                                    RichText::new(cell)
                                                        .strong()
                                                        .size(font_size)
                                                        .color(accent)
                                                } else {
                                                    RichText::new(cell)
                                                        .size(font_size)
                                                        .color(text)
                                                };
                                                ui.horizontal(|ui| {
                                                    if ci > 0 {
                                                        let h = (font_size + 8.0).max(16.0);
                                                        let (rect, _) = ui.allocate_exact_size(
                                                            Vec2::new(1.0, h),
                                                            Sense::hover(),
                                                        );
                                                        ui.painter().rect_filled(
                                                            rect,
                                                            0.0,
                                                            col_rule,
                                                        );
                                                        ui.add_space(8.0);
                                                    }
                                                    ui.label(rt);
                                                });
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                        ui.add_space(8.0);
                    }
                    table_rows.clear();
                }
                Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                    table_row.clear();
                }
                Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                    if !table_row.is_empty() {
                        table_rows.push(std::mem::take(&mut table_row));
                    }
                }
                Event::Start(Tag::TableCell) => {
                    in_table_cell = true;
                    cell_buf.clear();
                }
                Event::End(TagEnd::TableCell) => {
                    in_table_cell = false;
                    table_row.push(std::mem::take(&mut cell_buf));
                }
                Event::Rule => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled);
                    ui.add(egui::Separator::default().spacing(12.0));
                }
                Event::TaskListMarker(checked) => {
                    let mark = if checked { "☑ " } else { "☐ " };
                    paragraph_buf.push((RichText::new(mark).size(font_size).color(accent), None));
                }
                Event::Html(html) | Event::InlineHtml(html) => {
                    // Strip simple tags; keep text-ish content
                    let stripped = html.replace('<', "").replace('>', "");
                    if !stripped.trim().is_empty() {
                        paragraph_buf.push((
                            RichText::new(stripped).size(font_size * 0.9).color(muted),
                            None,
                        ));
                    }
                }
                _ => {}
            }
        }
        flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled);
    });

    let _ = (
        code_span,
        FontId::default(),
        TextStyle::Body,
        Vec2::ZERO,
        in_table,
    );
    PreviewOutcome {
        link,
        heading_rects,
        find_scrolled,
    }
}

fn resolve_dest(index: &VaultIndex, note_rel: &str, dest: &str) -> Option<LinkTarget> {
    let dest = dest.trim();
    let (target, fragment) = match dest.strip_prefix("wikilink:") {
        Some(inner) => split_fragment(inner),
        None => {
            if has_uri_scheme(dest) {
                return Some(LinkTarget::External(dest.to_string()));
            }
            split_fragment(dest)
        }
    };
    let heading = fragment
        .map(|f| f.trim_start_matches('^').trim().to_string())
        .filter(|f| !f.is_empty());
    if target.is_empty() {
        // `[[#Heading]]` / `[t](#anchor)`: stay in this note.
        return heading.map(LinkTarget::Heading);
    }
    if is_image_target(target) {
        return Some(LinkTarget::Image(target.replace('\\', "/")));
    }
    match resolve_link(index, note_rel, target) {
        ResolvedLink::Note(id) => index.get(id).map(|n| LinkTarget::Note {
            rel: n.rel.clone(),
            heading,
        }),
        ResolvedLink::Missing { target } => Some(LinkTarget::Note {
            rel: ensure_md(&target),
            heading,
        }),
        ResolvedLink::External { url } => Some(LinkTarget::External(url)),
    }
}

fn split_fragment(dest: &str) -> (&str, Option<&str>) {
    match dest.split_once('#') {
        Some((t, f)) => (t.trim(), Some(f)),
        None => (dest.trim(), None),
    }
}

fn ensure_md(target: &str) -> String {
    if target.to_lowercase().ends_with(".md") {
        target.to_string()
    } else {
        format!("{target}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_with(rels: &[&str]) -> (tempfile::TempDir, VaultIndex) {
        let dir = tempfile::tempdir().unwrap();
        let mut index = VaultIndex::new();
        for rel in rels {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("# {rel}\n")).unwrap();
            index.upsert_file(dir.path(), &path).unwrap();
        }
        (dir, index)
    }

    #[test]
    fn image_destinations_keep_typed_targets_for_resolver() {
        let index = VaultIndex::new();
        assert_eq!(
            resolve_dest(&index, "notes/page.md", "../images/photo.png"),
            Some(LinkTarget::Image("../images/photo.png".into()))
        );
        assert_eq!(
            resolve_dest(&index, "notes/page.md", "../../outside.png"),
            Some(LinkTarget::Image("../../outside.png".into()))
        );
    }

    #[test]
    fn heading_only_wikilink_jumps_within_current_note() {
        let (_d, index) = index_with(&["Home.md"]);
        assert_eq!(
            resolve_dest(&index, "Home.md", "wikilink:#Section"),
            Some(LinkTarget::Heading("Section".into()))
        );
    }

    #[test]
    fn anchor_link_jumps_within_current_note() {
        let (_d, index) = index_with(&["Home.md"]);
        assert_eq!(
            resolve_dest(&index, "Home.md", "#anchor"),
            Some(LinkTarget::Heading("anchor".into()))
        );
    }

    #[test]
    fn scheme_links_are_external() {
        let (_d, index) = index_with(&["Home.md"]);
        for url in ["mailto:x@y.z", "https://example.com", "obsidian://open"] {
            assert_eq!(
                resolve_dest(&index, "Home.md", url),
                Some(LinkTarget::External(url.into())),
                "{url}"
            );
        }
    }

    #[test]
    fn note_links_keep_their_heading() {
        let (_d, index) = index_with(&["Home.md", "Other.md"]);
        assert_eq!(
            resolve_dest(&index, "Home.md", "wikilink:Other#Part"),
            Some(LinkTarget::Note {
                rel: "Other.md".into(),
                heading: Some("Part".into()),
            })
        );
        assert_eq!(
            resolve_dest(&index, "Home.md", "wikilink:Other"),
            Some(LinkTarget::Note {
                rel: "Other.md".into(),
                heading: None,
            })
        );
        // Missing notes are still offered for creation, with a real name.
        assert_eq!(
            resolve_dest(&index, "Home.md", "wikilink:Brand New"),
            Some(LinkTarget::Note {
                rel: "Brand New.md".into(),
                heading: None,
            })
        );
    }

    #[test]
    fn preprocess_keeps_wikilink_heading_and_survives_markdown_parse() {
        let out = preprocess("See [[My Note#Deep Dive|here]] and [[#Local]].");
        let mut dests = Vec::new();
        for ev in Parser::new_ext(&out, Options::empty()) {
            if let Event::Start(Tag::Link { dest_url, .. }) = ev {
                dests.push(dest_url.to_string());
            }
        }
        assert_eq!(dests, vec!["wikilink:My Note#Deep Dive", "wikilink:#Local"]);
    }

    #[test]
    fn exact_heading_wins_over_slug_collision() {
        let body = "# A B\n\ntext\n\n# A-B\n";
        // `A-B` exists verbatim, so the slug twin `A B` must not match.
        let m = HeadingMatch::for_target(body, "A-B");
        assert!(!m.matches("A B"));
        assert!(m.matches("A-B"));
        // Without an exact heading the slug fallback still works.
        let m = HeadingMatch::for_target(body, "a-b");
        assert!(m.matches("A B"));
    }

    #[test]
    fn heading_match_accepts_slug_anchors() {
        assert!(heading_matches("My Heading", "My Heading"));
        assert!(heading_matches("My Heading", "my-heading"));
        assert!(heading_matches("Setup & Install", "setup--install"));
        assert!(!heading_matches("My Heading", "other"));
    }
}
