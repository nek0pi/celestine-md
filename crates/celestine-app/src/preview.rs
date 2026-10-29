use crate::media::{self, is_image_target};
use crate::table_ui::{self, TableStyle};
use celestine_core::{has_uri_scheme, resolve_link, ResolvedLink, VaultIndex};
use egui::{Color32, FontId, RichText, Sense, Stroke, TextStyle, Ui, Vec2};
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
    /// A checkbox click, expressed against the unmodified Markdown body.
    pub task_toggle: Option<TaskToggle>,
    pub image_resize: Option<crate::image_size::ResizeEdit>,
}

/// Byte offset of a task marker in the original note body, before preprocessing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskToggle {
    pub marker_offset: usize,
    pub checked: bool,
}

fn preview_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

fn task_markers(body: &str) -> Vec<TaskToggle> {
    Parser::new_ext(body, preview_options())
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::TaskListMarker(checked) => Some(TaskToggle {
                marker_offset: range.start,
                checked,
            }),
            _ => None,
        })
        .collect()
}

/// Change only the checkbox character; never reconstruct or normalize the note.
pub fn apply_task_toggle(buffer: &mut String, body: &str, toggle: TaskToggle) -> bool {
    if !buffer.ends_with(body) || !task_markers(body).contains(&toggle) {
        return false;
    }
    let offset = buffer.len() - body.len() + toggle.marker_offset;
    if !matches!(buffer.get(offset..offset + 3), Some("[ ]" | "[x]" | "[X]")) {
        return false;
    }
    buffer.replace_range(
        offset + 1..offset + 2,
        if toggle.checked { " " } else { "x" },
    );
    true
}

struct PreviewListItem {
    bullet: String,
    depth: usize,
    task: Option<(bool, Option<TaskToggle>)>,
    rendered: bool,
}

