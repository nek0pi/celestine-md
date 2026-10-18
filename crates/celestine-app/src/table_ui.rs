//! Theme-matched Markdown table chrome (preview + inline editor).

use crate::md_table::{self, MdTable};
use crate::theme_apply::mix;
use egui::{
    Color32, CornerRadius, FontId, Frame, Key, Label, Margin, Modifiers, RichText, Sense, Stroke,
    TextEdit, Ui, Vec2,
};

/// Result of drawing an editable table for one frame.
pub struct TableEditOutcome {
    pub changed: bool,
    /// Clipboard held a whole table while a cell was focused — insert this
    /// markdown *after* the current table instead of flattening into the cell.
    pub paste_table: Option<String>,
    /// User asked to remove this table from the note entirely.
    pub delete_table: bool,
}

#[derive(Clone, Copy)]
pub struct TableStyle {
    pub border: Color32,
    pub accent: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub content_bg: Color32,
    pub panel_bg: Color32,
    pub font_size: f32,
}

impl TableStyle {
    fn row_base(self) -> Color32 {
        mix(self.content_bg, self.panel_bg, 0.35)
    }

    fn row_alt(self) -> Color32 {
        mix(self.content_bg, self.panel_bg, 0.55)
    }

    fn row_header(self) -> Color32 {
        mix(self.row_base(), self.accent, 0.14)
    }
}

/// Read-only table (preview). `rows[0]` is the header when non-empty.
///
/// Click the table to select it (accent border); then Mod+C copies Markdown.
/// Right-click for Copy as Markdown / TSV. Individual cells stay selectable.
pub fn show_readonly(ui: &mut Ui, rows: &[Vec<String>], style: TableStyle) {
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let grid: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut c = r.clone();
            c.resize(cols, String::new());
            c
        })
        .collect();

    let markdown = md_table::rows_to_markdown(&grid);
    let tsv = md_table::rows_to_tsv(&grid);
    let table_id = ui.make_persistent_id(("table_ro", md5ish(&markdown)));

    let cell_pad = Vec2::new(10.0, 6.0);
    let widths = measure_widths(&grid, style.font_size, cell_pad, false);
    let row_h = (style.font_size + cell_pad.y * 2.0).max(22.0);

    ui.horizontal(|ui| {
        let selected = ui.memory(|m| m.data.get_temp::<bool>(table_id).unwrap_or(false));
        let stroke = if selected {
            Stroke::new(1.5, style.accent)
        } else {
            Stroke::new(1.0, style.border)
        };

        let response = Frame::new()
            .fill(style.content_bg)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(2))
            .inner_margin(Margin::ZERO)
            .show(ui, |ui| {
                let total_w: f32 = widths.iter().sum();
                let total_h = row_h * grid.len() as f32;
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::click());
                paint_row_fills(ui, rect, &widths, grid.len(), style, row_h);

                for (ri, row) in grid.iter().enumerate() {
                    let y = rect.min.y + ri as f32 * row_h;
                    let mut x = rect.min.x;
                    for (ci, cell) in row.iter().enumerate() {
                        let w = widths[ci];
                        let cell_rect =
                            egui::Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, row_h));
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(cell_rect.shrink2(cell_pad))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        let rt = if ri == 0 {
                            RichText::new(cell)
                                .strong()
                                .size(style.font_size)
                                .color(style.accent)
                        } else {
                            RichText::new(cell)
                                .size(style.font_size)
                                .color(style.text)
                        };
                        // Selectable so a single cell can be copied; whole-table
                        // copy uses the frame selection / context menu below.
                        child.add(Label::new(rt).selectable(true));
                        x += w;
                    }
                }

                paint_grid_lines(ui, rect, &widths, grid.len(), style.border, row_h);
                response
            })
            .inner;

        let _ = handle_table_selection(
            ui,
            table_id,
            &response,
            &markdown,
            &tsv,
            style,
            false,
            false,
        );
    });
    ui.add_space(8.0);
}

#[derive(Clone, Copy)]
enum StructOp {
    AddCol,
    AddRow,
    DelLastRow,
    DelLastCol,
}

