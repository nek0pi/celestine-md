//! Nib-pen ink for canvas drawings: speed-based pressure while recording,
//! smoothing + even resampling on finish, tapered widths, and a feathered
//! triangle-strip mesh for painting.

use egui::{epaint::Vertex, Color32, Mesh, Pos2, Vec2};

/// Ignore pointer jitter smaller than this (canvas units).
const MIN_STEP: f32 = 1.5;
/// Pointer speed (canvas units / s) at which the nib reaches its thinnest.
const FAST_SPEED: f32 = 2400.0;
const MIN_PRESSURE: f32 = 0.35;
/// Fraction of the stroke length that tapers at each end.
const TAPER: f32 = 0.12;
const RESAMPLE_SPACING: f32 = 2.0;

/// Collects raw pointer samples for a stroke in progress.
#[derive(Default)]
pub struct InkRecorder {
    points: Vec<[f32; 3]>,
    last_time: f64,
}

impl InkRecorder {
    pub fn push(&mut self, p: Pos2, time: f64) {
        let Some(last) = self.points.last().copied() else {
            self.points.push([p.x, p.y, 1.0]);
            self.last_time = time;
            return;
        };
        let dist = Vec2::new(p.x - last[0], p.y - last[1]).length();
        if dist < MIN_STEP {
            return;
        }
        let dt = (time - self.last_time).max(1.0 / 240.0) as f32;
        let speed = dist / dt;
        let target = (1.0 - speed / FAST_SPEED).clamp(MIN_PRESSURE, 1.0);
        // Ease pressure so one fast sample does not pinch the line.
        let pressure = last[2] + (target - last[2]) * 0.35;
        self.points.push([p.x, p.y, pressure]);
        self.last_time = time;
    }

    pub fn points(&self) -> &[[f32; 3]] {
        &self.points
    }

    pub fn finish(self) -> Vec<[f32; 3]> {
        smooth(&self.points)
    }
}

/// Exponential moving average (keeps both endpoints), then Catmull-Rom
/// resampling at an even spacing.
pub fn smooth(raw: &[[f32; 3]]) -> Vec<[f32; 3]> {
    if raw.len() < 3 {
        return raw.to_vec();
    }
    let alpha = 0.45;
    let mut ema = Vec::with_capacity(raw.len());
    ema.push(raw[0]);
    for p in &raw[1..raw.len() - 1] {
        let prev: [f32; 3] = *ema.last().unwrap();
        ema.push([
            prev[0] + (p[0] - prev[0]) * alpha,
            prev[1] + (p[1] - prev[1]) * alpha,
            prev[2] + (p[2] - prev[2]) * alpha,
        ]);
    }
    ema.push(raw[raw.len() - 1]);

    let mut out = vec![ema[0]];
    for i in 0..ema.len() - 1 {
        let p0 = ema[i.saturating_sub(1)];
        let p1 = ema[i];
        let p2 = ema[i + 1];
        let p3 = ema[(i + 2).min(ema.len() - 1)];
        let seg = Vec2::new(p2[0] - p1[0], p2[1] - p1[1]).length();
        let steps = ((seg / RESAMPLE_SPACING).ceil() as usize).max(1);
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            out.push(catmull_rom(p0, p1, p2, p3, t));
        }
    }
    out
}

fn catmull_rom(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3], t: f32) -> [f32; 3] {
    let t2 = t * t;
    let t3 = t2 * t;
    let mut r = [0.0; 3];
    for k in 0..3 {
        r[k] = 0.5
            * (2.0 * p1[k]
                + (-p0[k] + p2[k]) * t
                + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * t2
                + (-p0[k] + 3.0 * p1[k] - 3.0 * p2[k] + p3[k]) * t3);
    }
    r[2] = r[2].clamp(0.0, 1.0);
    r
}

/// Per-point widths: brush size × pressure, eased to a point at both ends.
pub fn widths(points: &[[f32; 3]], size: f32) -> Vec<f32> {
    let mut arc = Vec::with_capacity(points.len());
    let mut total = 0.0;
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            let q = points[i - 1];
            total += Vec2::new(p[0] - q[0], p[1] - q[1]).length();
        }
        arc.push(total);
    }
    points
        .iter()
        .zip(&arc)
        .map(|(p, &s)| {
            let mut w = size * p[2].max(MIN_PRESSURE * 0.5);
            if total > 0.0 {
                let u = s / total;
                let edge = (u.min(1.0 - u) / TAPER).min(1.0);
                let ease = 1.0 - (1.0 - edge) * (1.0 - edge);
                w *= ease.max(0.08);
            }
            w
        })
        .collect()
}

/// Unit normals along the polyline (zero vector for degenerate points).
pub fn normals(points: &[[f32; 3]]) -> Vec<Vec2> {
    let n = points.len();
    (0..n)
        .map(|i| {
            let a = points[i.saturating_sub(1)];
            let b = points[(i + 1).min(n - 1)];
            let d = Vec2::new(b[0] - a[0], b[1] - a[1]);
            if d.length_sq() < 1e-6 {
                Vec2::ZERO
            } else {
                d.normalized().rot90()
            }
        })
        .collect()
}

/// Cached geometry that only depends on the stroke itself.
pub struct StrokeGeom {
    pub normals: Vec<Vec2>,
    pub widths: Vec<f32>,
}

impl StrokeGeom {
    pub fn new(points: &[[f32; 3]], size: f32) -> Self {
        Self {
            normals: normals(points),
            widths: widths(points, size),
        }
    }
}

