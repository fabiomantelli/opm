//! Shapes and link routing. All links are routed together in one pass (`Routes::compute`) with
//! indexes instead of per-link scans, cached in a memo, and shared by drawing and hit-testing.
//! While a thing is dragged only the links around it are re-routed.
use crate::P;
use opm_core::{
    layout::{size, state_w},
    Id, Kind, Link, LinkKind, Model, Thing,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// (state id, x, y, w, h) pills along the bottom of an object.
pub fn state_boxes(t: &Thing) -> Vec<(Id, f64, f64, f64, f64)> {
    let (w, h) = size(t);
    let total: f64 = t.states.iter().map(|s| state_w(&s.name) + 8.0).sum::<f64>() - 8.0;
    let mut x = t.x + (w - total) / 2.0;
    Model::states_in_order(t)
        .into_iter()
        .map(|s| {
            let sw = state_w(&s.name);
            let b = (s.id, x, t.y + h - 40.0, sw, 28.0);
            x += sw + 8.0;
            b
        })
        .collect()
}

pub struct Shape {
    pub cx: f64,
    pub cy: f64,
    pub hw: f64,
    pub hh: f64,
    pub ellipse: bool,
}

pub fn shape_of(t: &Thing) -> Shape {
    let (w, h) = size(t);
    Shape { cx: t.x + w / 2.0, cy: t.y + h / 2.0, hw: w / 2.0, hh: h / 2.0, ellipse: t.kind == Kind::Process }
}

pub fn pill_or_body(t: &Thing, state: Option<Id>) -> Shape {
    state
        .and_then(|s| state_boxes(t).into_iter().find(|b| b.0 == s))
        .map(|(_, x, y, w, h)| Shape { cx: x + w / 2.0, cy: y + h / 2.0, hw: w / 2.0, hh: h / 2.0, ellipse: false })
        .unwrap_or_else(|| shape_of(t))
}

/// Point where the ray from the shape's center toward `p` leaves the shape.
pub fn clip(s: &Shape, p: P) -> P {
    let (dx, dy) = (p.0 - s.cx, p.1 - s.cy);
    if dx == 0.0 && dy == 0.0 {
        return (s.cx, s.cy);
    }
    let t = if s.ellipse {
        1.0 / ((dx / s.hw).powi(2) + (dy / s.hh).powi(2)).sqrt()
    } else {
        (s.hw / dx.abs()).min(s.hh / dy.abs())
    };
    (s.cx + dx * t, s.cy + dy * t)
}

pub fn seg(a: &Shape, b: &Shape) -> (P, P) {
    (clip(a, (b.cx, b.cy)), clip(b, (a.cx, a.cy)))
}

pub fn dist(a: P, b: P) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

pub fn dist_to_seg(p: P, (a, b): (P, P)) -> f64 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let len2 = vx * vx + vy * vy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * vx + (p.1 - a.1) * vy) / len2).clamp(0.0, 1.0) };
    dist(p, (a.0 + t * vx, a.1 + t * vy))
}

/// How links are drawn; a per-viewer preference.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LinkStyle {
    Straight,
    Curved,
    Orthogonal,
}

impl LinkStyle {
    pub fn key(self) -> &'static str {
        match self {
            LinkStyle::Straight => "straight",
            LinkStyle::Curved => "curved",
            LinkStyle::Orthogonal => "orthogonal",
        }
    }

    pub fn from_key(s: &str) -> Self {
        match s {
            "curved" => LinkStyle::Curved,
            "orthogonal" => LinkStyle::Orthogonal,
            _ => LinkStyle::Straight,
        }
    }
}

/// One drawn piece of a link.
#[derive(Clone)]
pub enum Route {
    Line(P, P),
    /// Cubic Bézier: start, control 1, control 2, end.
    Curve(P, P, P, P),
    /// Right-angle polyline.
    Ortho(Vec<P>),
    /// Smooth curve through these points (a hand-shaped curved link).
    Spline(Vec<P>),
}

/// Catmull-Rom through `pts` as cubic Bézier pieces: (start, control 1, control 2, end).
fn spline_pieces(pts: &[P]) -> Vec<(P, P, P, P)> {
    let n = pts.len();
    (0..n.saturating_sub(1))
        .map(|i| {
            let (p0, p1, p2, p3) = (pts[i.saturating_sub(1)], pts[i], pts[i + 1], pts[(i + 2).min(n - 1)]);
            (p1, (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0), (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0), p2)
        })
        .collect()
}

fn bezier(a: P, c1: P, c2: P, b: P, t: f64) -> P {
    let u = 1.0 - t;
    let f = |a: f64, b: f64, c: f64, d: f64| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
    (f(a.0, c1.0, c2.0, b.0), f(a.1, c1.1, c2.1, b.1))
}

