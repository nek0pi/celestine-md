//! Reserve image space between text rows without changing editor text or undo history.
use crate::image_size::{self, ImageEmbed, ResizeEdit};
use crate::media::{self, ImageResolver};
use egui::{Galley, Rect, Ui};
use std::sync::Arc;

pub struct InlineImage {
    uri: String,
    rect: Rect,
    id: egui::Id,
    embed: ImageEmbed,
    max_width: f32,
}

/// Only standalone Markdown image lines are expanded. The Markdown parser excludes
/// fenced/indented code and inline code, and supplies the decoded destination.
fn destinations(text: &str) -> Vec<(usize, ImageEmbed)> {
    image_size::embeds(text)
        .into_iter()
        .filter_map(|embed| {
            let range = &embed.range;
            let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
            let end = text[range.end..]
                .find('\n')
                .map_or(text.len(), |i| range.end + i);
            if !text[start..range.start].trim().is_empty()
                || !text[range.end..end].trim().is_empty()
            {
                return None;
            }
            Some((text[..start].bytes().filter(|b| *b == b'\n').count(), embed))
        })
        .collect()
}

pub fn layout(
    ui: &Ui,
    text: &str,
    mut galley: Arc<Galley>,
    width: f32,
    resolver: &mut ImageResolver,
    inventory: &[String],
    editor_id: egui::Id,
) -> (Arc<Galley>, Vec<InlineImage>) {
    let mut images = Vec::new();
    let mut heights = Vec::new();
    for (line, embed) in destinations(text) {
        let Some(resolved) = resolver.resolve(&embed.destination, inventory) else {
            continue;
        };
        let uri = media::path_to_file_uri(&resolved.abs);
        let id = editor_id.with(("image", embed.range.start));
        let size = image_size::fitted_size(ui, id, &uri, embed.size, width);
        heights.push((line, uri, size, id, embed));
    }
    if heights.is_empty() {
        return (galley, images);
    }
    let galley_mut = Arc::make_mut(&mut galley);
    let mut line = 0;
    let mut shift = 0.0;
    let last_row = galley_mut.rows.len().saturating_sub(1);
    for (i, row) in galley_mut.rows.iter_mut().enumerate() {
        row.pos.y += shift;
        if row.ends_with_newline || i == last_row {
            if let Some((_, uri, size, id, embed)) = heights.iter().find(|(n, ..)| *n == line) {
                images.push(InlineImage {
                    uri: uri.clone(),
                    rect: Rect::from_min_size(egui::pos2(0.0, row.rect().bottom() + 6.0), *size),
                    id: *id,
                    embed: embed.clone(),
                    max_width: width,
                });
                shift += size.y + 12.0;
            }
            line += 1;
        }
    }
    galley_mut.rect.max.y += shift;
    galley_mut.mesh_bounds.max.y += shift;
    (galley, images)
}

