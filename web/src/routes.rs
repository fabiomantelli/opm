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
    t.states
        .iter()
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
pub enum Route {
    Line(P, P),
    /// Cubic Bézier: start, control 1, control 2, end.
    Curve(P, P, P, P),
    /// Right-angle polyline.
    Ortho(Vec<P>),
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
            Route::Curve(a, c1, c2, b) => (0..=16)
                .map(|i| {
                    let t = i as f64 / 16.0;
                    let u = 1.0 - t;
                    let f = |a: f64, b: f64, c: f64, d: f64| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
                    (f(a.0, c1.0, c2.0, b.0), f(a.1, c1.1, c2.1, b.1))
                })
                .collect(),
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
            near.iter().flat_map(|id| adj.get(id).into_iter().flatten().map(|&(lid, _)| lid)).collect()
        });

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

        let mut links = HashMap::with_capacity(m.links.len());
        for l in &m.links {
            if let (Some(p), Some(dirty)) = (reuse, &dirty) {
                if !dirty.contains(&l.id) {
                    if let Some(g) = p.links.get(&l.id) {
                        links.insert(l.id, g.clone());
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
            let routes = link_routes(l, f, t, style, off, (sf, st), g);
            links.insert(l.id, Arc::new(geo(l, routes, style)));
        }
        Routes { style, links, boxes, link_ids }
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
    if l.kind == LinkKind::Effect && l.from_state.is_some() && l.to_state.is_some() && (obj_from || obj_to) {
        let (o, p) = if obj_from { (f, t) } else { (t, f) };
        let ps = shape_of(p);
        let po = if obj_from { st } else { sf };
        return vec![
            route(&pill_or_body(o, l.from_state), &ps, style, off, (0.0, po - 10.0), grid),
            route(&ps, &pill_or_body(o, l.to_state), style, off, (po + 10.0, 0.0), grid),
        ];
    }
    let s = l.from_state.or(l.to_state);
    // Ends on a state pill stay centered on it.
    let fs = if obj_from && s.is_some() { (pill_or_body(f, s), 0.0) } else { (shape_of(f), sf) };
    let ts = if obj_to && s.is_some() { (pill_or_body(t, s), 0.0) } else { (shape_of(t), st) };
    vec![route(&fs.0, &ts.0, style, off, (fs.1, ts.1), grid)]
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
