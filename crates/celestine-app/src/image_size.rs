//! Image dimensions stored in Markdown alt text or an Obsidian embed suffix.
use egui::{Id, Rect, Sense, Ui, Vec2};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageSize {
    pub width: u32,
    pub height: Option<u32>,
}

impl ImageSize {
    pub fn parse(text: &str) -> Option<Self> {
        let dimension = |s: &str| {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse::<u32>().ok().filter(|n| (1..=16384).contains(n))
        };
        let (width, height) = match text.trim().split_once('x') {
            Some((w, h)) => (dimension(w)?, Some(dimension(h)?)),
            None => (dimension(text.trim())?, None),
        };
        Some(Self { width, height })
    }

    fn suffix(self) -> String {
        match self.height {
            Some(h) => format!("|{}x{h}", self.width),
            None => format!("|{}", self.width),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImageEmbed {
    pub range: Range<usize>,
    pub destination: String,
    pub size: Option<ImageSize>,
    suffix: Range<usize>,
    original: String,
}

#[derive(Clone, Debug)]
pub struct ResizeEdit {
    pub embed: ImageEmbed,
    pub size: Option<ImageSize>,
}

impl ResizeEdit {
    pub fn shifted(mut self, offset: isize) -> Option<Self> {
        self.embed.range = self.embed.range.start.checked_add_signed(offset)?
            ..self.embed.range.end.checked_add_signed(offset)?;
        self.embed.suffix = self.embed.suffix.start.checked_add_signed(offset)?
            ..self.embed.suffix.end.checked_add_signed(offset)?;
        Some(self)
    }

    /// Refuse stale offsets instead of changing another occurrence of the image.
    pub fn replacement(&self, text: &str) -> Option<(Range<usize>, String)> {
        if text.get(self.embed.range.clone())? != self.embed.original {
            return None;
        }
        let replacement = self.size.map(ImageSize::suffix).unwrap_or_default();
        if text.get(self.embed.suffix.clone())? == replacement {
            return None;
        }
        Some((self.embed.suffix.clone(), replacement))
    }
}

fn embed(text: &str, range: Range<usize>, label: Range<usize>, destination: String) -> ImageEmbed {
    let (suffix, size) = text[label.clone()]
        .rfind('|')
        .and_then(|pipe| {
            if text[label.start..label.start + pipe]
                .bytes()
                .rev()
                .take_while(|b| *b == b'\\')
                .count()
                % 2
                == 1
            {
                return None;
            }
            let size = ImageSize::parse(&text[label.start + pipe + 1..label.end])?;
            Some((label.start + pipe..label.end, size))
        })
        .map_or((label.end..label.end, None), |(r, s)| (r, Some(s)));
    ImageEmbed {
        original: text[range.clone()].to_owned(),
        range,
        destination,
        size,
        suffix,
    }
}

/// Source offsets always refer to the unmodified Markdown, before preview preprocessing.
pub fn embeds(text: &str) -> Vec<ImageEmbed> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut images = Vec::new();
    let mut excluded = Vec::new();
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_))
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_) => excluded.push(range),
            Event::Start(Tag::Image { dest_url, .. }) => {
                // Find the matching alt-text bracket, respecting nested brackets and escapes.
                let mut depth = 0;
                let mut escaped = false;
                let mut close = None;
                for (i, c) in text[range.start + 2..range.end].char_indices() {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    match c {
                        '\\' => escaped = true,
                        '[' => depth += 1,
                        ']' if depth > 0 => depth -= 1,
                        ']' => {
                            close = Some(range.start + 2 + i);
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(close) = close {
                    let label = range.start + 2..close;
                    excluded.push(range.clone());
                    images.push(embed(text, range, label, dest_url.into_string()));
                }
            }
            _ => {}
        }
    }
    static WIKI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    for capture in WIKI
        .get_or_init(|| regex::Regex::new(r"!\[\[([^\[\]\r\n]+)\]\]").unwrap())
        .captures_iter(text)
    {
        let full = capture.get(0).unwrap();
        if excluded
            .iter()
            .any(|r| r.start <= full.start() && full.start() < r.end)
        {
            continue;
        }
        let inner = capture.get(1).unwrap();
        let target = inner.as_str().split('|').next().unwrap().trim();
        if !crate::media::is_image_target(target) {
            continue;
        }
        let label = inner.start() + target.len()..inner.end();
        images.push(embed(
            text,
            full.range(),
            label,
            format!("wikilink:{target}"),
        ));
    }
    images.sort_by_key(|image| image.range.start);
    images
}

#[derive(Clone)]
struct DragSize {
    origin: egui::Pos2,
    start: Vec2,
    current: Vec2,
}

