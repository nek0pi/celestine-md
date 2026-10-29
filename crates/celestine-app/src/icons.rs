//! Flea-style Lucide stroke marks (24×24 grid), shared across the whole UI.
//! Paths adapted from Flea's Icons.js (Lucide geometry, square caps / miter joins).

use egui::{pos2, Color32, Pos2, Response, Sense, Shape, Stroke, Ui, Vec2};

const GRID: f32 = 24.0;

/// Curated set used for custom folder marks (right-click) and chrome.
pub const FOLDER_ICON_CHOICES: &[&str] = &[
    "folder",
    "folder-open",
    "file-text",
    "markdown",
    "code",
    "image",
    "film",
    "music",
    "archive",
    "house",
    "star",
    "terminal",
    "lock",
    "tag",
    "globe",
    "camera",
    "download",
    "server",
    "book",
    "heart",
    "zap",
    "compass",
    "box",
    "layers",
];

fn path(name: &str) -> &'static str {
    match name {
        "folder" => "M2 20V3h6l2 3h12v14H2z",
        "folder-open" => "M2 20V3h6l2 3h12v3 M22 11l-2.5 9H2l2.5-9z",
        "file" => "M4 22V2h10l6 6v14H4z M14 2v6h6",
        "file-text" => "M4 22V2h10l6 6v14H4z M14 2v6h6 M8 9h2 M8 13h8 M8 17h8",
        "markdown" => "M2 5h20v14H2z M5.5 15V9l3 3.5 3-3.5v6 M17 9v6 M14.5 12.5 17 15l2.5-2.5",
        "code" => "M16 18l6-6-6-6 M8 6l-6 6 6 6",
        "image" => "M3 3h18v18H3z M7 9A2 2 0 1 0 11 9A2 2 0 1 0 7 9Z M21 15l-3-3L6 21",
        "film" => "M3 3h18v18H3z M7 3v18 M17 3v18 M3 12h18",
        "canvas" => "M2 3h8v6H2z M14 15h8v6h-8z M6 9v6h8",
        "music" => "M9 18V5l12-2v13 M6 16h6v6H6z M15 13h6v6h-6z",
        "archive" => "M2 3h20v5H2z M4 8v13h16V8 M10 12h4",
        "house" => "M3 21V10l9-7 9 7v11h-6v-8H9v8H3z",
        "star" => "M12 2l2.9 6.6 7.1.6-5.4 4.7 1.6 7L12 17.3l-6.2 3.6 1.6-7L2 9.2l7.1-.6z",
        "terminal" => "M4 17l6-6-6-6 M12 19h8",
        "lock" => "M4 11h16v10H4z M8 11V7a4 4 0 0 1 8 0v4",
        "tag" => "M3 3h7l11 11-7 7L3 10z M7 7h.01",
        "globe" => "M2 12h20 M12 2a10 10 0 1 0 0.01 0 M12 2v20",
        "camera" => "M14.5 4h-5L7 7H2v13h20V7h-5z M12 10a3 3 0 1 0 0.01 0",
        "download" => "M12 15V3 M7 10l5 5 5-5 M3 15v6h18v-6",
        "server" => "M2 2h20v8H2z M2 14h20v8H2z M6 6h.01 M6 18h.01",
        "book" => "M4 3h14v18H4z M6 3v18 M9 8h6 M9 12h6",
        "heart" => "M12 20L4 12l2.5-4.5h3.5L12 10l2 2.5h3.5L20 12z",
        "zap" => "M13 2L4 14h7l-1 8 9-12h-7l1-8z",
        "compass" => "M12 3a9 9 0 1 0 0.01 0 M16 8l-2.5 6.5L7 17l2.5-6.5z",
        "box" => "M21 8l-9-5-9 5v8l9 5 9-5z M3.3 7.5 12 12.5l8.7-5 M12 12.5V22",
        "layers" => "M12 2L2 7l10 5 10-5z M2 12l10 5 10-5 M2 17l10 5 10-5",
        // Two semicircles + handle (avoids Lucide's near-zero arc ε that our
        // tessellator used to treat as a chord instead of a full circle).
        "search" => "M3 11A8 8 0 1 1 19 11A8 8 0 1 1 3 11 M16.65 16.65L21 21",
        "sliders" => "M4 21v-7 M4 10V3 M12 21v-9 M12 8V3 M20 21v-5 M20 12V3 M2 14h4 M10 8h4 M18 16h4",
        "settings" => "M12 9a3 3 0 1 0 0.01 0 M4.5 12h3 M16.5 12h3 M7.8 7.8l2.1 2.1 M14.1 14.1l2.1 2.1 M7.8 16.2l2.1-2.1 M14.1 9.9l2.1-2.1",
        "panel-left" => "M3 3h18v18H3z M9 3v18",
        "panel-right" => "M3 3h18v18H3z M15 3v18",
        "chevron-right" => "M9 18l6-6-6-6",
        "chevron-left" => "M15 18l-6-6 6-6",
        "chevron-down" => "M6 9l6 6 6-6",
        "chevron-up" => "M18 15l-6-6-6 6",
        "x" => "M6 6l12 12 M18 6 6 18",
        "plus" => "M5 12h14 M12 5v14",
        "check" => "M4 12l6 6L20 6",
        "trash" => "M3 6h18 M8 6V3h8v3 M6 6l1.2 15h9.6L18 6",
        "graph" => "M3 3v18h18 M7 14l4-4 3 3 5-6",
        "vault" => "M3 7h18v14H3z M8 7V5h8v2 M12 13v4",
        "eye" => "M1 12s4-8 11-8 11 8 11 8-4 8-11 8S1 12 1 12z M12 9A3 3 0 1 0 12.01 9",
        "pencil" => "M12 20h9 M16.5 3.5l4 4L7 21H3v-4L16.5 3.5z",
        "git-branch" => "M6 3v12 M18 3a3 3 0 1 0 0.01 0 M6 15a3 3 0 1 0 0.01 0 M18 9a9 9 0 0 1-9 9",
        _ => "folder",
    }
}