/// Draw one paragraph of a list item. A separate text column provides hanging
/// indentation, including for wrapped links and subsequent loose paragraphs.
fn show_list_paragraph(
    ui: &mut Ui,
    buf: &mut Vec<(RichText, Option<String>)>,
    item: &mut PreviewListItem,
    note_rel: &str,
    vault_root: &Path,
    index: &VaultIndex,
    accent: Color32,
    muted: Color32,
    text: Color32,
    border: Color32,
    font_size: f32,
    open: &mut Option<LinkTarget>,
    toggle: &mut Option<TaskToggle>,
) -> egui::Rect {
    let first = !item.rendered;
    item.rendered = true;
    let id = ui.scope_id().with((
        "preview-task",
        vault_root,
        note_rel,
        item.task
            .and_then(|(_, source)| source)
            .map(|t| t.marker_offset),
    ));
    let checked = item.task.is_some_and(|(checked, _)| checked);
    let progress = if item.task.is_some() {
        ui.ctx()
            .animate_value_with_time(id, if checked { 1.0 } else { 0.0 }, 0.25)
    } else {
        0.0
    };
    let eased = progress * progress * (3.0 - 2.0 * progress);
    let size = font_size.clamp(18.0, 24.0);
    let control_w = if item.task.is_some() {
        size + 12.0
    } else {
        ui.painter()
            .layout_no_wrap(item.bullet.clone(), FontId::proportional(font_size), muted)
            .size()
            .x
            .max(size)
            + 12.0
    };
    let response = ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(item.depth.saturating_sub(1) as f32 * 24.0);
        let (control_rect, _) =
            ui.allocate_exact_size(egui::vec2(control_w, size + 8.0), Sense::hover());
        if first {
            if let Some((_, source)) = item.task {
                let response = ui
                    .interact(control_rect, id.with("control"), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(if checked {
                        "Reopen task"
                    } else {
                        "Complete task"
                    });
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::Role::CheckBox,
                        true,
                        checked,
                        buf.iter().map(|(rt, _)| rt.text()).collect::<String>(),
                    )
                });
                if response.clicked() {
                    *toggle = source;
                    ui.ctx().request_repaint();
                }
                let pulse = (progress * std::f32::consts::PI).sin() * 1.2;
                let box_rect = egui::Rect::from_center_size(
                    egui::pos2(
                        control_rect.left() + size * 0.5 + 2.0,
                        control_rect.top() + size * 0.5 + 3.0,
                    ),
                    egui::vec2(size + pulse, size + pulse),
                );
                if response.hovered() || response.has_focus() {
                    ui.painter().rect_filled(
                        box_rect.expand(4.0),
                        7.0,
                        accent.gamma_multiply(0.12),
                    );
                }
                let fill = ui.visuals().extreme_bg_color.lerp_to_gamma(accent, eased);
                ui.painter().rect(
                    box_rect,
                    5.0,
                    fill,
                    Stroke::new(
                        1.4,
                        border.lerp_to_gamma(accent, if response.hovered() { 1.0 } else { eased }),
                    ),
                    egui::StrokeKind::Inside,
                );
                if progress > 0.0 {
                    let a = box_rect.min + egui::vec2(size * 0.23, size * 0.51);
                    let b = box_rect.min + egui::vec2(size * 0.43, size * 0.71);
                    let c = box_rect.min + egui::vec2(size * 0.78, size * 0.30);
                    let ink = if accent.intensity() > 0.55 {
                        Color32::from_rgb(20, 24, 30)
                    } else {
                        Color32::WHITE
                    };
                    let stroke = Stroke::new(2.1, ink.gamma_multiply(eased));
                    ui.painter()
                        .line_segment([a, a.lerp(b, (eased * 2.5).min(1.0))], stroke);
                    if eased > 0.4 {
                        ui.painter()
                            .line_segment([b, b.lerp(c, (eased - 0.4) / 0.6)], stroke);
                    }
                }
            } else {
                ui.painter().text(
                    control_rect.left_top() + egui::vec2(2.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    &item.bullet,
                    FontId::proportional(font_size),
                    muted,
                );
            }
        }
        ui.vertical(|ui| {
            ui.set_width(ui.available_width().max(1.0));
            let mut job = egui::text::LayoutJob::default();
            let mut links = Vec::new();
            let mut char_offset = 0;
            for (rt, dest) in buf.drain(..) {
                let count = rt.text().chars().count();
                let col = rt
                    .color_override()
                    .unwrap_or(text)
                    .lerp_to_gamma(muted, eased * 0.85);
                rt.color(col).append_to(
                    &mut job,
                    ui.style(),
                    egui::FontSelection::Default,
                    egui::Align::Center,
                );
                if let Some(dest) = dest {
                    links.push((char_offset..char_offset + count, dest));
                }
                char_offset += count;
            }
            job.wrap.max_width = ui.available_width();
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let label = ui.add(egui::Label::new(galley.clone()).selectable(true));
            let origin = label.rect.min.to_vec2();
            let total_width: f32 = galley.rows.iter().map(|r| r.size.x).sum();
            let mut strike_left = total_width * eased;
            let mut row_start = 0;
            for (row_index, row) in galley.rows.iter().enumerate() {
                let rect = row.rect().translate(origin);
                if strike_left > 0.0 && item.task.is_some() {
                    let y = rect.center().y;
                    ui.painter().line_segment(
                        [
                            egui::pos2(rect.left(), y),
                            egui::pos2(rect.left() + strike_left.min(rect.width()), y),
                        ],
                        Stroke::new(1.2, muted.gamma_multiply(eased)),
                    );
                    strike_left -= rect.width();
                }
                for (link_index, (range, dest)) in links.iter().enumerate() {
                    let start = range.start.saturating_sub(row_start).min(row.glyphs.len());
                    let end = range.end.saturating_sub(row_start).min(row.glyphs.len());
                    if start < end {
                        let left = row.glyphs[start].pos.x;
                        let last = &row.glyphs[end - 1];
                        let link_rect = egui::Rect::from_min_max(
                            egui::pos2(origin.x + row.pos.x + left, rect.top()),
                            egui::pos2(
                                origin.x + row.pos.x + last.pos.x + last.advance_width,
                                rect.bottom(),
                            ),
                        );
                        let response = ui
                            .interact(
                                link_rect,
                                label.id.with((row_index, link_index)),
                                Sense::click(),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(dest);
                        if response.clicked() {
                            *open = resolve_dest(index, note_rel, dest);
                        }
                    }
                }
                row_start += row.glyphs.len() + usize::from(row.ends_with_newline);
            }
        });
    });
    ui.add_space(if item.task.is_some() { 4.0 } else { 2.0 });
    response.response.rect
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

