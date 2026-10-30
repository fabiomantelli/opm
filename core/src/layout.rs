//! Shape sizes and the layered "tidy" layout, shared by the editor and the MCP server.
use crate::{Id, Kind, LinkKind, Model, Thing};
use std::{collections::HashMap, f64::consts::SQRT_2};

fn text_w(s: &str, ch: f64) -> f64 {
    s.chars().count() as f64 * ch
}

pub fn state_w(s: &str) -> f64 {
    (text_w(s, 7.0) + 22.0).max(46.0)
}

/// Rendered (width, height) of a thing.
pub fn size(t: &Thing) -> (f64, f64) {
    if let Some(f) = t.frame {
        return f;
    }
    match t.kind {
        Kind::Process => {
            let dw = duration_label(t).map_or(0.0, |d| text_w(&d, 6.6) + 56.0);
            ((text_w(&t.name, 7.8) + 64.0).max(132.0).max(dw), 66.0)
        }
        Kind::Object => {
            let more = if t.more_states { state_w(MORE) + 8.0 } else { 0.0 } + value_w(t);
            let sw: f64 = t.states.iter().map(|s| state_w(&s.name) + 8.0).sum::<f64>() + more + 16.0;
            let pw = t.parts.iter().map(|p| text_w(&part_label(p), 7.0) + 48.0).fold(0.0, f64::max);
            let w = (text_w(&t.name, 7.8) + 44.0).max(120.0).max(sw).max(pw);
            let pills = !t.states.is_empty() || t.more_states || value_w(t) > 0.0;
            let h = match (t.parts.len(), pills) {
                (0, false) => 58.0,
                (0, true) => 96.0,
                (n, _) => PARTS_TOP + n as f64 * PART_ROW + if pills { 48.0 } else { 10.0 },
            };
            (w, h)
        }
    }
}

/// The pill standing for suppressed states.
pub const MORE: &str = "…";
/// Where a semi-folded object's part list starts (below its name), and each row's height.
pub const PARTS_TOP: f64 = 42.0;
pub const PART_ROW: f64 = 20.0;

/// "4 Wheels", "Engine".
pub fn part_label(p: &crate::Part) -> String {
    if p.multiplicity.is_empty() { p.name.clone() } else { format!("{} {}", p.multiplicity, p.name) }
}

/// Center line of a thing's name: centered, or near the top when states, parts or a value sit below it
/// (a process with a duration: a little above its center).
pub fn label_y(t: &Thing) -> f64 {
    let (_, h) = size(t);
    if t.kind == Kind::Process {
        return t.y + h / 2.0 - if duration_label(t).is_some() { 8.0 } else { 0.0 };
    }
    if t.states.is_empty() && !t.more_states && t.parts.is_empty() && value_w(t) == 0.0 { t.y + h / 2.0 } else { t.y + 24.0 }
}

/// Drawn under a process's name: "1–20 ms (~10)", "≤ 20 ms", "10 ms".
pub fn duration_label(t: &Thing) -> Option<String> {
    let d = t.duration.as_ref()?;
    let u = &d.unit;
    let range = match (d.min, d.max) {
        (Some(a), Some(b)) => Some(format!("{a}–{b} {u}")),
        (Some(a), None) => Some(format!("≥ {a} {u}")),
        (None, Some(b)) => Some(format!("≤ {b} {u}")),
        (None, None) => None,
    };
    match (range, d.expected) {
        (Some(r), Some(e)) => Some(format!("{r} (~{e})")),
        (Some(r), None) => Some(r),
        (None, Some(e)) => Some(format!("{e} {u}")),
        (None, None) => None,
    }
}

/// An object's value pill: "60 ms", "0–50 ms", "≥ 5 Hz", "≤ 50 ms".
pub fn value_label(t: &Thing) -> Option<String> {
    let q = t.measure.as_ref()?;
    let u = if q.unit.is_empty() { String::new() } else { format!(" {}", q.unit) };
    Some(match (q.value, q.min, q.max) {
        (Some(v), ..) => format!("{v}{u}"),
        (None, Some(a), Some(b)) => format!("{a}–{b}{u}"),
        (None, Some(a), None) => format!("≥ {a}{u}"),
        (None, None, Some(b)) => format!("≤ {b}{u}"),
        _ => return None,
    })
}