/// Map legacy emoji assignments (and unknown strings) onto Lucide names.
pub fn normalize_icon_name(raw: &str) -> &'static str {
    match raw.trim() {
        "📁" | "📂" | "🗂" => "folder",
        "📦" => "archive",
        "📚" | "📝" => "file-text",
        "💡" | "🔥" => "zap",
        "🔬" | "🧪" | "🧭" => "compass",
        "🎨" | "📷" => "image",
        "🎵" => "music",
        "🎬" => "film",
        "💻" | "🛠" | "🎮" | "🧠" => "code",
        "🏠" => "house",
        "⭐" => "star",
        "💎" | "♥" => "heart",
        "🌱" => "layers",
        "📌" | "🔒" => "lock",
        "☁" | "🗺" => "globe",
        other => known_static(other),
    }
}

fn known_static(name: &str) -> &'static str {
    match name {
        "folder" => "folder",
        "folder-open" => "folder-open",
        "file" => "file",
        "file-text" => "file-text",
        "markdown" => "markdown",
        "code" => "code",
        "image" => "image",
        "film" => "film",
        "music" => "music",
        "archive" => "archive",
        "house" => "house",
        "star" => "star",
        "terminal" => "terminal",
        "lock" => "lock",
        "tag" => "tag",
        "globe" => "globe",
        "camera" => "camera",
        "download" => "download",
        "server" => "server",
        "book" => "book",
        "heart" => "heart",
        "zap" => "zap",
        "compass" => "compass",
        "box" => "box",
        "layers" => "layers",
        "search" => "search",
        "sliders" => "sliders",
        "settings" => "settings",
        "panel-left" => "panel-left",
        "panel-right" => "panel-right",
        "chevron-right" => "chevron-right",
        "chevron-left" => "chevron-left",
        "chevron-down" => "chevron-down",
        "chevron-up" => "chevron-up",
        "x" => "x",
        "plus" => "plus",
        "check" => "check",
        "trash" => "trash",
        "graph" => "graph",
        "vault" => "vault",
        "eye" => "eye",
        "pencil" => "pencil",
        "git-branch" => "git-branch",
        _ => "folder",
    }
}

/// Paint a named mark into `rect` (stroke only).
pub fn paint(ui: &Ui, name: &str, rect: egui::Rect, color: Color32) {
    let name = normalize_icon_name(name);
    let stroke_w = (rect.width() / GRID * 2.0).clamp(1.0, 2.2);
    let stroke = Stroke::new(stroke_w, color);
    let origin = rect.min;
    let scale = rect.width().min(rect.height()) / GRID;
    let map = |x: f32, y: f32| pos2(origin.x + x * scale, origin.y + y * scale);

    let mut strokes: Vec<Vec<Pos2>> = Vec::new();
    parse_path(path(name), &mut strokes);
    let painter = ui.painter_at(rect.expand(1.0));
    for poly in strokes {
        if poly.len() < 2 {
            if let Some(&p) = poly.first() {
                // Zero-length LED dots → tiny square
                let s = stroke_w * 0.55;
                painter.rect_filled(
                    egui::Rect::from_center_size(map(p.x, p.y), Vec2::splat(s)),
                    0.0,
                    color,
                );
            }
            continue;
        }
        let pts: Vec<Pos2> = poly.iter().map(|p| map(p.x, p.y)).collect();
        painter.add(Shape::line(pts, stroke));
    }
}