/// Interactive table for edit mode.
///
/// Cells use TextEdit (in-cell select/copy). Click the frame / use the copy
/// control / right-click to copy the whole table as Markdown or TSV.
/// Pasting a whole table while a cell is focused is redirected (see
/// [`TableEditOutcome::paste_table`]) so newlines aren't flattened away.
pub fn show_editable(
    ui: &mut Ui,
    table: &mut MdTable,
    style: TableStyle,
    id: egui::Id,
) -> TableEditOutcome {
    table.ensure_shape();
    let mut cell_changed = false;
    let mut struct_op: Option<StructOp> = None;
    let mut paste_table: Option<String> = None;
    let mut delete_table = false;

    let mut grid: Vec<Vec<String>> = Vec::with_capacity(1 + table.rows.len());
    grid.push(table.headers.clone());
    grid.extend(table.rows.iter().cloned());

    let markdown = md_table::serialize(table);
    let tsv = md_table::rows_to_tsv(&grid);
    let table_id = id.with("table_sel");
    let n_cols = table.col_count();
    let n_grid_rows = grid.len();

    // If a cell has focus and the clipboard paste is a whole table, don't let
    // single-line TextEdit turn newlines into spaces (that was the broken paste).
    let cell_focused = (0..n_grid_rows).any(|ri| {
        (0..n_cols).any(|ci| ui.memory(|m| m.has_focus(id.with(("cell", ri, ci)))))
    });
    if cell_focused {
        ui.ctx().input_mut(|i| {
            i.events.retain(|ev| match ev {
                egui::Event::Paste(s) => {
                    if let Some(norm) = md_table::normalize_table_paste(s) {
                        paste_table = Some(norm);
                        false
                    } else {
                        true
                    }
                }
                _ => true,
            });
        });
    }

    let cell_pad = Vec2::new(8.0, 4.0);
    let widths = measure_widths(&grid, style.font_size, cell_pad, true);
    let row_h = (style.font_size + cell_pad.y * 2.0 + 6.0).max(26.0);
    let btn_w = 22.0;
    let can_del_row = !table.rows.is_empty();
    let can_del_col = table.col_count() > 1;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);

        let total_w: f32 = widths.iter().sum();
        let n_rows = grid.len();
        let total_h = row_h * n_rows as f32;
        let selected = ui.memory(|m| m.data.get_temp::<bool>(table_id).unwrap_or(false));
        let stroke = if selected {
            Stroke::new(1.5, style.accent)
        } else {
            Stroke::new(1.0, style.border)
        };

        // Background uses hover so cell TextEdits keep click/drag priority.
        let frame_resp = Frame::new()
            .fill(style.content_bg)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(2))
            .inner_margin(Margin::ZERO)
            .show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());
                paint_row_fills(ui, rect, &widths, n_rows, style, row_h);

                for (ri, row) in grid.iter_mut().enumerate() {
                    let y = rect.min.y + ri as f32 * row_h;
                    let mut x = rect.min.x;
                    for (ci, cell) in row.iter_mut().enumerate() {
                        let w = widths[ci];
                        let cell_rect =
                            egui::Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, row_h));
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(cell_rect.shrink2(Vec2::new(4.0, 2.0)))
                                .id_salt(("cell", ri, ci))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        let te = TextEdit::singleline(cell)
                            .id(id.with(("cell", ri, ci)))
                            .font(FontId::monospace(style.font_size))
                            .frame(egui::Frame::NONE)
                            .desired_width((w - 10.0).max(20.0))
                            .text_color(if ri == 0 { style.accent } else { style.text });
                        let cell_resp = child.add(te);
                        if cell_resp.changed() {
                            cell_changed = true;
                        }
                        // Focusing a cell clears whole-table selection so Mod+C
                        // copies the cell selection instead of the whole table.
                        if cell_resp.gained_focus() {
                            ui.memory_mut(|m| m.data.insert_temp(table_id, false));
                        }
                        x += w;
                    }
                }

                paint_grid_lines(ui, rect, &widths, n_rows, style.border, row_h);
                // Click empty chrome / right-click anywhere on the grid for select/copy.
                ui.interact(rect, id.with("table_bg"), Sense::click())
            })
            .inner;

        if handle_table_selection(
            ui,
            table_id,
            &frame_resp,
            &markdown,
            &tsv,
            style,
            true,
            true,
        ) {
            delete_table = true;
        }

        ui.vertical(|ui| {
            ui.add_space((total_h - btn_w).max(0.0) * 0.5);
            if ui
                .add_sized(
                    Vec2::splat(btn_w),
                    egui::Button::new(RichText::new("+").size(14.0).color(style.muted)).frame(false),
                )
                .on_hover_text("Add column")
                .clicked()
            {
                struct_op = Some(StructOp::AddCol);
            }
        });
    });

    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(RichText::new("+ row").size(12.0).color(style.muted))
                    .frame(false),
            )
            .on_hover_text("Add row")
            .clicked()
        {
            struct_op = Some(StructOp::AddRow);
        }

        if can_del_row {
            ui.add_space(8.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("− row").size(12.0).color(style.muted))
                        .frame(false),
                )
                .on_hover_text("Delete last row")
                .clicked()
            {
                struct_op = Some(StructOp::DelLastRow);
            }
        }

        if can_del_col {
            ui.add_space(8.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("− col").size(12.0).color(style.muted))
                        .frame(false),
                )
                .on_hover_text("Delete last column")
                .clicked()
            {
                struct_op = Some(StructOp::DelLastCol);
            }
        }

        ui.add_space(12.0);
        if copy_button(ui, style).clicked() {
            ui.ctx().copy_text(markdown.clone());
            ui.memory_mut(|m| m.data.insert_temp(table_id, true));
        }

        ui.add_space(8.0);
        if ui
            .add(
                egui::Button::new(RichText::new("delete").size(12.0).color(style.muted))
                    .frame(false),
            )
            .on_hover_text("Delete this table from the note")
            .clicked()
        {
            delete_table = true;
        }
    });

    if cell_changed || struct_op.is_some() {
        apply_grid(table, &grid);
    }
    match struct_op {
        Some(StructOp::AddCol) => table.add_column(),
        Some(StructOp::AddRow) => table.add_row(),
        Some(StructOp::DelLastRow) => {
            if !table.rows.is_empty() {
                let last = table.rows.len() - 1;
                table.remove_row(last);
            }
        }
        Some(StructOp::DelLastCol) => {
            if table.col_count() > 1 {
                let last = table.col_count() - 1;
                table.remove_column(last);
            }
        }
        None => {}
    }

    ui.add_space(6.0);
    TableEditOutcome {
        changed: cell_changed || struct_op.is_some(),
        paste_table,
        delete_table,
    }
}