pub fn paint(
    ui: &Ui,
    origin: egui::Pos2,
    images: &[InlineImage],
    interactive: bool,
) -> Option<ResizeEdit> {
    let mut edit = None;
    for image in images {
        let rect = image.rect.translate(origin.to_vec2());
        if ui.is_rect_visible(rect) {
            egui::Image::new(image.uri.clone()).paint_at(ui, rect);
            if interactive {
                edit = image_size::resize_handle(ui, image.id, rect, image.max_width, &image.embed)
                    .or(edit);
            }
        }
    }
    edit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_editor_corner_drag_updates_markdown_and_undoes() {
        let dir = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(200, 100, image::Rgba([255, 0, 0, 255]))
            .save(dir.path().join("image.png"))
            .unwrap();
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let original = "before\n![alt|200x100](image.png)\nafter\n";
        let mut text = original.to_owned();
        let mut resolver = ImageResolver::default();
        resolver.set_context(&dir.path().canonicalize().unwrap(), "Note.md", "editor");
        let mut frame = |time: f64, events: Vec<egui::Event>| {
            let mut rect = Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let editor_id = ui.scope_id().with("test-source-editor");
                    let mut images = Vec::new();
                    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, width: f32| {
                        let text = text.as_str();
                        let galley = ui.fonts_mut(|f| {
                            f.layout(
                                text.into(),
                                egui::FontId::monospace(14.0),
                                egui::Color32::WHITE,
                                width,
                            )
                        });
                        let (galley, laid_out) =
                            layout(ui, text, galley, width, &mut resolver, &[], editor_id);
                        images = laid_out;
                        galley
                    };
                    let mut out = egui::TextEdit::multiline(&mut text)
                        .id(editor_id)
                        .desired_width(600.0)
                        .layouter(&mut layouter)
                        .show(ui);
                    rect = images[0].rect.translate(out.galley_pos.to_vec2());
                    if let Some(edit) = paint(ui, out.galley_pos, &images, true) {
                        assert!(image_size::apply_in_editor(
                            &mut text,
                            &edit,
                            &mut out.state
                        ));
                        out.response.request_focus();
                        out.state.store(ui.ctx(), editor_id);
                    }
                },
            );
            output.textures_delta.clear();
            (rect, text.clone())
        };
        let (rect, _) = frame(0.0, vec![]);
        let start = rect.right_bottom() - egui::Vec2::splat(5.0);
        let end = start + egui::vec2(100.0, 50.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            0.1,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        frame(0.2, vec![egui::Event::PointerMoved(end)]);
        let (live, unchanged) = frame(0.3, vec![]);
        assert_eq!(live.size(), egui::vec2(300.0, 150.0));
        assert_eq!(unchanged, original);
        let (_, resized) = frame(0.4, vec![button(end, false)]);
        assert_eq!(resized, original.replace("200x100", "300x150"));
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
                ..Default::default()
            },
            |ui| {
                egui::TextEdit::multiline(&mut text)
                    .id(ui.scope_id().with("test-source-editor"))
                    .show(ui);
            },
        );
        output.textures_delta.clear();
        assert_eq!(text, original);
    }

    #[test]
    fn only_standalone_images_outside_code_are_expanded() {
        let source = "text\n![](<images/a%20b.png>)\n\n```md\n![](code.png)\n```\n\n    ![](indented.png)\n\n`![](inline.png)`\ntext ![](embedded.png)\n";
        assert_eq!(
            destinations(source)
                .into_iter()
                .map(|(line, e)| (line, e.destination))
                .collect::<Vec<_>>(),
            vec![(1, "images/a%20b.png".into())]
        );
    }

    #[test]
    fn inline_layout_preserves_source_rows_and_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        image::RgbaImage::from_pixel(10, 20, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let text = "before\n![](image.png)\nafter\n";
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let mut resolver = ImageResolver::default();
        resolver.set_context(&dir.path().canonicalize().unwrap(), "Note.md", text);
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let plain = ui.fonts_mut(|f| {
                f.layout_no_wrap(
                    text.into(),
                    egui::FontId::monospace(14.0),
                    egui::Color32::WHITE,
                )
            });
            let (expanded, images) = layout(
                ui,
                text,
                plain.clone(),
                400.0,
                &mut resolver,
                &[],
                ui.scope_id().with("editor"),
            );
            assert_eq!(images.len(), 1);
            assert_eq!(plain.job.text, expanded.job.text);
            assert_eq!(plain.rows.len(), expanded.rows.len());
            assert_eq!(plain.rows[0].pos, expanded.rows[0].pos);
            assert!(expanded.rows[2].pos.y > plain.rows[2].pos.y);
            assert!(images[0].rect.top() >= expanded.rows[1].rect().bottom());
            assert!(images[0].rect.bottom() < expanded.rows[2].rect().top());
            assert_eq!(
                plain.rows.iter().map(|r| r.glyphs.len()).sum::<usize>(),
                expanded.rows.iter().map(|r| r.glyphs.len()).sum::<usize>()
            );
            paint(ui, egui::Pos2::ZERO, &images, false);
        });
        out.textures_delta.clear();
    }
}