pub fn fitted_size(ui: &Ui, id: Id, uri: &str, size: Option<ImageSize>, width: f32) -> Vec2 {
    if let Some(drag) = ui.ctx().data(|d| d.get_temp::<DragSize>(id)) {
        return drag.current;
    }
    let natural = egui::Image::new(uri)
        .load_for_size(ui.ctx(), Vec2::new(width, 600.0))
        .ok()
        .and_then(|p| p.size())
        .filter(|s| s.x > 0.0 && s.y > 0.0)
        .unwrap_or(Vec2::new(width.min(240.0), 100.0));
    let desired = match size {
        Some(s) => Vec2::new(
            s.width as f32,
            s.height
                .map_or(s.width as f32 * natural.y / natural.x, |h| h as f32),
        ),
        None => natural,
    };
    let scale = (width.max(1.0) / desired.x).min(1.0);
    let scale = if size.is_none() {
        scale.min(600.0 / desired.y)
    } else {
        scale
    };
    desired * scale
}

/// A corner grip shared by preview and the source editor. The note changes only
/// on release, so a drag is one undoable edit rather than dozens of writes.
pub fn resize_handle(
    ui: &Ui,
    id: Id,
    rect: Rect,
    width: f32,
    image: &ImageEmbed,
) -> Option<ResizeEdit> {
    let grip = Rect::from_min_size(rect.right_bottom() - Vec2::splat(16.0), Vec2::splat(20.0));
    let response = ui
        .interact(grip, id.with("grip"), Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeNwSe)
        .on_hover_text("Drag to resize · double-click for automatic size");
    if response.drag_started() {
        let origin = ui
            .input(|i| i.pointer.press_origin())
            .unwrap_or(rect.right_bottom());
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                id,
                DragSize {
                    origin,
                    start: rect.size(),
                    current: rect.size(),
                },
            )
        });
    }
    if response.dragged() || response.drag_stopped() {
        if let Some(mut drag) = ui.ctx().data(|d| d.get_temp::<DragSize>(id)) {
            if let Some(pointer) = ui.input(|i| i.pointer.latest_pos()) {
                let delta = pointer - drag.origin;
                let ratio = drag.start.y / drag.start.x;
                let w = (drag.start.x + (delta.x + delta.y * ratio) / (1.0 + ratio * ratio))
                    .clamp(32.0_f32.min(width), width.max(1.0));
                drag.current = Vec2::new(w, w * ratio);
            }
            ui.ctx().request_repaint();
            ui.ctx().data_mut(|d| d.insert_temp(id, drag.clone()));
            if response.drag_stopped() {
                ui.ctx().data_mut(|d| d.remove::<DragSize>(id));
                if (drag.current.x - drag.start.x).abs() >= 1.0 {
                    return Some(ResizeEdit {
                        embed: image.clone(),
                        size: Some(ImageSize {
                            width: drag.current.x.round().max(1.0) as u32,
                            height: image
                                .size
                                .and_then(|s| s.height)
                                .map(|_| drag.current.y.round().clamp(1.0, 16384.0) as u32),
                        }),
                    });
                }
            }
        }
    }
    if response.hovered() || response.dragged() || ui.rect_contains_pointer(rect) {
        let color = ui.visuals().selection.stroke.color;
        ui.painter().rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Inside,
        );
        ui.painter().rect_filled(
            Rect::from_center_size(rect.right_bottom() - Vec2::splat(5.0), Vec2::splat(9.0)),
            2.0,
            color,
        );
    }
    if response.double_clicked() && image.size.is_some() {
        return Some(ResizeEdit {
            embed: image.clone(),
            size: None,
        });
    }
    None
}