impl Route {
    /// SVG path; `bolt` puts the invocation lightning in a straight line's middle.
    pub fn d(&self, bolt: bool) -> String {
        match self {
            Route::Line(a, b) if bolt => {
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                let (ux, uy, nx, ny) = (dx / len, dy / len, -dy / len, dx / len);
                let (mx, my) = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                format!(
                    "M{:.1},{:.1} L{:.1},{:.1} L{:.1},{:.1} L{:.1},{:.1}",
                    a.0, a.1, mx - ux * 6.0 + nx * 8.0, my - uy * 6.0 + ny * 8.0, mx + ux * 6.0 - nx * 8.0, my + uy * 6.0 - ny * 8.0, b.0, b.1
                )
            }
            Route::Line(a, b) => format!("M{:.1},{:.1} L{:.1},{:.1}", a.0, a.1, b.0, b.1),
            Route::Curve(a, c1, c2, b) => {
                format!("M{:.1},{:.1} C{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}", a.0, a.1, c1.0, c1.1, c2.0, c2.1, b.0, b.1)
            }
            Route::Spline(pts) => {
                let mut d = format!("M{:.1},{:.1}", pts[0].0, pts[0].1);
                for (_, c1, c2, b) in spline_pieces(pts) {
                    d += &format!(" C{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}", c1.0, c1.1, c2.0, c2.1, b.0, b.1);
                }
                d
            }
            Route::Ortho(pts) => {
                // Rounded corners.
                let mut d = format!("M{:.1},{:.1}", pts[0].0, pts[0].1);
                for i in 1..pts.len() {
                    let (p, c) = (pts[i - 1], pts[i]);
                    if let Some(&n) = pts.get(i + 1) {
                        let r = 10.0f64.min(dist(p, c) / 2.0).min(dist(c, n) / 2.0);
                        let toward = |from: P, to: P| {
                            let l = dist(from, to).max(1e-6);
                            (from.0 + (to.0 - from.0) / l * r, from.1 + (to.1 - from.1) / l * r)
                        };
                        let (s, e) = (toward(c, p), toward(c, n));
                        d += &format!(" L{:.1},{:.1} Q{:.1},{:.1} {:.1},{:.1}", s.0, s.1, c.0, c.1, e.0, e.1);
                    } else {
                        d += &format!(" L{:.1},{:.1}", c.0, c.1);
                    }
                }
                d
            }
        }
    }

    /// Polyline approximation (for hit-testing and placing mid-link symbols).
    pub fn points(&self) -> Vec<P> {
        match self {
            Route::Line(a, b) => vec![*a, *b],
            Route::Ortho(pts) => pts.clone(),
            Route::Curve(a, c1, c2, b) => (0..=16).map(|i| bezier(*a, *c1, *c2, *b, i as f64 / 16.0)).collect(),
            Route::Spline(pts) => {
                let mut out = vec![pts[0]];
                for (a, c1, c2, b) in spline_pieces(pts) {
                    out.extend((1..=12).map(|i| bezier(a, c1, c2, b, i as f64 / 12.0)));
                }
                out
            }
        }
    }
}