/// Allocate a square slot and paint the icon.
pub fn show(ui: &mut Ui, name: &str, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui, name, rect, color);
    resp
}

/// Clickable flea-style chrome icon (no frame).
pub fn button(ui: &mut Ui, name: &str, size: f32, color: Color32) -> Response {
    let sense = Sense::click();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let c = if resp.hovered() {
        color.gamma_multiply(1.25).or_safe()
    } else {
        color
    };
    paint(ui, name, rect, c);
    resp
}

trait OrSafe {
    fn or_safe(self) -> Self;
}
impl OrSafe for Color32 {
    fn or_safe(self) -> Self {
        self
    }
}

// --- SVG path subset parser (Lucide / Flea) ---------------------------------

fn parse_path(src: &str, out: &mut Vec<Vec<Pos2>>) {
    let mut chars = src.chars().peekable();
    let mut nums: Vec<f32> = Vec::new();
    let mut cmd = 'M';
    let mut cur = Pos2::ZERO;
    let mut start = Pos2::ZERO;
    let mut poly: Vec<Pos2> = Vec::new();

    let flush = |poly: &mut Vec<Pos2>, out: &mut Vec<Vec<Pos2>>| {
        if !poly.is_empty() {
            out.push(std::mem::take(poly));
        }
    };

    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_whitespace() || ch == ',' {
            chars.next();
            continue;
        }
        if ch.is_ascii_alphabetic() {
            // Finish previous command with collected nums
            apply_cmd(
                cmd, &nums, &mut cur, &mut start, &mut poly, out, &flush,
            );
            nums.clear();
            cmd = chars.next().unwrap();
            if cmd == 'Z' || cmd == 'z' {
                apply_cmd(
                    cmd, &[], &mut cur, &mut start, &mut poly, out, &flush,
                );
            }
            continue;
        }
        // number
        let mut s = String::new();
        if ch == '-' || ch == '+' {
            s.push(chars.next().unwrap());
        }
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' {
                s.push(chars.next().unwrap());
            } else {
                break;
            }
        }
        if let Ok(v) = s.parse::<f32>() {
            nums.push(v);
        }
    }
    apply_cmd(
        cmd, &nums, &mut cur, &mut start, &mut poly, out, &flush,
    );
    flush(&mut poly, out);
}

fn apply_cmd(
    cmd: char,
    nums: &[f32],
    cur: &mut Pos2,
    start: &mut Pos2,
    poly: &mut Vec<Pos2>,
    out: &mut Vec<Vec<Pos2>>,
    flush: &dyn Fn(&mut Vec<Pos2>, &mut Vec<Vec<Pos2>>),
) {
    let rel = cmd.is_ascii_lowercase();
    let c = cmd.to_ascii_uppercase();
    match c {
        'M' => {
            let mut i = 0;
            while i + 1 < nums.len() {
                let p = abs_pt(rel, *cur, nums[i], nums[i + 1]);
                if i == 0 {
                    flush(poly, out);
                    poly.push(p);
                    *start = p;
                } else {
                    poly.push(p);
                }
                *cur = p;
                i += 2;
            }
        }
        'L' => {
            let mut i = 0;
            while i + 1 < nums.len() {
                let p = abs_pt(rel, *cur, nums[i], nums[i + 1]);
                poly.push(p);
                *cur = p;
                i += 2;
            }
        }
        'H' => {
            for &x in nums {
                let nx = if rel { cur.x + x } else { x };
                let p = pos2(nx, cur.y);
                poly.push(p);
                *cur = p;
            }
        }
        'V' => {
            for &y in nums {
                let ny = if rel { cur.y + y } else { y };
                let p = pos2(cur.x, ny);
                poly.push(p);
                *cur = p;
            }
        }
        'Z' => {
            poly.push(*start);
            flush(poly, out);
            *cur = *start;
        }
        'C' => {
            let mut i = 0;
            while i + 5 < nums.len() {
                let p1 = abs_pt(rel, *cur, nums[i], nums[i + 1]);
                let p2 = abs_pt(rel, *cur, nums[i + 2], nums[i + 3]);
                let p = abs_pt(rel, *cur, nums[i + 4], nums[i + 5]);
                tessellate_cubic(poly, *cur, p1, p2, p, 8);
                *cur = p;
                i += 6;
            }
        }
        'A' => {
            // rx ry x-rot large-arc sweep x y
            let mut i = 0;
            while i + 6 < nums.len() {
                let rx = nums[i].abs().max(0.01);
                let ry = nums[i + 1].abs().max(0.01);
                let large = nums[i + 3] != 0.0;
                let sweep = nums[i + 4] != 0.0;
                let end = abs_pt(rel, *cur, nums[i + 5], nums[i + 6]);
                tessellate_arc(poly, *cur, end, rx, ry, large, sweep, 16);
                *cur = end;
                i += 7;
            }
        }
        _ => {}
    }
}