/// Preserve Unicode cursor positions and force one undo step for the entire drag.
pub fn apply_in_editor(
    text: &mut String,
    edit: &ResizeEdit,
    state: &mut egui::text_edit::TextEditState,
) -> bool {
    let Some((range, replacement)) = edit.replacement(text) else {
        return false;
    };
    let mut cursor = state.cursor.char_range().unwrap_or_default();
    let mut undo = state.undoer();
    undo.add_undo(&(cursor, text.clone()));
    let start = text[..range.start].chars().count();
    let end = text[..range.end].chars().count();
    let new_len = replacement.chars().count();
    for c in [&mut cursor.primary, &mut cursor.secondary] {
        if c.index.0 >= end {
            c.index.0 = c.index.0 - (end - start) + new_len;
        } else if c.index.0 > start {
            c.index.0 = start + new_len;
        }
    }
    text.replace_range(range, &replacement);
    state.cursor.set_char_range(Some(cursor));
    // A resize after undo starts a new edit branch and invalidates old redos.
    undo.feed_state(0.0, &(cursor, text.clone()));
    undo.add_undo(&(cursor, text.clone()));
    state.set_undoer(undo);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: u32, h: Option<u32>) -> Option<ImageSize> {
        Some(ImageSize {
            width: w,
            height: h,
        })
    }

    #[test]
    fn parses_dimensions_and_ignores_malformed_sizes() {
        assert_eq!(ImageSize::parse("320"), size(320, None));
        assert_eq!(ImageSize::parse("320x180"), size(320, Some(180)));
        for invalid in [
            "",
            "0",
            "-2",
            "10x0",
            "auto",
            "100%",
            "100x",
            "12x4x2",
            "999999999999999999",
            "16385",
        ] {
            assert_eq!(ImageSize::parse(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn scans_markdown_and_wiki_images_without_touching_code() {
        let text = "文 ![caption|320](<a b.png> \"title\")\r\n![[a.png|180x90]]\r\n![[a.png|alias|200]]\r\n![escaped\\|400](a.png)\r\n\r\n```md\r\n![code|100](a.png)\r\n![[code.png|100]]\r\n```\r\n\r\n    ![[indented.png|100]]\r\n\r\n`![[inline.png|100]]`\r\n";
        let images = embeds(text);
        assert_eq!(images.len(), 4);
        assert_eq!(images[0].size, size(320, None));
        assert_eq!(images[0].destination, "a b.png");
        assert_eq!(images[1].size, size(180, Some(90)));
        assert_eq!(images[2].size, size(200, None));
        assert_eq!(images[3].size, None);
        for image in images {
            assert_eq!(&text[image.range.clone()], image.original);
        }
    }

    #[test]
    fn resize_edits_only_the_chosen_occurrence_and_preserves_markup() {
        for (text, expected) in [
            (
                "![a [nested] caption](<space image.png> \"keep\")",
                "![a [nested] caption|240](<space image.png> \"keep\")",
            ),
            ("![caption|100x50](image.png)", "![caption|240](image.png)"),
            ("![[image.png|alias]]", "![[image.png|alias|240]]"),
            ("![[image.png|120]]", "![[image.png|240]]"),
            (
                "![caption][ref]\n\n[ref]: image.png \"title\"",
                "![caption|240][ref]\n\n[ref]: image.png \"title\"",
            ),
        ] {
            let edit = ResizeEdit {
                embed: embeds(text).remove(0),
                size: size(240, None),
            };
            let (range, replacement) = edit.replacement(text).unwrap();
            let mut changed = text.to_owned();
            changed.replace_range(range, &replacement);
            assert_eq!(changed, expected);
            assert!(edit.replacement(&format!("prefix{text}")).is_none());
        }
        let text = "![](a.png)\n![](a.png)\n";
        let edit = ResizeEdit {
            embed: embeds(text).remove(1),
            size: size(123, Some(45)),
        };
        let mut changed = text.to_owned();
        let (range, replacement) = edit.replacement(text).unwrap();
        changed.replace_range(range, &replacement);
        assert_eq!(changed, "![](a.png)\n![|123x45](a.png)\n");
        let reset = ResizeEdit {
            embed: embeds(&changed).remove(1),
            size: None,
        };
        let (range, replacement) = reset.replacement(&changed).unwrap();
        changed.replace_range(range, &replacement);
        assert_eq!(changed, text);
    }

    #[test]
    fn source_resizing_is_one_undo_step_and_keeps_unicode_cursors() {
        let original = "文 ![é|100](image.png) ω";
        let mut text = original.to_owned();
        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(text.chars().count()),
            )));
        let edit = ResizeEdit {
            embed: embeds(&text).remove(0),
            size: size(320, Some(180)),
        };
        assert!(apply_in_editor(&mut text, &edit, &mut state));
        assert_eq!(text, "文 ![é|320x180](image.png) ω");
        let cursor = state.cursor.char_range().unwrap();
        assert_eq!(cursor.primary.index.0, text.chars().count());
        assert_eq!(
            state.undoer().undo(&(cursor, text.clone())).unwrap().1,
            original
        );
    }

    #[test]
    fn corner_drag_previews_live_size_and_commits_on_release() {
        let ctx = egui::Context::default();
        let image = embeds("![alt|200x100](image.png)").remove(0);
        let mut id = None;
        let mut rect = Rect::NOTHING;
        let mut frame = |time: f64, events: Vec<egui::Event>| {
            let mut edit = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let image_id = ui.scope_id().with("image-test");
                    id = Some(image_id);
                    let size = ui
                        .ctx()
                        .data(|d| d.get_temp::<DragSize>(image_id))
                        .map_or(Vec2::new(200.0, 100.0), |d| d.current);
                    rect = Rect::from_min_size(egui::pos2(20.0, 20.0), size);
                    edit = resize_handle(ui, image_id, rect, 600.0, &image);
                },
            );
            output.textures_delta.clear();
            (edit, rect)
        };
        let (_, rect) = frame(0.0, vec![]);
        let grip = rect.right_bottom() - Vec2::splat(5.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert!(frame(
            0.1,
            vec![egui::Event::PointerMoved(grip), button(grip, true)]
        )
        .0
        .is_none());
        let end = grip + Vec2::new(100.0, 50.0);
        assert!(frame(0.2, vec![egui::Event::PointerMoved(end)]).0.is_none());
        let (_, live_rect) = frame(0.3, vec![]);
        assert_eq!(live_rect.size(), Vec2::new(300.0, 150.0));
        let (edit, _) = frame(0.4, vec![button(end, false)]);
        assert_eq!(edit.unwrap().size, size(300, Some(150)));
        drop(frame);
        assert!(ctx.data(|d| d.get_temp::<DragSize>(id.unwrap())).is_none());
    }
}