/// Target a single preview block for find jump / flash.
///
/// `needle` should be a normalized line (see [`normalize_find_needle`]), and
/// `occurrence` is the 0-based index among preview blocks that match it.
#[derive(Debug, Clone)]
pub struct PreviewFindFocus {
    pub needle: String,
    pub occurrence: usize,
    pub alpha: f32,
    pub do_scroll: bool,
}

/// Strip common Markdown markers so source lines can match rendered preview text.
pub fn normalize_find_needle(line: &str) -> String {
    let mut t = line.trim();
    while let Some(rest) = t.strip_prefix('>') {
        t = rest.trim_start();
    }
    let hash = t.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hash) {
        let after = &t[hash..];
        if after.starts_with(|c: char| c.is_whitespace()) || after.is_empty() {
            t = after.trim_start();
        }
    }
    for prefix in [
        "- [ ] ",
        "- [x] ",
        "- [X] ",
        "* [ ] ",
        "* [x] ",
        "* [X] ",
        "- ",
        "* ",
        "+ ",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest;
            break;
        }
    }
    if let Some((num, rest)) = t.split_once(". ") {
        if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            t = rest;
        }
    }
    t.trim().to_lowercase()
}

/// Amber wash for find/search jump flash — distinct on dark and light themes.
pub fn find_flash_fill(alpha: f32) -> Color32 {
    let a = (alpha * 140.0).clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(255, 196, 48, a)
}

fn preview_plain_matches(plain_lower: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return false;
    }
    let p = plain_lower.trim();
    if p.is_empty() {
        return false;
    }
    p == needle_lower || p.contains(needle_lower)
}