fn abs_pt(rel: bool, cur: Pos2, x: f32, y: f32) -> Pos2 {
    if rel {
        pos2(cur.x + x, cur.y + y)
    } else {
        pos2(x, y)
    }
}

fn tessellate_cubic(poly: &mut Vec<Pos2>, p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, n: usize) {
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let x = u * u * u * p0.x
            + 3.0 * u * u * t * p1.x
            + 3.0 * u * t * t * p2.x
            + t * t * t * p3.x;
        let y = u * u * u * p0.y
            + 3.0 * u * u * t * p1.y
            + 3.0 * u * t * t * p2.y
            + t * t * t * p3.y;
        poly.push(pos2(x, y));
    }
}

/// Approximate elliptical arc (sufficient for Lucide circles).
fn tessellate_arc(
    poly: &mut Vec<Pos2>,
    from: Pos2,
    to: Pos2,
    rx: f32,
    ry: f32,
    _large: bool,
    sweep: bool,
    n: usize,
) {
    // Treat as circle/ellipse centered between endpoints when radii match distance —
    // for Lucide "a r r 0 1 0 x y" full circles we sample a full turn around midpoint.
    let mid = pos2((from.x + to.x) * 0.5, (from.y + to.y) * 0.5);
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let dist = (dx * dx + dy * dy).sqrt();
    // Lucide full-circle shorthand: `M(cx, cy-r) a r r 0 1 0 ε 0` with ε≈0.01.
    // Must use `<=` — a strict `< 0.01` missed the common ε and left only dots.
    if dist <= 0.02 {
        // Center sits one radius below the start (top of the circle).
        let cx = from.x;
        let cy = from.y + ry;
        let dir = if sweep { 1.0 } else { -1.0 };
        let a0 = -std::f32::consts::FRAC_PI_2;
        for i in 1..=n {
            let a = a0 + dir * std::f32::consts::TAU * (i as f32 / n as f32);
            poly.push(pos2(cx + rx * a.cos(), cy + ry * a.sin()));
        }
        return;
    }
    // Chord midpoint + perpendicular offset to estimate center
    let r = rx.max(ry);
    let h2 = (r * r - (dist * 0.5).powi(2)).max(0.0);
    let h = h2.sqrt();
    let nx = -dy / dist;
    let ny = dx / dist;
    let sign = if sweep { 1.0 } else { -1.0 };
    let cx = mid.x + sign * h * nx;
    let cy = mid.y + sign * h * ny;
    let a0 = (from.y - cy).atan2(from.x - cx);
    let a1 = (to.y - cy).atan2(to.x - cx);
    let mut delta = a1 - a0;
    if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    } else if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    }
    for i in 1..=n {
        let a = a0 + delta * (i as f32 / n as f32);
        poly.push(pos2(cx + rx * a.cos(), cy + ry * a.sin()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_icon_is_a_magnifying_glass_not_dots() {
        let mut strokes = Vec::new();
        parse_path(path("search"), &mut strokes);
        // Circle (many points) + handle (2 points). Lone single-point strokes
        // are what paint() renders as the “two dots” LED marks.
        assert!(
            strokes.iter().any(|s| s.len() >= 8),
            "expected a tessellated lens circle, got {strokes:?}"
        );
        assert!(
            strokes.iter().any(|s| s.len() >= 2 && {
                let a = s.first().unwrap();
                let b = s.last().unwrap();
                (*a - *b).length() > 3.0
            }),
            "expected a diagonal handle stroke, got {strokes:?}"
        );
        assert!(
            strokes.iter().all(|s| s.len() != 1),
            "single-point strokes paint as dots: {strokes:?}"
        );
    }

    #[test]
    fn lucide_epsilon_arc_draws_a_full_circle() {
        let mut strokes = Vec::new();
        // Same shorthand globe/camera/settings use.
        parse_path("M11 3a8 8 0 1 0 0.01 0", &mut strokes);
        let pts: usize = strokes.iter().map(|s| s.len()).sum();
        assert!(pts >= 12, "ε-arc should tessellate a circle, got {strokes:?}");
        let xs: Vec<f32> = strokes.iter().flat_map(|s| s.iter().map(|p| p.x)).collect();
        let ys: Vec<f32> = strokes.iter().flat_map(|s| s.iter().map(|p| p.y)).collect();
        let span_x = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let span_y = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            - ys.iter().cloned().fold(f32::INFINITY, f32::min);
        assert!(span_x > 10.0 && span_y > 10.0, "circle too flat: {span_x}×{span_y}");
    }
}