/// Middle of a polyline and the direction back toward its start, in degrees.
pub fn mid_and_angle(pts: &[P]) -> (P, f64) {
    let total: f64 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    let mut left = total / 2.0;
    for w in pts.windows(2) {
        let l = dist(w[0], w[1]);
        if l >= left && l > 0.0 {
            let t = left / l;
            let p = (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
            return (p, (w[0].1 - w[1].1).atan2(w[0].0 - w[1].0).to_degrees());
        }
        left -= l;
    }
    (pts[0], 0.0)
}

/// How far along `path` the point nearest `p` is.
pub fn arc_of(path: &[P], p: P) -> f64 {
    let (mut best, mut at, mut run) = (f64::MAX, 0.0, 0.0);
    for w in path.windows(2) {
        let l = dist(w[0], w[1]);
        let d = dist_to_seg(p, (w[0], w[1]));
        if d < best {
            let t = if l > 0.0 { (((p.0 - w[0].0) * (w[1].0 - w[0].0) + (p.1 - w[0].1) * (w[1].1 - w[0].1)) / (l * l)).clamp(0.0, 1.0) } else { 0.0 };
            (best, at) = (d, run + t * l);
        }
        run += l;
    }
    at
}

/// The point `s` along `path`.
pub fn point_at(path: &[P], s: f64) -> P {
    let mut left = s;
    for w in path.windows(2) {
        let l = dist(w[0], w[1]);
        if left <= l && l > 0.0 {
            let t = left / l;
            return (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
        }
        left -= l;
    }
    *path.last().unwrap_or(&(0.0, 0.0))
}

/// Which side of `a` faces `b`: (vertical?, direction sign).
fn side(a: &Shape, b: &Shape) -> (bool, i8) {
    let (dx, dy) = (b.cx - a.cx, b.cy - a.cy);
    if dy.abs() - a.hh - b.hh >= dx.abs() - a.hw - b.hw {
        (true, if dy < 0.0 { -1 } else { 1 })
    } else {
        (false, if dx < 0.0 { -1 } else { 1 })
    }
}

/// Point on the `side` of a shape, shifted `off` along that side (following an ellipse's curve).
fn port_at(a: &Shape, (vertical, s): (bool, f64), off: f64) -> P {
    if vertical {
        let off = off.clamp(-a.hw * 0.85, a.hw * 0.85);
        let k = if a.ellipse { (1.0 - (off / a.hw).powi(2)).sqrt() } else { 1.0 };
        (a.cx + off, a.cy + s * a.hh * k)
    } else {
        let off = off.clamp(-a.hh * 0.85, a.hh * 0.85);
        let k = if a.ellipse { (1.0 - (off / a.hh).powi(2)).sqrt() } else { 1.0 };
        (a.cx + s * a.hw * k, a.cy + off)
    }
}

/// Axis-aligned box: x0, y0, x1, y1.
pub type Rect = (f64, f64, f64, f64);

/// Does an axis-aligned segment cross the box?
fn seg_hits_rect(a: P, b: P, r: &Rect) -> bool {
    a.0.min(b.0) < r.2 && a.0.max(b.0) > r.0 && a.1.min(b.1) < r.3 && a.1.max(b.1) > r.1
}

/// Uniform grid over the things' boxes so obstacle queries only look at nearby things.
struct Grid {
    rects: Vec<(Id, Rect)>,
    cells: HashMap<(i32, i32), Vec<usize>>,
}

const CELL: f64 = 160.0;

impl Grid {
    fn new(rects: Vec<(Id, Rect)>) -> Self {
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, (_, r)) in rects.iter().enumerate() {
            for cx in (r.0 / CELL).floor() as i32..=(r.2 / CELL).floor() as i32 {
                for cy in (r.1 / CELL).floor() as i32..=(r.3 / CELL).floor() as i32 {
                    cells.entry((cx, cy)).or_default().push(i);
                }
            }
        }
        Grid { rects, cells }
    }

    /// Indexes of boxes an axis-aligned segment crosses (may repeat), skipping things in `skip`.
    fn hits(&self, a: P, b: P, skip: [Id; 2], out: &mut Vec<usize>) {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        for cx in (x0 / CELL).floor() as i32..=(x1 / CELL).floor() as i32 {
            for cy in (y0 / CELL).floor() as i32..=(y1 / CELL).floor() as i32 {
                for &i in self.cells.get(&(cx, cy)).into_iter().flatten() {
                    let (id, r) = &self.rects[i];
                    if !skip.contains(id) && seg_hits_rect(a, b, r) {
                        out.push(i);
                    }
                }
            }
        }
    }

    /// Number of distinct boxes a polyline crosses.
    fn cost(&self, pts: &[P], skip: [Id; 2]) -> usize {
        let mut hit = vec![];
        for w in pts.windows(2) {
            self.hits(w[0], w[1], skip, &mut hit);
        }
        hit.sort_unstable();
        hit.dedup();
        hit.len()
    }
}

/// Route between two shapes. Straight links run center to center (`off` fans out parallel ones);
/// curved and orthogonal links leave and enter through the facing sides at `offs` along each side;
/// orthogonal ones also pick the elbow or detour that crosses the fewest things.
fn route(a: &Shape, b: &Shape, style: LinkStyle, off: f64, offs: (f64, f64), grid: Option<(&Grid, [Id; 2])>) -> Route {
    let (dx, dy) = (b.cx - a.cx, b.cy - a.cy);
    if style == LinkStyle::Straight {
        let (p, q) = seg(a, b);
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let (nx, ny) = (-dy / len * off, dx / len * off);
        return Route::Line((p.0 + nx, p.1 + ny), (q.0 + nx, q.1 + ny));
    }
    let (vertical, s) = side(a, b);
    let s = s as f64;
    let p = port_at(a, (vertical, s), offs.0);
    let q = port_at(b, (vertical, -s), offs.1);
    if style == LinkStyle::Curved {
        let dir = if vertical { (0.0, s) } else { (s, 0.0) };
        let along = if vertical { (q.1 - p.1).abs() } else { (q.0 - p.0).abs() };
        let k = (along / 2.0).max(30.0);
        return Route::Curve(p, (p.0 + dir.0 * k, p.1 + dir.1 * k), (q.0 - dir.0 * k, q.1 - dir.1 * k), q);
    }
    // Work in "main axis" coordinates (u along the link, v across) and swap back at the end.
    let (pu, pv, qu, qv) = if vertical { (p.1, p.0, q.1, q.0) } else { (p.0, p.1, q.0, q.1) };
    let back = |u: f64, v: f64| if vertical { (v, u) } else { (u, v) };
    let (u1, u2) = (pu + s * 22.0, qu - s * 22.0);
    let elbow = |mu: f64| vec![back(pu, pv), back(mu, pv), back(mu, qv), back(qu, qv)];
    let mut best = elbow((pu + qu) / 2.0);
    if let Some((grid, skip)) = grid {
        let len = |pts: &Vec<P>| pts.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>();
        let mut best_cost = (grid.cost(&best, skip), len(&best));
        if best_cost.0 > 0 {
            // Blocked: try elbows near either end, then detours through lanes beside each blocker.
            let mut cands = vec![elbow(u1), elbow(u2)];
            let mut blockers = vec![];
            for c in [&best, &cands[0], &cands[1]] {
                for w in c.windows(2) {
                    grid.hits(w[0], w[1], skip, &mut blockers);
                }
            }
            blockers.sort_unstable();
            blockers.dedup();
            for i in blockers {
                let r = grid.rects[i].1;
                let (lo, hi) = if vertical { (r.0, r.2) } else { (r.1, r.3) };
                for lane in [lo - 18.0, hi + 18.0] {
                    cands.push(vec![back(pu, pv), back(u1, pv), back(u1, lane), back(u2, lane), back(u2, qv), back(qu, qv)]);
                }
            }
            for c in cands {
                let cost = (grid.cost(&c, skip), len(&c));
                if cost.0 < best_cost.0 || (cost.0 == best_cost.0 && cost.1 < best_cost.1) {
                    (best, best_cost) = (c, cost);
                }
            }
        }
    }
    best.dedup_by(|a, b| dist(*a, *b) < 0.5);
    Route::Ortho(best)
}

/// A point as a zero-size shape.
fn point(p: P) -> Shape {
    Shape { cx: p.0, cy: p.1, hw: 0.0, hh: 0.0, ellipse: false }
}

/// Route from a state pill of an object (box `body`) to `other`. It leaves through the pill's bottom and
/// the object's bottom edge, then goes around the object when `other` is above, so it never crosses the
/// object's name or other states. `back` reverses it (the link ends on the pill).
fn from_pill(pill: &Shape, body: Rect, other: &Shape, other_off: f64, style: LinkStyle, grid: Option<(&Grid, [Id; 2])>, back: bool) -> Route {
    let b = (pill.cx, pill.cy + pill.hh);
    let s = (pill.cx, body.3 + 14.0);
    let (vertical, sg) = side(other, &point(s));
    let sg = sg as f64;
    let q = port_at(other, (vertical, sg), other_off);
    // Just outside the other end's side, where its last segment starts.
    let qs = if vertical { (q.0, q.1 + sg * 22.0) } else { (q.0 + sg * 22.0, q.1) };
    let toward = if other.cx < pill.cx { -1.0 } else { 1.0 };
    let lanes: Vec<f64> = {
        let mut v = vec![if toward < 0.0 { body.0 - 16.0 } else { body.2 + 16.0 }, if toward < 0.0 { body.2 + 16.0 } else { body.0 - 16.0 }];
        if qs.0 < body.0 - 16.0 || qs.0 > body.2 + 16.0 {
            v.insert(0, qs.0);
        }
        v
    };
    let route = if style == LinkStyle::Orthogonal || other.cy - other.hh < s.1 {
        let mut cands: Vec<Vec<P>> = vec![];
        if qs.1 >= s.1 {
            // Below: drop straight down, then across.
            cands.push(if vertical { vec![b, (b.0, (s.1 + qs.1) / 2.0), (q.0, (s.1 + qs.1) / 2.0), q] } else { vec![b, (b.0, q.1), q] });
            cands.push(vec![b, s, (qs.0, s.1), qs, q]);
        }
        for &lane in &lanes {
            // Around the object's side, then in.
            cands.push(if vertical { vec![b, s, (lane, s.1), (lane, qs.1), qs, q] } else { vec![b, s, (lane, s.1), (lane, q.1), q] });
        }
        let len = |c: &Vec<P>| c.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>();
        // The first stretch is inside the object by design; the rest should cross nothing, not even it.
        let cost = |c: &Vec<P>| (grid.map_or(0, |(g, skip)| g.cost(&c[1..], [skip[0], skip[0]])), len(c));
        let cost = |c: &Vec<P>| {
            let (n, l) = cost(c);
            // Lines rising back through the object count as crossing it.
            let through = c.windows(2).skip(1).any(|w| w[0].0 > body.0 && w[0].0 < body.2 && w[1].1 < body.3 && w[0].0 == w[1].0);
            (n + through as usize * 2, l)
        };
        // Straight and curved styles going around take the same right-angle path.
        let mut best = cands.into_iter().min_by(|a, b| cost(a).partial_cmp(&cost(b)).unwrap()).unwrap();
        best.dedup_by(|a, b| dist(*a, *b) < 0.5);
        Route::Ortho(best)
    } else if style == LinkStyle::Curved {
        let k = ((q.1 - b.1).abs() / 2.0).max(30.0);
        let dir = if vertical { (0.0, sg) } else { (sg, 0.0) };
        Route::Curve(b, (b.0, b.1 + k), (q.0 + dir.0 * k, q.1 + dir.1 * k), q)
    } else {
        Route::Ortho(vec![b, s, clip(other, s)])
    };
    route.flip(back)
}

/// Moves apart the middle segments of right-angle links that run along the same line, so each can be
/// followed: 8px apart, ordered by where they come from and go to so they don't cross each other.
/// End segments stay put (they sit on the things' ports).
fn nudge(raw: &mut HashMap<Id, Vec<Route>>) {
    // (link, piece, segment index, vertical, coordinate, from, to, lean)
    let mut segs: Vec<(Id, usize, usize, bool, f64, f64, f64, f64)> = vec![];
    for (&id, routes) in raw.iter() {
        for (pi, r) in routes.iter().enumerate() {
            let Route::Ortho(pts) = r else { continue };
            for i in 1..pts.len().saturating_sub(2) {
                let (a, b) = (pts[i], pts[i + 1]);
                let vertical = (a.0 - b.0).abs() < 0.5;
                if !vertical && (a.1 - b.1).abs() >= 0.5 {
                    continue;
                }
                let (c, lo, hi) = if vertical { (a.0, a.1.min(b.1), a.1.max(b.1)) } else { (a.1, a.0.min(b.0), a.0.max(b.0)) };
                // Which way its neighbors turn: toward lower or higher coordinates.
                let (p0, p3) = (pts[i - 1], pts[i + 2]);
                let lean = if vertical { p0.0 + p3.0 - 2.0 * c } else { p0.1 + p3.1 - 2.0 * c };
                segs.push((id, pi, i, vertical, c, lo, hi, lean));
            }
        }
    }
    segs.sort_by(|a, b| (a.3, a.4).partial_cmp(&(b.3, b.4)).unwrap());
    let mut moves: Vec<(Id, usize, usize, bool, f64)> = vec![];
    let mut start = 0;
    while start < segs.len() {
        // Same direction, (nearly) the same line.
        let mut end = start + 1;
        while end < segs.len() && segs[end].3 == segs[start].3 && segs[end].4 - segs[end - 1].4 < 4.0 {
            end += 1;
        }
        let mut group: Vec<_> = segs[start..end].to_vec();
        group.sort_by(|a, b| a.5.total_cmp(&b.5));
        // Split into runs that actually overlap along the line.
        let mut runs: Vec<Vec<(Id, usize, usize, bool, f64, f64, f64, f64)>> = vec![];
        let mut reach = f64::MIN;
        for g in group {
            if g.5 < reach - 2.0 {
                runs.last_mut().unwrap().push(g);
            } else {
                runs.push(vec![g]);
            }
            reach = reach.max(g.6);
        }
        for mut run in runs.into_iter().filter(|r| r.len() > 1) {
            run.sort_by(|a, b| a.7.total_cmp(&b.7).then(a.0.cmp(&b.0)));
            let base = run.iter().map(|g| g.4).sum::<f64>() / run.len() as f64;
            let n = run.len() as f64;
            for (k, g) in run.iter().enumerate() {
                moves.push((g.0, g.1, g.2, g.3, base + (k as f64 - (n - 1.0) / 2.0) * 8.0));
            }
        }
        start = end;
    }
    for (id, pi, i, vertical, c) in moves {
        if let Some(Route::Ortho(pts)) = raw.get_mut(&id).and_then(|r| r.get_mut(pi)) {
            for j in [i, i + 1] {
                if vertical { pts[j].0 = c } else { pts[j].1 = c }
            }
        }
    }
}

/// Everything needed to draw and hit-test one link.
pub struct LinkGeo {
    /// SVG path per drawn piece (an Effect with in/out states has two).
    pub d: Vec<String>,
    /// Polylines per piece.
    pub pts: Vec<Vec<P>>,
    /// Middle of the first piece and the direction toward its start (degrees).
    pub mid: (P, f64),
    /// Bounding box of all pieces, for a quick hit-test reject.
    pub bbox: Rect,
    /// Unique per computation: a stable key for keyed rendering.
    pub rev: u64,
}

static REV: AtomicU64 = AtomicU64::new(1);

/// Routes for every link of a model, plus what they were computed from (to re-route incrementally).
pub struct Routes {
    pub style: LinkStyle,
    pub links: HashMap<Id, Arc<LinkGeo>>,
    /// Routes before nudging, to reuse while dragging.
    raw: HashMap<Id, Vec<Route>>,
    /// Hand-shaped routes at compute time.
    hand: HashMap<Id, opm_core::Route>,
    /// Box and state count per thing at compute time.
    boxes: HashMap<Id, (Rect, usize)>,
    link_ids: Vec<Id>,
}

// A memo only needs to know "changed"; every recompute is a change.
impl PartialEq for Routes {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

fn thing_box(t: &Thing) -> Rect {
    let (w, h) = size(t);
    (t.x, t.y, t.x + w, t.y + h)
}

impl Routes {
    /// Routes all links. With `prev` and `dragging`, only links near things that moved are re-routed;
    /// the rest are reused (their obstacle detours catch up when the drag ends).
    pub fn compute(m: &Model, style: LinkStyle, prev: Option<&Routes>, dragging: bool) -> Routes {
        let things: HashMap<Id, &Thing> = m.things.iter().map(|t| (t.id, t)).collect();
        let shapes: HashMap<Id, Shape> = m.things.iter().map(|t| (t.id, shape_of(t))).collect();
        let boxes: HashMap<Id, (Rect, usize)> = m.things.iter().map(|t| (t.id, (thing_box(t), t.states.len()))).collect();
        let link_ids: Vec<Id> = m.links.iter().map(|l| l.id).collect();

        // Adjacency: thing -> (link, other end).
        let mut adj: HashMap<Id, Vec<(Id, Id)>> = HashMap::new();
        for l in &m.links {
            adj.entry(l.from).or_default().push((l.id, l.to));
            adj.entry(l.to).or_default().push((l.id, l.from));
        }

        // Which links need routing.
        let reuse = prev.filter(|p| dragging && p.style == style && p.link_ids == link_ids);
        let dirty: Option<HashSet<Id>> = reuse.map(|p| {
            let moved: Vec<Id> = boxes.iter().filter(|(id, b)| p.boxes.get(id) != Some(b)).map(|(id, _)| *id).collect();
            // A move changes the moved thing's links and how its neighbors' sides are shared.
            let mut near: HashSet<Id> = moved.iter().copied().collect();
            for id in &moved {
                near.extend(adj.get(id).into_iter().flatten().map(|&(_, o)| o));
            }
            let mut dirty: HashSet<Id> = near.iter().flat_map(|id| adj.get(id).into_iter().flatten().map(|&(lid, _)| lid)).collect();
            // And links being shaped by hand.
            dirty.extend(m.links.iter().filter(|l| l.route.as_ref() != p.hand.get(&l.id)).map(|l| l.id));
            dirty
        });
        let hand: HashMap<Id, opm_core::Route> = m.links.iter().filter_map(|l| Some((l.id, l.route.clone()?))).collect();

        // Parallel links between the same pair fan out (straight style).
        let mut pairs: HashMap<(Id, Id), Vec<Id>> = HashMap::new();
        for l in &m.links {
            pairs.entry((l.from.min(l.to), l.from.max(l.to))).or_default().push(l.id);
        }

        // Link ends sharing a side of a thing spread along it, ordered by where the other ends are.
        let mut spread: HashMap<(Id, Id), f64> = HashMap::new();
        if style != LinkStyle::Straight {
            for (&tid, ends) in &adj {
                let me = &shapes[&tid];
                let mut by_side: HashMap<(bool, i8), Vec<(f64, Id)>> = HashMap::new();
                for &(lid, o) in ends {
                    let os = &shapes[&o];
                    let sd = side(me, os);
                    by_side.entry(sd).or_default().push((if sd.0 { os.cx } else { os.cy }, lid));
                }
                for (sd, mut peers) in by_side {
                    peers.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    let n = peers.len() as f64;
                    let span = if sd.0 { me.hw } else { me.hh } * 1.4;
                    for (i, &(_, lid)) in peers.iter().enumerate() {
                        spread.insert((lid, tid), (i as f64 - (n - 1.0) / 2.0) * (span / n).min(24.0));
                    }
                }
            }
        }

        let grid = (style == LinkStyle::Orthogonal).then(|| {
            // An in-zoomed frame is a container, not an obstacle.
            Grid::new(m.things.iter().filter(|t| t.frame.is_none()).map(|t| {
                let r = thing_box(t);
                (t.id, (r.0 - 6.0, r.1 - 6.0, r.2 + 6.0, r.3 + 6.0))
            }).collect())
        });

        let mut raw: HashMap<Id, Vec<Route>> = HashMap::with_capacity(m.links.len());
        for l in &m.links {
            if let (Some(p), Some(dirty)) = (reuse, &dirty) {
                if !dirty.contains(&l.id) {
                    if let Some(r) = p.raw.get(&l.id) {
                        raw.insert(l.id, r.clone());
                        continue;
                    }
                }
            }
            let (Some(f), Some(t)) = (things.get(&l.from), things.get(&l.to)) else { continue };
            let pair = &pairs[&(l.from.min(l.to), l.from.max(l.to))];
            let k = pair.iter().position(|&id| id == l.id).unwrap_or(0) as f64;
            let mut off = (k - (pair.len() as f64 - 1.0) / 2.0) * 18.0;
            if f.id > t.id {
                off = -off; // same side whichever way each link points
            }
            let sf = spread.get(&(l.id, f.id)).copied().unwrap_or(0.0);
            let st = spread.get(&(l.id, t.id)).copied().unwrap_or(0.0);
            let g = grid.as_ref().map(|g| (g, [f.id, t.id]));
            let effect_pair = l.kind == LinkKind::Effect && l.from_state.is_some() && l.to_state.is_some();
            let routes = match &l.route {
                // ponytail: effects drawn as two pieces (in-state, out-state) stay automatic.
                Some(r) if !effect_pair => vec![hand_route(l, r, f, t, style)],
                _ => link_routes(l, f, t, style, off, (sf, st), g),
            };
            raw.insert(l.id, routes);
        }
        let mut drawn = raw.clone();
        if style == LinkStyle::Orthogonal {
            // Hand-shaped links are where the user put them.
            let mut auto: HashMap<Id, Vec<Route>> = raw.iter().filter(|(id, _)| !hand.contains_key(id)).map(|(id, r)| (*id, r.clone())).collect();
            nudge(&mut auto);
            drawn.extend(auto);
        }
        // Unchanged links keep their geometry (and DOM).
        let same = prev.filter(|p| p.style == style);
        let mut links = HashMap::with_capacity(m.links.len());
        for l in &m.links {
            let Some(routes) = drawn.remove(&l.id) else { continue };
            let pts: Vec<Vec<P>> = routes.iter().map(Route::points).collect();
            match same.and_then(|p| p.links.get(&l.id)).filter(|g| g.pts == pts) {
                Some(g) => links.insert(l.id, g.clone()),
                None => links.insert(l.id, Arc::new(geo(l, routes, style))),
            };
        }
        Routes { style, links, raw, hand, boxes, link_ids }
    }

    /// The link under `p`, if any.
    pub fn hit(&self, p: P, tol: f64) -> Option<Id> {
        self.links
            .iter()
            .filter(|(_, g)| p.0 >= g.bbox.0 - tol && p.0 <= g.bbox.2 + tol && p.1 >= g.bbox.1 - tol && p.1 <= g.bbox.3 + tol)
            .find(|(_, g)| g.pts.iter().any(|pts| pts.windows(2).any(|w| dist_to_seg(p, (w[0], w[1])) < tol)))
            .map(|(&id, _)| id)
    }
}

fn link_routes(l: &Link, f: &Thing, t: &Thing, style: LinkStyle, off: f64, (sf, st): (f64, f64), grid: Option<(&Grid, [Id; 2])>) -> Vec<Route> {
    let obj_from = f.kind == Kind::Object && t.kind == Kind::Process;
    let obj_to = f.kind == Kind::Process && t.kind == Kind::Object;
    if !(obj_from || obj_to) || (l.from_state.is_none() && l.to_state.is_none()) {
        return vec![route(&shape_of(f), &shape_of(t), style, off, (sf, st), grid)];
    }
    // State-specific: the object end sits on a state pill.
    let (o, p, po) = if obj_from { (f, t, st) } else { (t, f, sf) };
    let (body, ps) = (thing_box(o), shape_of(p));
    let g = grid.map(|(g, _)| (g, [p.id, o.id]));
    let pill = |s: Option<Id>| pill_or_body(o, s);
    if l.kind == LinkKind::Effect && l.from_state.is_some() && l.to_state.is_some() {
        // Input state -> process -> output state.
        return vec![
            from_pill(&pill(l.from_state), body, &ps, po - 10.0, style, g, false),
            from_pill(&pill(l.to_state), body, &ps, po + 10.0, style, g, true),
        ];
    }
    // The pill end is `from` when the object is the link's source.
    vec![from_pill(&pill(l.from_state.or(l.to_state)), body, &ps, po, style, g, !obj_from)]
}

impl Route {
    /// The same route walked the other way.
    fn flip(self, yes: bool) -> Route {
        if !yes {
            return self;
        }
        match self {
            Route::Line(a, z) => Route::Line(z, a),
            Route::Curve(a, c1, c2, z) => Route::Curve(z, c2, c1, a),
            Route::Ortho(mut pts) => {
                pts.reverse();
                Route::Ortho(pts)
            }
            Route::Spline(mut pts) => {
                pts.reverse();
                Route::Spline(pts)
            }
        }
    }
}

/// Drops repeated points and middle points of straight right-angle runs (also back-and-forth ones).
fn simplify(pts: Vec<P>) -> Vec<P> {
    let mut v: Vec<P> = vec![];
    for p in pts {
        if v.last().is_none_or(|l: &P| dist(*l, p) > 0.5) {
            v.push(p);
        }
    }
    let mut i = 1;
    while i + 1 < v.len() {
        let (a, b, c) = (v[i - 1], v[i], v[i + 1]);
        let line = ((a.0 - b.0).abs() < 0.5 && (b.0 - c.0).abs() < 0.5) || ((a.1 - b.1).abs() < 0.5 && (b.1 - c.1).abs() < 0.5);
        if line {
            v.remove(i);
        } else {
            i += 1;
        }
    }
    v
}

/// One end of a hand-shaped link: the point on the shape and the direction it leaves in.
/// `anchor` (fractions of the thing's box) wins; otherwise it faces `toward`.
fn hand_end(shape: &Shape, t: &Thing, anchor: Option<(f64, f64)>, toward: P, style: LinkStyle) -> (P, P) {
    let normal = |p: P| {
        let (dx, dy) = ((p.0 - shape.cx) / shape.hw.max(1.0), (p.1 - shape.cy) / shape.hh.max(1.0));
        if style == LinkStyle::Orthogonal || !shape.ellipse {
            if dy.abs() >= dx.abs() { (0.0, dy.signum()) } else { (dx.signum(), 0.0) }
        } else {
            let l = (dx * dx + dy * dy).sqrt().max(1e-6);
            (dx / l, dy / l)
        }
    };
    if let Some((u, v)) = anchor {
        let (w, h) = size(t);
        let p = clip(shape, (t.x + u * w, t.y + v * h));
        return (p, normal(p));
    }
    if style == LinkStyle::Orthogonal {
        let (vertical, sg) = side(shape, &point(toward));
        let off = if vertical { toward.0 - shape.cx } else { toward.1 - shape.cy };
        let p = port_at(shape, (vertical, sg as f64), off);
        return (p, if vertical { (0.0, sg as f64) } else { (sg as f64, 0.0) });
    }
    let p = clip(shape, toward);
    (p, normal(p))
}

/// A link the user shaped in this diagram: through its points, from its ends' anchors.
fn hand_route(l: &Link, hand: &opm_core::Route, f: &Thing, t: &Thing, style: LinkStyle) -> Route {
    let obj_from = f.kind == Kind::Object && t.kind == Kind::Process;
    let obj_to = f.kind == Kind::Process && t.kind == Kind::Object;
    let state = l.from_state.or(l.to_state);
    // An end on a state pill stays on its pill.
    let (fs, fa) = if obj_from && state.is_some() { (pill_or_body(f, state), None) } else { (shape_of(f), hand.from) };
    let (ts, ta) = if obj_to && state.is_some() { (pill_or_body(t, state), None) } else { (shape_of(t), hand.to) };
    let ws = &hand.points;
    let (p0, n0) = hand_end(&fs, f, fa, ws.first().copied().unwrap_or((ts.cx, ts.cy)), style);
    let (q, n1) = hand_end(&ts, t, ta, ws.last().copied().unwrap_or((fs.cx, fs.cy)), style);
    match style {
        LinkStyle::Straight => Route::Ortho(std::iter::once(p0).chain(ws.iter().copied()).chain([q]).collect()),
        LinkStyle::Curved if ws.is_empty() => {
            let k = (dist(p0, q) / 2.5).max(30.0);
            Route::Curve(p0, (p0.0 + n0.0 * k, p0.1 + n0.1 * k), (q.0 + n1.0 * k, q.1 + n1.1 * k), q)
        }
        LinkStyle::Curved => Route::Spline(std::iter::once(p0).chain(ws.iter().copied()).chain([q]).collect()),
        LinkStyle::Orthogonal => {
            // Leave and enter straight out of the sides, pass through every point at right angles.
            let s0 = (p0.0 + n0.0 * 16.0, p0.1 + n0.1 * 16.0);
            let s1 = (q.0 + n1.0 * 16.0, q.1 + n1.1 * 16.0);
            let mut pts = vec![p0, s0];
            let mut vertical = n0.0 == 0.0;
            for &b in ws {
                let a = *pts.last().unwrap();
                if (a.0 - b.0).abs() > 0.5 && (a.1 - b.1).abs() > 0.5 {
                    pts.push(if vertical { (a.0, b.1) } else { (b.0, a.1) });
                }
                pts.push(b);
                vertical = (pts[pts.len() - 2].0 - b.0).abs() < 0.5;
            }
            // The last turn lines up with the side it enters.
            let a = *pts.last().unwrap();
            if (a.0 - s1.0).abs() > 0.5 && (a.1 - s1.1).abs() > 0.5 {
                pts.push(if n1.0 == 0.0 { (s1.0, a.1) } else { (a.0, s1.1) });
            }
            pts.extend([s1, q]);
            Route::Ortho(simplify(pts))
        }
    }
}

fn geo(l: &Link, routes: Vec<Route>, style: LinkStyle) -> LinkGeo {
    let bolt = l.kind == LinkKind::Invocation && style == LinkStyle::Straight;
    let pts: Vec<Vec<P>> = routes.iter().map(Route::points).collect();
    let mut bbox = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts.iter().flatten() {
        bbox = (bbox.0.min(p.0), bbox.1.min(p.1), bbox.2.max(p.0), bbox.3.max(p.1));
    }
    LinkGeo {
        d: routes.iter().map(|r| r.d(bolt)).collect(),
        mid: pts.first().map(|p| mid_and_angle(p)).unwrap_or(((0.0, 0.0), 0.0)),
        pts,
        bbox,
        rev: REV.fetch_add(1, Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opm_core::{Essence, Op};

    fn thing(m: &mut Model, name: &str, kind: Kind, x: f64, y: f64) -> Id {
        m.apply(Op::AddThing { name: name.into(), kind, essence: Essence::Informatical, affiliation: Default::default(), opd: None, x: Some(x), y: Some(y), inside: None })
            .unwrap()
    }

    /// Does a polyline enter the box (shrunk by `inset`)?
    fn crosses(pts: &[P], r: Rect, inset: f64) -> bool {
        let r = (r.0 + inset, r.1 + inset, r.2 - inset, r.3 - inset);
        pts.windows(2).any(|w| (0..=20).any(|k| {
            let t = k as f64 / 20.0;
            let (x, y) = (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
            x > r.0 && x < r.2 && y > r.1 && y < r.3
        }))
    }

    #[test]
    fn state_links_go_around_their_object_and_shared_lanes_spread() {
        let mut m = Model::new("t");
        let doc = thing(&mut m, "Document", Kind::Object, 300.0, 300.0);
        let draft = m.apply(Op::AddState { thing: doc, name: "draft".into() }).unwrap();
        let fin = m.apply(Op::AddState { thing: doc, name: "final".into() }).unwrap();
        // A process above-left and one below-right: both links must stay out of the object's body.
        let above = thing(&mut m, "Reviewing", Kind::Process, 40.0, 60.0);
        let below = thing(&mut m, "Publishing", Kind::Process, 600.0, 600.0);
        m.apply(Op::AddLink { from: doc, to: above, kind: LinkKind::Consumption, from_state: Some(draft), to_state: None }).unwrap();
        m.apply(Op::AddLink { from: doc, to: below, kind: LinkKind::Instrument, from_state: Some(fin), to_state: None }).unwrap();
        let v = m.view(m.root());
        let t = v.thing(doc).unwrap();
        let body = thing_box(t);
        let pills = state_boxes(t);
        for style in [LinkStyle::Orthogonal, LinkStyle::Straight, LinkStyle::Curved] {
            let r = Routes::compute(&v, style, None, false);
            for (id, g) in &r.links {
                for pts in &g.pts {
                    // Leave the pill it starts on out of it; the rest of the object must stay clear.
                    let inner: Vec<P> = pts.iter().copied().filter(|p| !pills.iter().any(|b| p.0 >= b.1 - 1.0 && p.0 <= b.1 + b.3 + 1.0 && p.1 >= b.2 - 1.0 && p.1 <= body.3 + 1.0)).collect();
                    assert!(!crosses(&inner, body, 2.0), "{style:?} link {id} crosses the object: {pts:?}");
                }
            }
        }
        // Pills are ordered toward their processes: draft (linked left) before final (linked right).
        assert_eq!(pills.iter().map(|b| b.0).collect::<Vec<_>>(), [draft, fin]);
    }

    #[test]
    fn hand_routes_pass_through_their_points_at_right_angles() {
        let mut m = Model::new("t");
        let a = thing(&mut m, "Source", Kind::Object, 0.0, 0.0);
        let b = thing(&mut m, "Using", Kind::Process, 400.0, 300.0);
        let l = m.apply(Op::AddLink { from: a, to: b, kind: LinkKind::Consumption, from_state: None, to_state: None }).unwrap();
        // The user dragged the middle of the link to y = 200.
        m.apply(Op::SetRoute { opd: None, link: l, points: vec![(250.0, 200.0)], from: None, to: None }).unwrap();
        let ortho = |m: &Model| {
            let r = Routes::compute(&m.view(m.root()), LinkStyle::Orthogonal, None, false);
            r.links[&l].pts[0].clone()
        };
        let pts = ortho(&m);
        assert!(pts.windows(2).all(|w| (w[0].0 - w[1].0).abs() < 0.5 || (w[0].1 - w[1].1).abs() < 0.5), "right angles: {pts:?}");
        assert!(pts.windows(2).any(|w| (w[0].1 - 200.0).abs() < 0.5 && (w[1].1 - 200.0).abs() < 0.5), "runs along y = 200: {pts:?}");
        // Moving one end keeps the user's middle and the ends stay attached.
        m.apply(Op::Move { opd: None, id: b, x: 600.0, y: 320.0, auto: false }).unwrap();
        let pts = ortho(&m);
        assert!(pts.windows(2).any(|w| (w[0].1 - 200.0).abs() < 0.5 && (w[1].1 - 200.0).abs() < 0.5), "{pts:?}");
        let end = *pts.last().unwrap();
        assert!(end.0 >= 600.0 && end.1 >= 320.0 - 0.5, "ends on the moved process: {pts:?}");
        // An anchored end sits where it was pinned: the right middle of the object's box.
        m.apply(Op::SetRoute { opd: None, link: l, points: vec![(250.0, 200.0)], from: Some((1.0, 0.5)), to: None }).unwrap();
        let v = m.view(m.root());
        let (w, h) = size(v.thing(a).unwrap());
        assert_eq!(ortho(&m)[0], (w, h / 2.0));
        // Curved and straight routes go through the point too.
        for style in [LinkStyle::Straight, LinkStyle::Curved] {
            let r = Routes::compute(&v, style, None, false);
            assert!(r.links[&l].pts[0].iter().any(|p| dist(*p, (250.0, 200.0)) < 1.0), "{style:?}");
        }
    }

    #[test]
    fn overlapping_middle_segments_move_apart() {
        let mut raw: HashMap<Id, Vec<Route>> = HashMap::new();
        for (id, y0, y1) in [(1, 0.0, 300.0), (2, 50.0, 250.0), (3, 400.0, 500.0)] {
            raw.insert(id, vec![Route::Ortho(vec![(0.0, y0), (100.0, y0), (100.0, y1), (200.0, y1)])]);
        }
        nudge(&mut raw);
        let x = |id: Id| match &raw[&id][0] {
            Route::Ortho(p) => (p[1].0, p[2].0),
            _ => unreachable!(),
        };
        assert_eq!(x(1), (96.0, 96.0));
        assert_eq!(x(2), (104.0, 104.0));
        assert_eq!(x(3), (100.0, 100.0), "not overlapping: stays");
    }
}