/// Whether an object's value is outside its allowed range.
pub fn value_out_of_range(t: &Thing) -> bool {
    t.measure.as_ref().is_some_and(|q| q.value.is_some_and(|v| q.min.is_some_and(|a| v < a) || q.max.is_some_and(|b| v > b)))
}

fn value_w(t: &Thing) -> f64 {
    value_label(t).map_or(0.0, |s| state_w(&s) + 8.0)
}

/// Layered (Sugiyama) layout, flowing top to bottom: inputs above the processes that use them,
/// results below, invoked processes after their invoker, parts below their whole.
/// Steps: break cycles, rank by longest path, add dummy nodes on long edges,
/// order each layer by barycenter sweeps (keeping the fewest crossings), then place x by neighbor alignment.
pub fn tidy(m: &Model) -> Vec<(Id, f64, f64)> {
    use LinkKind::*;
    let n = m.things.len();
    if n == 0 {
        return vec![];
    }
    let idx: HashMap<Id, usize> = m.things.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let is_proc = |i: usize| m.things[i].kind == Kind::Process;

    // Flow edges between things.
    let mut edges: Vec<(usize, usize)> = vec![];
    for l in &m.links {
        let (Some(&f), Some(&t)) = (idx.get(&l.from), idx.get(&l.to)) else { continue };
        // Effects point object -> process whichever way they were drawn.
        edges.push(if l.kind == Effect && is_proc(f) { (t, f) } else { (f, t) });
        // A process that changes an object into a state comes before whoever then uses the object.
        if l.kind == Effect && l.to_state.is_some() {
            let (p, o) = if is_proc(f) { (f, t) } else { (t, f) };
            for l2 in &m.links {
                let (Some(&a), Some(&b)) = (idx.get(&l2.from), idx.get(&l2.to)) else { continue };
                let user = if a == o { b } else if b == o { a } else { continue };
                if user != p && is_proc(user) && matches!(l2.kind, Consumption | Effect | Instrument | Agent) {
                    edges.push((p, user));
                }
            }
        }
    }
    edges.retain(|(a, b)| a != b);
    edges.sort();
    edges.dedup();

    // Break cycles: reverse DFS back edges.
    let mut out: Vec<Vec<usize>> = vec![vec![]; n];
    for &(a, b) in &edges {
        out[a].push(b);
    }
    let mut seen = vec![0u8; n]; // 0 new, 1 on stack, 2 done
    let mut back = vec![];
    for s in 0..n {
        if seen[s] != 0 {
            continue;
        }
        let mut stack = vec![(s, 0usize)];
        seen[s] = 1;
        while let Some(top) = stack.last_mut() {
            let (v, i) = *top;
            if let Some(&w) = out[v].get(i) {
                top.1 += 1;
                match seen[w] {
                    0 => {
                        seen[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => back.push((v, w)),
                    _ => {}
                }
            } else {
                seen[v] = 2;
                stack.pop();
            }
        }
    }
    let mut edges: Vec<(usize, usize)> = edges.into_iter().map(|e| if back.contains(&e) { (e.1, e.0) } else { e }).collect();
    edges.sort();
    edges.dedup();

    // Rank = longest path from a source (edges now form a DAG).
    let mut rank = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for &(a, b) in &edges {
            if rank[b] < rank[a] + 1 {
                rank[b] = rank[a] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // Pull pure sources (agents, instruments, raw inputs) down next to their first user.
    for v in 0..n {
        let has_preds = edges.iter().any(|e| e.1 == v);
        if let Some(r) = edges.iter().filter(|e| e.0 == v).map(|e| rank[e.1]).min() {
            if !has_preds {
                rank[v] = r - 1;
            }
        }
    }

    // Proper graph: dummy nodes on edges spanning several layers.
    struct Node {
        w: f64,
        h: f64,
        layer: usize,
    }
    let mut nodes: Vec<Node> = m
        .things
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (w, h) = size(t);
            Node { w, h, layer: rank[i] }
        })
        .collect();
    let mut pe: Vec<(usize, usize)> = vec![];
    for &(a, b) in &edges {
        let mut prev = a;
        for layer in rank[a] + 1..rank[b] {
            nodes.push(Node { w: 24.0, h: 0.0, layer });
            pe.push((prev, nodes.len() - 1));
            prev = nodes.len() - 1;
        }
        pe.push((prev, b));
    }
    let layers_n = nodes.iter().map(|v| v.layer).max().unwrap_or(0) + 1;
    let mut layers: Vec<Vec<usize>> = vec![vec![]; layers_n];
    for (i, v) in nodes.iter().enumerate() {
        layers[v.layer].push(i);
    }
    let mut preds: Vec<Vec<usize>> = vec![vec![]; nodes.len()];
    let mut succs: Vec<Vec<usize>> = vec![vec![]; nodes.len()];
    for &(a, b) in &pe {
        succs[a].push(b);
        preds[b].push(a);
    }

    // Crossing minimization: alternate barycenter sweeps, keep the best ordering seen.
    let crossings = |layers: &[Vec<usize>]| -> usize {
        let mut pos = vec![0usize; nodes.len()];
        for l in layers {
            for (i, &v) in l.iter().enumerate() {
                pos[v] = i;
            }
        }
        // ponytail: O(E²) pair count per layer gap; switch to an accumulator-tree count for huge models.
        let mut c = 0;
        for l in 0..layers.len().saturating_sub(1) {
            let es: Vec<(usize, usize)> = layers[l].iter().flat_map(|&u| succs[u].iter().map(move |&v| (u, v))).collect();
            for (i, &(a, b)) in es.iter().enumerate() {
                for &(a2, b2) in &es[i + 1..] {
                    if (pos[a] < pos[a2] && pos[b] > pos[b2]) || (pos[a] > pos[a2] && pos[b] < pos[b2]) {
                        c += 1;
                    }
                }
            }
        }
        c
    };
    let mut best = layers.clone();
    let mut best_c = crossings(&layers);
    for iter in 0..24 {
        if best_c == 0 {
            break;
        }
        let down = iter % 2 == 0;
        let order: Vec<usize> = if down { (1..layers_n).collect() } else { (0..layers_n.saturating_sub(1)).rev().collect() };
        for l in order {
            let other = if down { l - 1 } else { l + 1 };
            let pos: HashMap<usize, f64> = layers[other].iter().enumerate().map(|(i, &v)| (v, i as f64)).collect();
            let mut keyed: Vec<(f64, usize)> = layers[l]
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let nb = if down { &preds[v] } else { &succs[v] };
                    let ps: Vec<f64> = nb.iter().filter_map(|u| pos.get(u)).copied().collect();
                    (if ps.is_empty() { i as f64 } else { ps.iter().sum::<f64>() / ps.len() as f64 }, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            layers[l] = keyed.into_iter().map(|k| k.1).collect();
        }
        let c = crossings(&layers);
        if c < best_c {
            (best, best_c) = (layers.clone(), c);
        }
    }
    let layers = best;

    // x: layers packed left to right, then nodes pulled toward their neighbors' centers.
    // Averaging a left-packed and a right-packed placement keeps both the order and the spacing.
    const GAP: f64 = 56.0;
    let mut cx = vec![0.0f64; nodes.len()];
    for l in &layers {
        let mut x = 0.0;
        for &v in l {
            cx[v] = x + nodes[v].w / 2.0;
            x += nodes[v].w + GAP;
        }
    }
    for iter in 0..12 {
        let order: Vec<usize> = if iter % 2 == 0 { (0..layers_n).collect() } else { (0..layers_n).rev().collect() };
        for li in order {
            let l = &layers[li];
            let want: Vec<f64> = l
                .iter()
                .map(|&v| {
                    let nb: Vec<f64> = preds[v].iter().chain(&succs[v]).map(|&u| cx[u]).collect();
                    if nb.is_empty() { cx[v] } else { nb.iter().sum::<f64>() / nb.len() as f64 }
                })
                .collect();
            let k = l.len();
            let space = |i: usize| (nodes[l[i]].w + nodes[l[i + 1]].w) / 2.0 + GAP;
            let mut left = want.clone();
            for i in 1..k {
                left[i] = left[i].max(left[i - 1] + space(i - 1));
            }
            let mut right = want.clone();
            for i in (0..k.saturating_sub(1)).rev() {
                right[i] = right[i].min(right[i + 1] - space(i));
            }
            for i in 0..k {
                cx[l[i]] = (left[i] + right[i]) / 2.0;
            }
        }
    }

    // y: one band per layer.
    let min_x = (0..n).map(|v| cx[v] - nodes[v].w / 2.0).fold(f64::MAX, f64::min);
    let mut placed = vec![];
    let mut y = 60.0;
    for l in &layers {
        let h = l.iter().map(|&v| nodes[v].h).fold(0.0, f64::max);
        for &v in l.iter().filter(|&&v| v < n) {
            let x = cx[v] - nodes[v].w / 2.0 - min_x + 60.0;
            placed.push((m.things[v].id, x.round(), (y + (h - nodes[v].h) / 2.0).round()));
        }
        y += h.max(20.0) + 110.0;
    }
    placed
}

const FRAME_PAD: f64 = 28.0;
const FRAME_LABEL: f64 = 34.0;

/// The in-zoomed process drawn around its inside boxes `(x, y, (w, h))`: an ellipse containing them,
/// with room for its name on top. With nothing inside yet: a default size at `at`.
pub fn frame_box(inner: &[(f64, f64, (f64, f64))], at: (f64, f64)) -> (f64, f64, f64, f64) {
    if inner.is_empty() {
        return (at.0, at.1, 320.0, 220.0);
    }
    let x0 = inner.iter().map(|b| b.0).fold(f64::MAX, f64::min) - FRAME_PAD;
    let y0 = inner.iter().map(|b| b.1).fold(f64::MAX, f64::min) - FRAME_PAD;
    let x1 = inner.iter().map(|b| b.0 + b.2 .0).fold(f64::MIN, f64::max) + FRAME_PAD;
    let y1 = inner.iter().map(|b| b.1 + b.2 .1).fold(f64::MIN, f64::max) + FRAME_PAD;
    // An ellipse through the corners of a w×h box has semi-axes w/√2, h/√2.
    let (w, h) = (((x1 - x0) * SQRT_2).ceil(), ((y1 - y0 + FRAME_LABEL) * SQRT_2).ceil());
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0 - FRAME_LABEL / 2.0);
    ((cx - w / 2.0).floor(), (cy - h / 2.0).floor(), w, h)
}

/// Center line of the frame's name, between its top and its inside.
pub fn frame_label_y(y: f64, h: f64) -> f64 {
    y + h / 2.0 - h / (2.0 * SQRT_2) + FRAME_LABEL / 2.0
}

/// Full layout of one diagram: layered for ordinary ones, a timeline for in-zoom ones.
pub fn tidy_opd(m: &Model, opd: Id) -> Vec<(Id, f64, f64)> {
    let v = m.view(opd);
    match v.things.iter().find(|t| t.frame.is_some()) {
        Some(f) => zoom_tidy(&v, f.id),
        None => tidy(&v),
    }
}

/// Automatic arranging of one diagram after an edit. While the layout owns every position there
/// (`Thing::auto`), it is a full `tidy_opd`. Once the user has placed things by hand everything stays
/// put except the automatic things in `touched` (just added or linked), which are fitted in next to
/// their linked neighbors on the nearest free spot.
pub fn arrange_opd(m: &Model, opd: Id, touched: &[Id]) -> Vec<(Id, f64, f64)> {
    let v = m.view(opd);
    if v.things.iter().filter(|t| t.frame.is_none()).all(|t| t.auto) {
        return tidy_opd(m, opd);
    }
    arrange(&v, touched)
}

/// Producer → user order between processes: invocations, and "yields/changes X" before "uses X".
fn sequence(v: &Model, procs: &[&Thing]) -> Vec<Id> {
    use LinkKind::*;
    let is_proc = |id: Id| v.thing(id).is_some_and(|t| t.kind == Kind::Process);
    let mut produces = vec![];
    let mut uses = vec![];
    let mut edges = vec![];
    for l in &v.links {
        let (p, o) = if is_proc(l.from) { (l.from, l.to) } else { (l.to, l.from) };
        match l.kind {
            Invocation | Overtime | Undertime if l.from != l.to => edges.push((l.from, l.to)),
            Result => produces.push((p, o)),
            Effect if l.to_state.is_some() => produces.push((p, o)),
            Consumption | Instrument | Agent | Effect => uses.push((p, o)),
            _ => {}
        }
    }
    for &(a, o) in &produces {
        edges.extend(uses.iter().filter(|u| u.1 == o && u.0 != a && !produces.contains(&(u.0, o))).map(|u| (a, u.0)));
    }
    let n = procs.len();
    let mut rank: HashMap<Id, usize> = procs.iter().map(|t| (t.id, 0)).collect();
    for _ in 0..n {
        for &(a, b) in &edges {
            if let (Some(&ra), Some(&rb)) = (rank.get(&a), rank.get(&b)) {
                if rb < ra + 1 && ra + 1 < n {
                    rank.insert(b, ra + 1);
                }
            }
        }
    }
    let mut order: Vec<&&Thing> = procs.iter().collect();
    order.sort_by(|a, b| rank[&a.id].cmp(&rank[&b.id]).then(a.y.total_cmp(&b.y)).then(a.id.cmp(&b.id)));
    order.into_iter().map(|t| t.id).collect()
}

/// Stacks boxes `(id, wanted top, h)` in a column without overlaps, keeping their order by wanted top.
fn column(mut items: Vec<(Id, f64, f64)>, gap: f64) -> Vec<(Id, f64)> {
    items.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut floor = f64::MIN;
    items
        .into_iter()
        .map(|(id, top, h)| {
            let y = top.max(floor);
            floor = y + h + gap;
            (id, y)
        })
        .collect()
}

/// In-zoom diagram: subprocesses top to bottom in execution order inside the frame, internal objects
/// beside them, inputs from outside on the left and results on the right, level with what they touch.
fn zoom_tidy(v: &Model, frame: Id) -> Vec<(Id, f64, f64)> {
    use LinkKind::*;
    let procs: Vec<&Thing> = v.things.iter().filter(|t| t.inside && t.kind == Kind::Process).collect();
    let mut pos: HashMap<Id, (f64, f64)> = HashMap::new();
    let sz = |id: Id| v.thing(id).map(size).unwrap_or((0.0, 0.0));
    // Steps in sequence order; parallel subprocesses (already level) stay side by side in one row.
    let steps = v.timeline();
    let mut rows: Vec<Vec<Id>> = vec![];
    for id in sequence(v, &procs) {
        if !rows.iter().any(|r| r.contains(&id)) {
            rows.push(steps.iter().find(|s| s.contains(&id)).cloned().unwrap_or(vec![id]));
        }
    }
    let mut y = 0.0;
    let mut pw: f64 = 0.0;
    for row in rows {
        let total = row.iter().map(|&id| sz(id).0).sum::<f64>() + 60.0 * (row.len() - 1) as f64;
        let h = row.iter().map(|&id| sz(id).1).fold(0.0, f64::max);
        let mut x = -total / 2.0;
        for id in row {
            let (w, ih) = sz(id);
            pos.insert(id, (x, y + (h - ih) / 2.0));
            x += w + 60.0;
        }
        pw = pw.max(total);
        y += h + 40.0;
    }
    let center_y = |pos: &HashMap<Id, (f64, f64)>, ids: &[Id]| {
        let ys: Vec<f64> = ids.iter().filter_map(|id| pos.get(id).map(|p| p.1 + sz(*id).1 / 2.0)).collect();
        (!ys.is_empty()).then(|| ys.iter().sum::<f64>() / ys.len() as f64)
    };
    let neighbors = |id: Id| -> Vec<Id> { v.links.iter().filter_map(|l| if l.from == id { Some(l.to) } else if l.to == id { Some(l.from) } else { None }).collect() };

    // Internal objects: a column to the right of the subprocesses.
    let objs: Vec<(Id, f64, f64)> = v
        .things
        .iter()
        .filter(|t| t.inside && t.kind == Kind::Object)
        .map(|t| {
            let h = size(t).1;
            (t.id, center_y(&pos, &neighbors(t.id)).unwrap_or(y) - h / 2.0, h)
        })
        .collect();
    for (id, top) in column(objs, 24.0) {
        pos.insert(id, (pw / 2.0 + 50.0, top));
    }

    // The frame around them, then the outside columns.
    let inner: Vec<_> = pos.iter().map(|(id, p)| (p.0, p.1, sz(*id))).collect();
    let (fx, fy, fw, fh) = frame_box(&inner, (0.0, 0.0));
    let input = |id: Id| {
        v.links.iter().any(|l| {
            (l.from == id && matches!(l.kind, Consumption | Instrument | Agent | Effect)) || (l.to == id && l.kind == Effect)
        })
    };
    let (mut left, mut right) = (vec![], vec![]);
    for t in v.things.iter().filter(|t| !t.inside && t.id != frame) {
        let h = size(t).1;
        let near: Vec<Id> = neighbors(t.id);
        let cy = center_y(&pos, &near).unwrap_or(fy + fh / 2.0);
        let side = if t.kind == Kind::Object && input(t.id) { &mut left } else { &mut right };
        side.push((t.id, cy - h / 2.0, h));
    }
    for (id, top) in column(left, 28.0) {
        pos.insert(id, (fx - 90.0 - sz(id).0, top));
    }
    for (id, top) in column(right, 28.0) {
        pos.insert(id, (fx + fw + 90.0, top));
    }
    pos.insert(frame, (fx, fy));

    // Into view from (60, 60).
    let x0 = pos.values().map(|p| p.0).fold(fx, f64::min);
    let y0 = pos.values().map(|p| p.1).fold(fy, f64::min);
    pos.into_iter().map(|(id, (x, y))| (id, (x - x0 + 60.0).round(), (y - y0 + 60.0).round())).collect()
}

/// Fits the automatic things in `touched` into a diagram view (see `arrange_opd`).
pub fn arrange(m: &Model, touched: &[Id]) -> Vec<(Id, f64, f64)> {
    const MARGIN: f64 = 36.0;
    const DY: f64 = 150.0;
    let moves = |t: &Thing| t.auto && touched.contains(&t.id) && t.frame.is_none();
    // Boxes already settled: everything that stays, then each moved thing once placed. The frame of
    // an in-zoomed process is kept apart: only things outside it must avoid it.
    let mut placed: HashMap<Id, (f64, f64, f64, f64)> = m
        .things
        .iter()
        .filter(|t| !moves(t) && t.frame.is_none())
        .map(|t| {
            let (w, h) = size(t);
            (t.id, (t.x, t.y, w, h))
        })
        .collect();
    let frame = m.things.iter().find(|t| t.frame.is_some()).map(|f| {
        let inner: Vec<_> = m.things.iter().filter(|t| t.inside && !moves(t)).map(|t| (t.x, t.y, size(t))).collect();
        frame_box(&inner, (f.x, f.y))
    });
    // (upper, lower) pairs, the same flow direction tidy uses.
    let flow: Vec<(Id, Id)> = m
        .links
        .iter()
        .map(|l| match m.thing(l.from) {
            Some(f) if l.kind == LinkKind::Effect && f.kind == Kind::Process => (l.to, l.from),
            _ => (l.from, l.to),
        })
        .collect();
    let mut todo: Vec<&Thing> = m.things.iter().filter(|t| moves(t)).collect();
    let mut out = vec![];
    while !todo.is_empty() {
        // Next: the one with the most settled neighbors (ties: oldest), so chains grow out from what's fixed.
        let settled = |t: &Thing| flow.iter().filter(|&&(a, b)| (a == t.id && placed.contains_key(&b)) || (b == t.id && placed.contains_key(&a))).count();
        let i = (0..todo.len()).max_by_key(|&i| (settled(todo[i]), std::cmp::Reverse(todo[i].id))).unwrap();
        let t = todo.swap_remove(i);
        let (w, h) = size(t);
        let near: Vec<(f64, f64, bool)> = flow
            .iter()
            .filter_map(|&(a, b)| {
                let (other, below) = if a == t.id { (b, false) } else if b == t.id { (a, true) } else { return None };
                let &(x, y, ow, oh) = placed.get(&other)?;
                Some((x + ow / 2.0, y + oh / 2.0, below))
            })
            .collect();
        let avg = |f: &dyn Fn(&(f64, f64, bool)) -> f64| near.iter().map(f).sum::<f64>() / near.len() as f64;
        let inner_procs = || placed.iter().filter(|(id, _)| m.thing(**id).is_some_and(|x| x.inside && x.kind == Kind::Process)).map(|(_, b)| *b);
        let (cx, cy) = match frame {
            // In-zoom: a new subprocess goes last in the timeline; inside objects beside what they touch;
            // outside things in the left (inputs) or right column, level with what they touch.
            Some(_) if t.inside && t.kind == Kind::Process => {
                let col: Vec<_> = inner_procs().collect();
                let x = if col.is_empty() { 0.0 } else { col.iter().map(|b| b.0 + b.2 / 2.0).sum::<f64>() / col.len() as f64 };
                (x, col.iter().map(|b| b.1 + b.3).fold(0.0, f64::max) + 40.0 + h / 2.0)
            }
            Some(_) if t.inside && !near.is_empty() => (avg(&|p| p.0) + 180.0, avg(&|p| p.1)),
            Some((fx, fy, fw, fh)) if !t.inside => {
                let input = !near.is_empty() && near.iter().filter(|p| p.2).count() * 2 >= near.len();
                let x = if input && t.kind == Kind::Object { fx - 90.0 - w / 2.0 } else { fx + fw + 90.0 + w / 2.0 };
                (x, if near.is_empty() { fy + fh / 2.0 } else { avg(&|p| p.1) })
            }
            // Wanted center: averaged over settled neighbors, one band above or below each.
            _ if !near.is_empty() => (avg(&|p| p.0), avg(&|p| p.1 + if p.2 { DY } else { -DY })),
            _ => {
                // Unlinked: to the right of everything, top-aligned.
                let right = placed.values().map(|b| b.0 + b.2).fold(f64::MIN, f64::max);
                let top = placed.values().map(|b| b.1).fold(f64::MAX, f64::min);
                if placed.is_empty() { (60.0 + w / 2.0, 60.0 + h / 2.0) } else { (right + 80.0 + w / 2.0, top + h / 2.0) }
            }
        };
        // Nearest free spot, preferring sideways over up/down so a band stays a band.
        let obstacles: Vec<(f64, f64, f64, f64)> = placed.values().copied().chain(frame.filter(|_| !t.inside)).collect();
        let free = |x: f64, y: f64| {
            obstacles.iter().all(|&(bx, by, bw, bh)| x + w + MARGIN <= bx || bx + bw + MARGIN <= x || y + h + MARGIN <= by || by + bh + MARGIN <= y)
        };
        let mut spots: Vec<(f64, f64, f64)> = (-30..=30)
            .flat_map(|i| (-12..=12).map(move |j| (i as f64 * 40.0, j as f64 * 40.0)))
            .map(|(dx, dy)| (dx.abs() + 2.5 * dy.abs(), dx, dy))
            .collect();
        spots.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (x0, y0) = (cx - w / 2.0, cy - h / 2.0);
        let (x, y) = spots
            .iter()
            .map(|&(_, dx, dy)| (x0 + dx, y0 + dy))
            .find(|&(x, y)| free(x, y))
            .unwrap_or_else(|| (obstacles.iter().map(|b| b.0 + b.2).fold(x0, f64::max) + 80.0, y0));
        let (x, y) = (x.round(), y.round());
        placed.insert(t.id, (x, y, w, h));
        out.push((t.id, x, y));
    }
    out
}