/// Build a feathered mesh in screen space. `to_screen` maps canvas → screen,
/// `zoom` scales widths, and a 1px alpha fringe stands in for anti-aliasing.
pub fn tessellate(
    points: &[[f32; 3]],
    geom: &StrokeGeom,
    zoom: f32,
    color: Color32,
    to_screen: impl Fn(Pos2) -> Pos2,
) -> Mesh {
    let mut mesh = Mesh::default();
    if points.is_empty() {
        return mesh;
    }
    let clear = Color32::TRANSPARENT;
    let feather = 1.0;
    if points.len() == 1 || geom.normals.iter().all(|n| *n == Vec2::ZERO) {
        let c = to_screen(Pos2::new(points[0][0], points[0][1]));
        dot(&mut mesh, c, (geom.widths[0] * zoom * 0.5).max(0.75), color);
        return mesh;
    }
    for (i, p) in points.iter().enumerate() {
        let c = to_screen(Pos2::new(p[0], p[1]));
        let half = (geom.widths[i] * zoom * 0.5).max(0.35);
        let n = geom.normals[i];
        let v = |pos: Pos2, color| Vertex {
            pos,
            uv: egui::epaint::WHITE_UV,
            color,
        };
        mesh.vertices.push(v(c + n * (half + feather), clear));
        mesh.vertices.push(v(c + n * half, color));
        mesh.vertices.push(v(c - n * half, color));
        mesh.vertices.push(v(c - n * (half + feather), clear));
        if i > 0 {
            let a = ((i - 1) * 4) as u32;
            let b = (i * 4) as u32;
            for k in 0..3 {
                mesh.add_triangle(a + k, a + k + 1, b + k);
                mesh.add_triangle(a + k + 1, b + k + 1, b + k);
            }
        }
    }
    for idx in [0, points.len() - 1] {
        let p = points[idx];
        let c = to_screen(Pos2::new(p[0], p[1]));
        dot(&mut mesh, c, (geom.widths[idx] * zoom * 0.5).max(0.35), color);
    }
    mesh
}

/// A filled circle with a 1px feathered rim.
fn dot(mesh: &mut Mesh, c: Pos2, r: f32, color: Color32) {
    const SEG: u32 = 12;
    let base = mesh.vertices.len() as u32;
    mesh.colored_vertex(c, color);
    for s in 0..SEG {
        let a = s as f32 / SEG as f32 * std::f32::consts::TAU;
        let d = Vec2::angled(a);
        mesh.colored_vertex(c + d * r, color);
        mesh.colored_vertex(c + d * (r + 1.0), Color32::TRANSPARENT);
    }
    for s in 0..SEG {
        let i = base + 1 + s * 2;
        let j = base + 1 + ((s + 1) % SEG) * 2;
        mesh.add_triangle(base, i, j);
        mesh.add_triangle(i, i + 1, j);
        mesh.add_triangle(i + 1, j + 1, j);
    }
}

/// Distance from `p` to the stroke's centerline (canvas units).
pub fn distance_to(points: &[[f32; 3]], p: Pos2) -> f32 {
    match points.len() {
        0 => f32::INFINITY,
        1 => Pos2::new(points[0][0], points[0][1]).distance(p),
        _ => points
            .windows(2)
            .map(|w| {
                segment_distance(Pos2::new(w[0][0], w[0][1]), Pos2::new(w[1][0], w[1][1]), p)
            })
            .fold(f32::INFINITY, f32::min),
    }
}

pub fn segment_distance(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return a.distance(p);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wiggle() -> Vec<[f32; 3]> {
        (0..40)
            .map(|i| {
                let x = i as f32 * 6.0;
                [x, (x * 0.1).sin() * 20.0, 0.8]
            })
            .collect()
    }

    #[test]
    fn smoothing_keeps_endpoints() {
        let raw = wiggle();
        let out = smooth(&raw);
        assert_eq!(out.first(), raw.first());
        let (a, b) = (out.last().unwrap(), raw.last().unwrap());
        assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3);
        assert!(out.len() > raw.len());
    }

    #[test]
    fn ends_taper_to_near_zero() {
        let pts = smooth(&wiggle());
        let w = widths(&pts, 6.0);
        let mid = w[w.len() / 2];
        assert!(w[0] < mid * 0.15, "{} vs {mid}", w[0]);
        assert!(w[w.len() - 1] < mid * 0.15);
    }

    #[test]
    fn faster_strokes_are_thinner() {
        let mut slow = InkRecorder::default();
        let mut fast = InkRecorder::default();
        for i in 0..20 {
            let p = Pos2::new(i as f32 * 10.0, 0.0);
            slow.push(p, i as f64 * 0.2);
            fast.push(p, i as f64 * 0.002);
        }
        assert!(fast.points().last().unwrap()[2] < slow.points().last().unwrap()[2]);
    }

    #[test]
    fn short_strokes_do_not_panic() {
        for n in 0..3 {
            let pts: Vec<[f32; 3]> = (0..n).map(|i| [i as f32, 0.0, 1.0]).collect();
            let s = smooth(&pts);
            let geom = StrokeGeom::new(&s, 4.0);
            let _ = tessellate(&s, &geom, 1.0, Color32::WHITE, |p| p);
            let _ = distance_to(&s, Pos2::ZERO);
        }
        let same = vec![[1.0, 1.0, 1.0]; 4];
        let geom = StrokeGeom::new(&same, 4.0);
        let _ = tessellate(&same, &geom, 1.0, Color32::WHITE, |p| p);
    }
}