/// Returns `true` when the user chose Delete table from the context menu.
fn handle_table_selection(
    ui: &mut Ui,
    table_id: egui::Id,
    response: &egui::Response,
    markdown: &str,
    tsv: &str,
    style: TableStyle,
    editing_cells: bool,
    allow_delete: bool,
) -> bool {
    let mut selected = ui.memory(|m| m.data.get_temp::<bool>(table_id).unwrap_or(false));
    let mut delete_clicked = false;

    if response.clicked() {
        selected = true;
        ui.memory_mut(|m| m.data.insert_temp(table_id, true));
    }
    if response.clicked_elsewhere() {
        // Don't clear while the user is interacting with a cell TextEdit inside
        // this table — those clicks are "elsewhere" relative to the frame Sense.
        if !(editing_cells && ui.ctx().egui_wants_keyboard_input()) {
            selected = false;
            ui.memory_mut(|m| m.data.insert_temp(table_id, false));
        }
    }

    response.context_menu(|ui| {
        if ui
            .button(RichText::new("Copy as Markdown").color(style.text))
            .clicked()
        {
            ui.ctx().copy_text(markdown.to_string());
            ui.close();
        }
        if ui
            .button(RichText::new("Copy as TSV").color(style.text))
            .clicked()
        {
            ui.ctx().copy_text(tsv.to_string());
            ui.close();
        }
        if ui
            .button(RichText::new("Copy as plain text").color(style.text))
            .clicked()
        {
            ui.ctx().copy_text(tsv.replace('\t', " "));
            ui.close();
        }
        if allow_delete {
            ui.separator();
            if ui
                .button(RichText::new("Delete table").color(style.text))
                .clicked()
            {
                delete_clicked = true;
                ui.close();
            }
        }
    });

    // Hover affordance: copy control next to the table in preview.
    if !editing_cells && (response.hovered() || selected) {
        if copy_button(ui, style).clicked() {
            ui.ctx().copy_text(markdown.to_string());
            selected = true;
            ui.memory_mut(|m| m.data.insert_temp(table_id, true));
        }
    }

    // Whole-table Mod+C when selected and no text field is eating keyboard input.
    // (In edit mode, a focused cell TextEdit sets egui_wants_keyboard_input.)
    if selected
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::C))
    {
        ui.ctx().copy_text(markdown.to_string());
    }

    if selected {
        response.clone().on_hover_text("Selected · Mod+C copies Markdown · right-click for more");
    } else {
        response
            .clone()
            .on_hover_text("Click to select · right-click to copy · cells are selectable");
    }

    delete_clicked
}

