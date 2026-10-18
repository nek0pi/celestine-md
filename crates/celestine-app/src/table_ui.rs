//! Theme-matched Markdown table chrome (preview + inline editor).

use crate::md_table::MdTable;
use crate::theme_apply::mix;
use egui::{
    Color32, CornerRadius, FontId, Frame, Margin, RichText, Sense, Stroke, TextEdit, Ui, Vec2,
};

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

    let cell_pad = Vec2::new(10.0, 6.0);
    let widths = measure_widths(&grid, style.font_size, cell_pad, false);
    let row_h = (style.font_size + cell_pad.y * 2.0).max(22.0);

    Frame::new()
        .fill(style.content_bg)
        .stroke(Stroke::new(1.0, style.border))
        .corner_radius(CornerRadius::same(2))
        .inner_margin(Margin::ZERO)
        .show(ui, |ui| {
            let total_w: f32 = widths.iter().sum();
            let total_h = row_h * grid.len() as f32;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());
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
                    child.label(rt);
                    x += w;
                }
            }

            paint_grid_lines(ui, rect, &widths, grid.len(), style.border, row_h);
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

/// Interactive table for edit mode. Returns `true` if the model changed.
pub fn show_editable(ui: &mut Ui, table: &mut MdTable, style: TableStyle, id: egui::Id) -> bool {
    table.ensure_shape();
    let mut cell_changed = false;
    let mut struct_op: Option<StructOp> = None;

    let mut grid: Vec<Vec<String>> = Vec::with_capacity(1 + table.rows.len());
    grid.push(table.headers.clone());
    grid.extend(table.rows.iter().cloned());

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

        Frame::new()
            .fill(style.content_bg)
            .stroke(Stroke::new(1.0, style.border))
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
                        if child.add(te).changed() {
                            cell_changed = true;
                        }
                        x += w;
                    }
                }

                paint_grid_lines(ui, rect, &widths, n_rows, style.border, row_h);
            });

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
    cell_changed || struct_op.is_some()
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