fn apply_find_focus(
    ui: &Ui,
    rect: egui::Rect,
    plain: &str,
    focus: Option<&PreviewFindFocus>,
    hits_seen: &mut usize,
    _accent: Color32,
    find_scrolled: &mut bool,
) {
    let Some(focus) = focus else {
        return;
    };
    if focus.needle.chars().count() < 2 {
        return;
    }
    let p = plain.to_lowercase();
    if !preview_plain_matches(&p, &focus.needle) {
        return;
    }
    let idx = *hits_seen;
    *hits_seen += 1;
    // Only the chosen occurrence — never wash every past / sibling match.
    if idx != focus.occurrence {
        return;
    }
    if focus.alpha > 0.01 {
        ui.painter().rect_filled(
            rect.expand(3.0),
            4.0,
            find_flash_fill(focus.alpha),
        );
    }
    if focus.do_scroll && !*find_scrolled {
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

/// Vertical gap after each preview paragraph (doubled from the original 20px).
/// Extra blank lines in the source multiply this unit (3 empties → 3× gap).
const PARA_GAP: f32 = 40.0;

/// HTML comment inserted for runs of 2+ blank lines so preview can scale gaps.
/// CommonMark collapses blank runs; this preserves the count for rendering.
const VBLANK_MARK_PREFIX: &str = "<!--celestine-vblank:";
const VBLANK_MARK_SUFFIX: &str = "-->";

fn vblank_mark(count: usize) -> String {
    format!("{VBLANK_MARK_PREFIX}{count}{VBLANK_MARK_SUFFIX}")
}

fn parse_vblank_mark(html: &str) -> Option<usize> {
    let s = html.trim();
    let rest = s.strip_prefix(VBLANK_MARK_PREFIX)?;
    let num = rest.strip_suffix(VBLANK_MARK_SUFFIX)?;
    let n: usize = num.parse().ok()?;
    (n >= 2).then_some(n)
}

fn fence_opener(line: &str) -> Option<&str> {
    let t = line.trim_start();
    for fence in ["```", "~~~"] {
        if t.starts_with(fence) {
            return Some(fence);
        }
    }
    None
}

/// Replace runs of 2+ blank lines (outside fenced code) with a countable marker.
/// One blank line stays a normal Markdown paragraph break (base [`PARA_GAP`]).
fn inject_extra_vblank_markers(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 32);
    let mut blank_run: usize = 0;
    let mut fence: Option<&str> = None;

    let flush_blanks = |out: &mut String, blank_run: &mut usize| {
        let n = std::mem::take(blank_run);
        if n == 0 {
            return;
        }
        // Prior content line already ends with `\n` (via split_inclusive).
        // One more `\n` yields the Markdown block separator; extras become a marker.
        out.push('\n');
        if n >= 2 {
            out.push_str(&vblank_mark(n));
            out.push_str("\n\n");
        }
    };

    for line in body.split_inclusive('\n') {
        let line_body = line.strip_suffix('\n').unwrap_or(line);
        let line_body = line_body.strip_suffix('\r').unwrap_or(line_body);
        let is_blank = line_body.trim().is_empty();

        if let Some(f) = fence {
            out.push_str(line);
            if line_body.trim_start().starts_with(f) {
                fence = None;
            }
            continue;
        }

        if let Some(f) = fence_opener(line_body) {
            flush_blanks(&mut out, &mut blank_run);
            out.push_str(line);
            fence = Some(f);
            continue;
        }

        if is_blank {
            blank_run += 1;
            continue;
        }

        flush_blanks(&mut out, &mut blank_run);
        out.push_str(line);
    }
    // Trailing blank runs: preserve countable gaps at end of note.
    flush_blanks(&mut out, &mut blank_run);
    out
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
    inject_extra_vblank_markers(&out)
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
    border: Color32,
    font_size: f32,
    scroll_to_heading: Option<&str>,
    find_focus: Option<&PreviewFindFocus>,
    heading_palette: &[Color32; 6],
    image_resolver: &mut media::ImageResolver,
) -> PreviewOutcome {
    image_resolver.set_context(vault_root, note_rel, body);
    let headings = HeadingColors::from_theme(heading_palette);
    let heading_match = scroll_to_heading.map(|w| HeadingMatch::for_target(body, w));
    let prepared = preprocess(body);
    let options = preview_options();

    let parser = Parser::new_ext(&prepared, options);
    let mut source_images = crate::image_size::embeds(body).into_iter();
    let mut image_source = None;
    let mut image_resize = None;
    let mut link: Option<LinkTarget> = None;
    let mut heading_rects = Vec::new();
    let mut heading_counter = 0usize;
    let mut find_scrolled = false;
    let mut find_hits_seen = 0usize;

    // Style state
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let code_span = false;
    let mut heading_level: Option<u8> = None;
    let mut link_dest: Option<String> = None;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut items: Vec<PreviewListItem> = Vec::new();
    let markers = task_markers(body);
    let mut markers = markers.into_iter();
    let mut task_toggle = None;
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
                           find_scrolled: &mut bool,
                           find_hits_seen: &mut usize,
                           item: Option<&mut PreviewListItem>,
                           toggle: &mut Option<TaskToggle>| {
        if let Some(item) = item {
            if !buf.is_empty() || !item.rendered {
                let plain: String = buf.iter().map(|(r, _)| r.text()).collect();
                let rect = show_list_paragraph(
                    ui, buf, item, note_rel, vault_root, index, accent, muted, text, border,
                    font_size, open, toggle,
                );
                apply_find_focus(
                    ui,
                    rect,
                    &plain,
                    find_focus,
                    find_hits_seen,
                    accent,
                    find_scrolled,
                );
            }
            return;
        }
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
            find_focus,
            find_hits_seen,
            accent,
            find_scrolled,
        );
        // Breathing room between paragraphs / sentence blocks in preview.
        // Extra blank lines add further multiples via `celestine-vblank` markers.
        ui.add_space(PARA_GAP);
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
                                    &mut find_hits_seen,
                                    items.last_mut(), &mut task_toggle,
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
                            &mut find_hits_seen,
                            items.last_mut(), &mut task_toggle,
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
                        find_focus,
                        &mut find_hits_seen,
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
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled, &mut find_hits_seen, items.last_mut(), &mut task_toggle);
                    image_dest = Some(dest_url.to_string());
                    image_alt.clear();
                    image_source = source_images.next();
                }
                Event::End(TagEnd::Image) => {
                    let dest = image_dest.take().unwrap_or_default();
                    let source = image_source.take();
                    let dest = source.as_ref().map_or(dest.as_str(), |s| s.destination.as_str());
                    let alt = std::mem::take(&mut image_alt);
                    let max_w = ui.available_width().max(32.0);
                    if dest.starts_with("http://") || dest.starts_with("https://") {
                        ui.label(
                            RichText::new(format!("[image: {alt}] ({dest})"))
                                .size(font_size * 0.9)
                                .color(muted),
                        );
                    } else if let Some(resolved) = image_resolver.resolve(dest, image_rels) {
                        let uri = media::path_to_file_uri(&resolved.abs);
                        let id = ui.make_persistent_id(("image-resize", note_rel, source.as_ref().map(|s| s.range.start)));
                        let size = crate::image_size::fitted_size(ui, id, &uri, source.as_ref().and_then(|s| s.size), max_w);
                        let resp = ui.add(egui::Image::new(uri).fit_to_exact_size(size).maintain_aspect_ratio(false).sense(Sense::click()));
                        if let Some(source) = &source {
                            if let Some(edit) = crate::image_size::resize_handle(ui, id, resp.rect, max_w, source) {
                                image_resize = Some(edit);
                            }
                        }
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
                event @ (Event::SoftBreak | Event::HardBreak) => {
                    if in_code_block {
                        code_buf.push('\n');
                    } else if in_table_cell {
                        cell_buf.push(' ');
                    } else {
                        paragraph_buf.push((
                            RichText::new(if matches!(event, Event::HardBreak) { "\n" } else { " " }).size(font_size),
                            None,
                        ));
                    }
                }
                Event::Start(Tag::List(start)) => {
                    // Flush the parent before descending so children cannot steal its text.
                    flush_paragraph(
                        ui,
                        &mut paragraph_buf,
                        &mut link,
                        index,
                        note_rel,
                        &mut find_scrolled,
                        &mut find_hits_seen,
                        items.last_mut(),
                        &mut task_toggle,
                    );
                    lists.push(start);
                }
                Event::End(TagEnd::List(_)) => {
                    lists.pop();
                    ui.add_space(2.0);
                }
                Event::Start(Tag::Item) => {
                    paragraph_buf.clear();
                    let bullet = match lists.last_mut() {
                        Some(Some(next)) => {
                            let n = *next;
                            *next += 1;
                            format!("{n}.")
                        }
                        _ => "•".into(),
                    };
                    items.push(PreviewListItem {
                        bullet,
                        depth: lists.len(),
                        task: None,
                        rendered: false,
                    });
                }
                Event::End(TagEnd::Item) => {
                    flush_paragraph(
                        ui,
                        &mut paragraph_buf,
                        &mut link,
                        index,
                        note_rel,
                        &mut find_scrolled,
                        &mut find_hits_seen,
                        items.last_mut(),
                        &mut task_toggle,
                    );
                    items.pop();
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled, &mut find_hits_seen, items.last_mut(), &mut task_toggle);
                    in_blockquote = true;
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    in_blockquote = false;
                    ui.add_space(4.0);
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled, &mut find_hits_seen, items.last_mut(), &mut task_toggle);
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
                        find_focus,
                        &mut find_hits_seen,
                        accent,
                        &mut find_scrolled,
                    );
                    ui.add_space(8.0);
                }
                Event::Start(Tag::Table(_)) => {
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled, &mut find_hits_seen, items.last_mut(), &mut task_toggle);
                    in_table = true;
                    table_rows.clear();
                }
                Event::End(TagEnd::Table) => {
                    in_table = false;
                    if !table_rows.is_empty() {
                        let style = TableStyle {
                            border,
                            accent,
                            text,
                            muted,
                            content_bg: ui.visuals().extreme_bg_color,
                            panel_bg: ui.visuals().faint_bg_color,
                            font_size,
                        };
                        let before = ui.min_rect();
                        table_ui::show_readonly(ui, &table_rows, style);
                        let after = ui.min_rect();
                        let rect = before.union(after);
                        let plain = table_rows
                            .iter()
                            .flat_map(|r| r.iter().cloned())
                            .collect::<Vec<_>>()
                            .join(" ");
                        apply_find_focus(
                            ui,
                            rect,
                            &plain,
                            find_focus,
                            &mut find_hits_seen,
                            accent,
                            &mut find_scrolled,
                        );
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
                    flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled, &mut find_hits_seen, items.last_mut(), &mut task_toggle);
                    ui.add(egui::Separator::default().spacing(12.0));
                }
                Event::TaskListMarker(checked) => {
                    let source = markers.next().filter(|source| source.checked == checked);
                    if let Some(item) = items.last_mut() {
                        item.task = Some((checked, source));
                    }
                }
                Event::Html(html) | Event::InlineHtml(html) => {
                    if let Some(n) = parse_vblank_mark(&html) {
                        // Base PARA_GAP already added after the previous block;
                        // extras scale so N blank lines → N × PARA_GAP total.
                        ui.add_space((n - 1) as f32 * PARA_GAP);
                    } else {
                        // Strip simple tags; keep text-ish content
                        let stripped = html.replace('<', "").replace('>', "");
                        if !stripped.trim().is_empty() {
                            paragraph_buf.push((
                                RichText::new(stripped).size(font_size * 0.9).color(muted),
                                None,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        flush_paragraph(ui, &mut paragraph_buf, &mut link, index, note_rel, &mut find_scrolled, &mut find_hits_seen, items.last_mut(), &mut task_toggle);
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
        task_toggle,
        image_resize,
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
    fn task_updates_preserve_every_byte_except_the_marker() {
        let body = "Intro :smile: [[Other|文]]\r\n\r\n- [ ] duplicate\r\n  - [X] child\r\n\r\n1. [x] duplicate\r\n\r\n> - [ ] quoted\r\n\r\n```md\r\n- [ ] code\r\n```\r\n\r\n    - [ ] indented code\r\n";
        let prefix = "\u{feff}---\r\ntitle: Test\r\n---\r\n";
        let markers = task_markers(body);
        assert_eq!(markers.len(), 4);
        for marker in markers {
            let original = format!("{prefix}{body}");
            let mut buffer = original.clone();
            assert!(apply_task_toggle(&mut buffer, body, marker));
            let offset = prefix.len() + marker.marker_offset + 1;
            assert_eq!(&buffer[..offset], &original[..offset]);
            assert_eq!(&buffer[offset + 1..], &original[offset + 1..]);
            assert_eq!(
                &buffer[offset..offset + 1],
                if marker.checked { " " } else { "x" }
            );
            let current_body = buffer[prefix.len()..].to_string();
            let reverse = TaskToggle {
                checked: !marker.checked,
                ..marker
            };
            assert!(apply_task_toggle(&mut buffer, &current_body, reverse));
            assert_eq!(buffer.to_lowercase(), original.to_lowercase());
        }
    }

    #[test]
    fn task_updates_reject_stale_or_non_task_markers() {
        let body = "`[ ]`\n\n- [ ] actual\n";
        let mut buffer = body.to_string();
        assert!(!apply_task_toggle(
            &mut buffer,
            body,
            TaskToggle {
                marker_offset: 1,
                checked: false
            }
        ));
        let marker = task_markers(body)[0];
        assert!(!apply_task_toggle(
            &mut buffer,
            body,
            TaskToggle {
                checked: true,
                ..marker
            }
        ));
        buffer.push_str("changed");
        let before = buffer.clone();
        assert!(!apply_task_toggle(&mut buffer, body, marker));
        assert_eq!(buffer, before);
    }

    fn preview_frame(
        ctx: &egui::Context,
        body: &str,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (PreviewOutcome, egui::FullOutput) {
        let mut outcome = None;
        let mut resolver = media::ImageResolver::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(360.0, 1000.0),
                )),
                ..Default::default()
            },
            |ui| {
                let muted = ui.visuals().weak_text_color();
                let text = ui.visuals().text_color();
                let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
                outcome = Some(show_preview(
                    ui,
                    body,
                    "Test.md",
                    Path::new("."),
                    &VaultIndex::new(),
                    &[],
                    Color32::from_rgb(110, 190, 230),
                    muted,
                    text,
                    border,
                    18.0,
                    None,
                    None,
                    &[Color32::WHITE; 6],
                    &mut resolver,
                ));
            },
        );
        output.textures_delta.clear();
        (outcome.unwrap(), output)
    }

    fn painted_text(output: &egui::FullOutput) -> Vec<&egui::epaint::TextShape> {
        fn collect<'a>(shape: &'a egui::Shape, texts: &mut Vec<&'a egui::epaint::TextShape>) {
            match shape {
                egui::Shape::Text(text) => texts.push(text),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, texts);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for shape in &output.shapes {
            collect(&shape.shape, &mut texts);
        }
        texts
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn preview_tasks_wrap_keep_nested_order_and_toggle_the_original_marker() {
        let ctx = egui::Context::default();
        let body = "- [ ] Parent [[Other|link]] :smile: with a long label that wraps across multiple lines\n  - [X] Child\n\n- [ ] Loose\n\n  second paragraph\n\n3. numbered\n   - nested bullet\n4. next numbered\n";
        let (_, output) = preview_frame(&ctx, body, 0.0, vec![]);
        let texts = painted_text(&output);
        let labels: Vec<_> = texts
            .iter()
            .map(|text| text.galley.job.text.as_str())
            .collect();
        let parent = texts
            .iter()
            .find(|t| t.galley.job.text.starts_with("Parent"))
            .unwrap();
        assert!(parent.galley.rows.len() > 1);
        let index = |prefix: &str| labels.iter().position(|s| s.starts_with(prefix)).unwrap();
        assert!(index("Parent") < index("Child"));
        assert!(index("Child") < index("Loose"));
        assert!(index("Loose") < index("second paragraph"));
        assert!(labels.contains(&"3.") && labels.contains(&"4."));
        assert_eq!(labels.iter().filter(|s| **s == "•").count(), 1);
        // Checkbox lies immediately to the left of the hanging text column.
        let pos = parent.pos + egui::vec2(-21.0, 12.0);
        preview_frame(&ctx, body, 0.05, pointer(pos, true));
        let (outcome, _) = preview_frame(&ctx, body, 0.1, pointer(pos, false));
        assert_eq!(outcome.task_toggle, Some(task_markers(body)[0]));
        let mut updated = body.to_string();
        assert!(apply_task_toggle(
            &mut updated,
            body,
            outcome.task_toggle.unwrap()
        ));
        preview_frame(&ctx, &updated, 0.12, vec![]);
        let (_, animated) = preview_frame(&ctx, &updated, 0.24, vec![]);
        let (_, finished) = preview_frame(&ctx, &updated, 0.5, vec![]);
        let color = |out: &egui::FullOutput| {
            painted_text(out)
                .iter()
                .find(|t| t.galley.job.text.starts_with("Parent"))
                .unwrap()
                .galley
                .job
                .sections[0]
                .format
                .color
        };
        assert_ne!(color(&output), color(&animated));
        assert_ne!(color(&animated), color(&finished));
        // Reopening is available on the same stable control after animation.
        preview_frame(&ctx, &updated, 0.6, pointer(pos, true));
        let (outcome, _) = preview_frame(&ctx, &updated, 0.65, pointer(pos, false));
        assert_eq!(outcome.task_toggle, Some(task_markers(&updated)[0]));
    }

    #[test]
    fn completed_tasks_start_crossed_out_on_every_line_in_both_themes() {
        for visuals in [egui::Visuals::light(), egui::Visuals::dark()] {
            let ctx = egui::Context::default();
            ctx.set_visuals(visuals);
            let body = "- [x] Finished task with **formatted text** and a long label that wraps across several lines\n  - [ ] Independent child\n";
            let (_, output) = preview_frame(&ctx, body, 0.0, vec![]);
            let texts = painted_text(&output);
            let finished = texts
                .iter()
                .find(|t| t.galley.job.text.starts_with("Finished"))
                .unwrap();
            let child = texts
                .iter()
                .find(|t| t.galley.job.text == "Independent child")
                .unwrap();
            assert!(finished.galley.rows.len() > 1);
            assert_ne!(
                finished.galley.job.sections[0].format.color,
                child.galley.job.sections[0].format.color
            );
            let strike_count = output
                .shapes
                .iter()
                .filter(|s| {
                    matches!(s.shape,
                egui::Shape::LineSegment { stroke, .. } if stroke.width == 1.2)
                })
                .count();
            assert_eq!(strike_count, finished.galley.rows.len());
            let (_, later) = preview_frame(&ctx, body, 0.4, vec![]);
            let later_texts = painted_text(&later);
            let later_finished = later_texts
                .iter()
                .find(|t| t.galley.job.text.starts_with("Finished"))
                .unwrap();
            assert_eq!(finished.galley.job, later_finished.galley.job);
        }
    }

    #[test]
    fn task_links_remain_clickable_without_toggling_the_checkbox() {
        let ctx = egui::Context::default();
        let body = "- [x] [Go](https://example.com)\n";
        let (_, output) = preview_frame(&ctx, body, 0.0, vec![]);
        let texts = painted_text(&output);
        let label = texts.iter().find(|t| t.galley.job.text == "Go").unwrap();
        let pos = label.pos + egui::vec2(8.0, 8.0);
        preview_frame(&ctx, body, 0.1, pointer(pos, true));
        let (outcome, _) = preview_frame(&ctx, body, 0.15, pointer(pos, false));
        assert_eq!(
            outcome.link,
            Some(LinkTarget::External("https://example.com".into()))
        );
        assert!(outcome.task_toggle.is_none());
    }

    #[test]
    fn preview_image_dimensions_and_mouse_resize_use_original_source_offsets() {
        let root = tempfile::tempdir().unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let path = root.path().join("image.png");
        image::RgbaImage::from_pixel(200, 100, image::Rgba([255, 0, 0, 255])).save(&path).unwrap();
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        ctx.include_bytes(media::path_to_file_uri(&path), std::fs::read(&path).unwrap());
        let body = "文 :smile: [[Other|link]]\n\n![caption|80x60](image.png)\n\n![[image.png|200x100]]\n\n![width|120](image.png)\n";
        let mut resolver = media::ImageResolver::default();
        let mut frame = |time: f64, events: Vec<egui::Event>| {
            let mut result = None;
            let mut out = ctx.run_ui(egui::RawInput {
                time: Some(time), events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 800.0))),
                ..Default::default()
            }, |ui| {
                result = Some(show_preview(ui, body, "Note.md", &canonical_root, &VaultIndex::new(), &[],
                    Color32::WHITE, Color32::GRAY, Color32::WHITE, Color32::GRAY,
                    16.0, None, None, &[Color32::WHITE; 6], &mut resolver));
            });
            fn image_rects(shape: &egui::Shape, rects: &mut Vec<egui::Rect>) {
                match shape {
                    egui::Shape::Mesh(mesh) => rects.push(mesh.calc_bounds()),
                    egui::Shape::Rect(rect) if rect.brush.is_some() => rects.push(rect.rect),
                    egui::Shape::Vec(shapes) => for shape in shapes { image_rects(shape, rects); },
                    _ => {}
                }
            }
            let mut rects = Vec::new();
            for shape in &out.shapes { image_rects(&shape.shape, &mut rects); }
            out.textures_delta.clear();
            (result.unwrap(), rects)
        };
        let (_, mut rects) = frame(0.0, vec![]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while rects.len() != 3 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            rects = frame(0.05, vec![]).1;
        }
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].size(), egui::vec2(80.0, 60.0));
        assert_eq!(rects[1].size(), egui::vec2(200.0, 100.0));
        assert_eq!(rects[2].size(), egui::vec2(120.0, 60.0));
        let start = rects[1].right_bottom() - egui::Vec2::splat(5.0);
        let end = start + egui::vec2(100.0, 50.0);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(0.1, vec![egui::Event::PointerMoved(start), button(start, true)]);
        assert!(frame(0.2, vec![egui::Event::PointerMoved(end)]).0.image_resize.is_none());
        let (_, live) = frame(0.3, vec![]);
        assert_eq!(live[1].size(), egui::vec2(300.0, 150.0));
        let (outcome, _) = frame(0.4, vec![button(end, false)]);
        assert!(outcome.link.is_none());
        let edit = outcome.image_resize.unwrap();
        let (range, replacement) = edit.replacement(body).unwrap();
        let mut updated = body.to_owned();
        updated.replace_range(range, &replacement);
        assert_eq!(updated, body.replace("|200x100", "|300x150"));
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
    fn inject_vblank_markers_scales_extra_blank_lines() {
        let one = inject_extra_vblank_markers("a\n\nb");
        assert_eq!(one, "a\n\nb");
        assert!(parse_vblank_mark("<!--celestine-vblank:1-->").is_none());

        let three = inject_extra_vblank_markers("a\n\n\n\nb");
        assert!(three.contains("<!--celestine-vblank:3-->"), "{three}");
        assert_eq!(parse_vblank_mark("<!--celestine-vblank:3-->"), Some(3));

        // Marker must surface as a Markdown HTML event (not swallowed).
        let mut saw = false;
        for ev in Parser::new_ext(&three, Options::empty()) {
            if let Event::Html(html) | Event::InlineHtml(html) = ev {
                if parse_vblank_mark(&html) == Some(3) {
                    saw = true;
                }
            }
        }
        assert!(saw, "expected vblank HTML event in {three}");

        // Fenced code keeps internal blank runs untouched.
        let fenced = inject_extra_vblank_markers("a\n\n```\nx\n\n\n\ny\n```\n\nb");
        assert!(fenced.contains("x\n\n\n\ny"), "{fenced}");
        assert!(!fenced.contains("celestine-vblank"), "{fenced}");
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