fn copy_button(ui: &mut Ui, style: TableStyle) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new("copy").size(12.0).color(style.muted)).frame(false),
    )
    .on_hover_text("Copy table as Markdown")
}

/// Tiny stable id salt from table contents (not cryptographic).
fn md5ish(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn apply_grid(table: &mut MdTable, grid: &[Vec<String>]) {
    if let Some(h) = grid.first() {
        table.headers = h.clone();
    }
    table.rows = grid.iter().skip(1).cloned().collect();
    table.ensure_shape();
}

fn measure_widths(
    grid: &[Vec<String>],
    font_size: f32,
    cell_pad: Vec2,
    editable: bool,
) -> Vec<f32> {
    let cols = grid.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let mut widths = vec![font_size * 4.0; cols];
    let em = if editable { 0.62 } else { 0.55 };
    for row in grid {
        for (ci, cell) in row.iter().enumerate().take(cols) {
            let w = cell.chars().count() as f32 * font_size * em
                + cell_pad.x * 2.0
                + if editable { 12.0 } else { 0.0 };
            widths[ci] = widths[ci].max(w).max(font_size * 3.0);
        }
    }
    for w in &mut widths {
        *w = w.min(280.0);
    }
    widths
}

fn paint_row_fills(
    ui: &Ui,
    rect: egui::Rect,
    widths: &[f32],
    n_rows: usize,
    style: TableStyle,
    row_h: f32,
) {
    let total_w: f32 = widths.iter().sum();
    for ri in 0..n_rows {
        let fill = if ri == 0 {
            style.row_header()
        } else if ri % 2 == 0 {
            style.row_base()
        } else {
            style.row_alt()
        };
        let y = rect.min.y + ri as f32 * row_h;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(egui::pos2(rect.min.x, y), Vec2::new(total_w, row_h)),
            0.0,
            fill,
        );
    }
}

fn paint_grid_lines(
    ui: &Ui,
    rect: egui::Rect,
    widths: &[f32],
    n_rows: usize,
    border: Color32,
    row_h: f32,
) {
    let stroke = Stroke::new(1.0, border);
    let painter = ui.painter();
    let total_w: f32 = widths.iter().sum();
    let total_h = row_h * n_rows as f32;

    for ri in 1..n_rows {
        let y = rect.min.y + ri as f32 * row_h;
        painter.hline(rect.min.x..=rect.min.x + total_w, y, stroke);
    }
    let mut x = rect.min.x;
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            painter.vline(x, rect.min.y..=rect.min.y + total_h, stroke);
        }
        x += *w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_and_tsv_round_trip_shapes() {
        let rows = vec![
            vec!["A".into(), "B".into()],
            vec!["1".into(), "2".into()],
        ];
        let md = md_table::rows_to_markdown(&rows);
        assert!(md.contains("| A "), "{md}");
        assert!(md.contains("| 1 "), "{md}");
        let tsv = md_table::rows_to_tsv(&rows);
        assert_eq!(tsv, "A\tB\n1\t2\n");
    }
}
