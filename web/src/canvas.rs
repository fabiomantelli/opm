//! The diagram: geometry, hit-testing, pointer/keyboard interaction, and the floating UI on the canvas.
pub use crate::routes::LinkStyle;
use crate::routes::{arc_of, clip, dist, more_box, pill_or_body, value_box, point_at, seg, shape_of, state_boxes, LinkGeo, Route, Routes};
use crate::{storage, ui, Drag, Menu, Sel, Target, Ui, P};
use leptos::{ev, prelude::*};
use opm_core::{
    layout::{duration_label, frame_label_y, label_y, part_label, size, value_label, value_out_of_range, MORE, PARTS_TOP, PART_ROW},
    opl, valid_link_kinds, Affiliation, Essence, FanKind, Id, Kind, Link, LinkKind, Model, Modifier, Op, Thing,
};
use wasm_bindgen::JsCast;

// ---------- hit-testing ----------

/// Connection ports: top, right, bottom, left.
fn ports(t: &Thing) -> [P; 4] {
    let (w, h) = size(t);
    [(t.x + w / 2.0, t.y), (t.x + w, t.y + h / 2.0), (t.x + w / 2.0, t.y + h), (t.x, t.y + h / 2.0)]
}

fn inside(t: &Thing, p: P) -> bool {
    let s = shape_of(t);
    let (dx, dy) = ((p.0 - s.cx) / s.hw, (p.1 - s.cy) / s.hh);
    if s.ellipse { dx * dx + dy * dy <= 1.0 } else { dx.abs() <= 1.0 && dy.abs() <= 1.0 }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    Port(Id),
    State(Id, Id),
    Thing(Id),
    Link(Id),
    /// A handle on the selected link.
    Handle(Id, Handle),
}

impl Hit {
    fn thing(self) -> Option<Id> {
        match self {
            Hit::Port(id) | Hit::State(id, _) | Hit::Thing(id) => Some(id),
            Hit::Link(_) | Hit::Handle(..) => None,
        }
    }
}

/// Grips on a selected link for shaping it by hand.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Handle {
    /// The `from` (false) or `to` (true) end: slides along its thing's border.
    End(bool),
    /// A point the link passes through (double-click removes it).
    Bend(usize),
    /// Between two points: dragging it adds a point there.
    Add(usize),
}

/// The handles of link `lid` and where they are. Effects drawn in two pieces have none (they stay automatic).
pub fn link_handles(m: &Model, routes: &Routes, lid: Id) -> Vec<(Handle, P)> {
    let (Some(l), Some(g)) = (m.links.iter().find(|l| l.id == lid), routes.links.get(&lid)) else { return vec![] };
    let [path] = g.pts.as_slice() else { return vec![] };
    let (p0, q) = (path[0], *path.last().unwrap());
    let bends: Vec<P> = l.route.as_ref().map(|r| r.points.clone()).unwrap_or_default();
    // Ends on a state pill stay on the pill.
    let pill = |end: Id| m.thing(end).is_some_and(|t| t.kind == Kind::Object) && (l.from_state.is_some() || l.to_state.is_some());
    let mut out = vec![];
    if !pill(l.from) {
        out.push((Handle::End(false), p0));
    }
    if !pill(l.to) {
        out.push((Handle::End(true), q));
    }
    let ctrl: Vec<f64> = std::iter::once(0.0).chain(bends.iter().map(|&b| arc_of(path, b))).chain([arc_of(path, q)]).collect();
    for (i, w) in ctrl.windows(2).enumerate() {
        if w[1] - w[0] > 36.0 {
            out.push((Handle::Add(i), point_at(path, (w[0] + w[1]) / 2.0)));
        }
    }
    out.extend(bends.into_iter().enumerate().map(|(i, b)| (Handle::Bend(i), b)));
    out
}

/// What is under `p`; ports count only on the things in `with_ports`; links only when `routes` is given.
fn hit(m: &Model, with_ports: &[Id], p: P, zoom: f64, routes: Option<&Routes>) -> Option<Hit> {
    for &id in with_ports {
        if let Some(t) = m.thing(id) {
            if ports(t).iter().any(|&c| dist(c, p) < 9.0 / zoom.min(1.0)) {
                return Some(Hit::Port(id));
            }
        }
    }
    for t in m.things.iter().rev().filter(|t| t.frame.is_none()) {
        for (sid, x, y, w, h) in state_boxes(t) {
            if p.0 >= x && p.0 <= x + w && p.1 >= y && p.1 <= y + h {
                return Some(Hit::State(t.id, sid));
            }
        }
        if inside(t, p) {
            return Some(Hit::Thing(t.id));
        }
    }
    // An in-zoomed frame is grabbed by its rim or its name, so its inside stays free for box-selecting.
    for f in m.things.iter().filter(|t| t.frame.is_some()) {
        let s = shape_of(f);
        let r = (((p.0 - s.cx) / s.hw).powi(2) + ((p.1 - s.cy) / s.hh).powi(2)).sqrt();
        let on_name = (p.1 - frame_label_y(f.y, size(f).1)).abs() < 14.0 && (p.0 - s.cx).abs() < s.hw * 0.4;
        if (r - 1.0).abs() < 10.0 / zoom / s.hw.min(s.hh) || (r < 1.0 && on_name) {
            return Some(Hit::Thing(f.id));
        }
    }
    routes?.hit(p, 7.0 / zoom).map(Hit::Link)
}

pub fn kind_label(k: &LinkKind) -> &'static str {
    match k {
        LinkKind::Agent => "Agent",
        LinkKind::Instrument => "Instrument",
        LinkKind::Consumption => "Consumption",
        LinkKind::Result => "Result",
        LinkKind::Effect => "Effect",
        LinkKind::Invocation => "Invocation",
        LinkKind::Overtime => "Overtime exception",
        LinkKind::Undertime => "Undertime exception",
        LinkKind::Aggregation => "Aggregation",
        LinkKind::Exhibition => "Exhibition",
        LinkKind::Generalization => "Generalization",
        LinkKind::Classification => "Classification",
        LinkKind::Tagged(_) => "Tagged",
    }
}

/// The AddLink op for `kind`, keeping only the states that make sense for it.
fn link_op(from: Id, to: Id, kind: LinkKind, from_state: Option<Id>, to_state: Option<Id>) -> Op {
    use LinkKind::*;
    let (fs, ts) = match kind {
        Agent | Instrument | Consumption => (from_state.or(to_state), None),
        Result => (None, to_state.or(from_state)),
        Effect => (from_state, to_state),
        _ => (None, None),
    };
    Op::AddLink { from, to, kind, from_state: fs, to_state: ts }
}

/// Clipboard payload; the marker field keeps random JSON from being pasted as a model.
#[derive(serde::Serialize, serde::Deserialize)]
struct Clip {
    opm_clip: u8,
    things: Vec<Thing>,
    links: Vec<Link>,
    #[serde(default)]
    at: Vec<(Id, f64, f64)>,
}

// ---------- interaction ----------

impl Ui {
    pub fn select(self, s: Option<Sel>) {
        self.group.set(match s {
            Some(Sel::Thing(id)) => vec![id],
            _ => vec![],
        });
        self.sel.set(s);
    }

    pub fn select_things(self, ids: Vec<Id>) {
        self.sel.set(if ids.len() == 1 { Some(Sel::Thing(ids[0])) } else { None });
        self.group.set(ids);
    }

    pub fn add_thing(self, kind: Kind, at: Option<P>, then: impl FnOnce(Id) + 'static) {
        let (Some(m), Some(full)) = (self.model.get_untracked(), self.full.get_untracked()) else { return };
        let base = if kind == Kind::Object { "Object" } else { "Processing" };
        let name = (1..)
            .map(|i| if i == 1 { base.to_string() } else { format!("{base} {i}") })
            .find(|n| full.find(n).is_none())
            .unwrap();
        let probe = Thing {
            id: 0,
            name: name.clone(),
            kind,
            essence: Essence::Informatical,
            affiliation: Affiliation::Systemic,
            states: vec![],
            duration: None,
            measure: None,
            x: 0.0,
            y: 0.0,
            auto: false,
            frame: None,
            inside: false,
            detail: None,
            state_order: vec![],
            more_states: false,
            parts: vec![],
        };
        let (w, h) = size(&probe);
        let (mut x, mut y) = at.unwrap_or_else(|| {
            let (px, py, z) = self.view.get_untracked();
            let (cw, ch) = self.svg_rect().map(|r| (r.width(), r.height())).unwrap_or((800.0, 600.0));
            ((cw / 2.0 - px) / z, (ch / 2.0 - py) / z)
        });
        (x, y) = ((x - w / 2.0).round(), (y - h / 2.0).round());
        while m.things.iter().any(|t| (t.x - x).abs() < 20.0 && (t.y - y).abs() < 20.0) {
            (x, y) = (x + 28.0, y + 28.0);
        }
        // In an in-zoom diagram, dropped inside the frame = part of the zoomed process.
        let within = m.things.iter().find(|t| t.frame.is_some()).map(|f| inside(f, (x + w / 2.0, y + h / 2.0)));
        let op = Op::AddThing {
            name,
            kind,
            essence: Essence::Informatical,
            affiliation: Affiliation::Systemic,
            opd: Some(self.here()),
            x: Some(x),
            y: Some(y),
            inside: within,
        };
        self.send(vec![op], move |ids| {
            if let Some(&id) = ids.first() {
                self.select(Some(Sel::Thing(id)));
                then(id);
            }
        });
    }

    pub fn add_state(self) {
        let (Some(Sel::Thing(id)), Some(m)) = (self.sel.get_untracked(), self.model.get_untracked()) else {
            return self.say("Select an object to add a state");
        };
        let Some(t) = m.thing(id).filter(|t| t.kind == Kind::Object) else {
            return self.say("Only objects have states");
        };
        let name = (1..).map(|i| format!("state {i}")).find(|n| !t.states.iter().any(|s| &s.name == n)).unwrap();
        self.send(vec![Op::AddState { thing: id, name }], move |ids| {
            if let Some(&sid) = ids.first() {
                self.select(Some(Sel::State(sid)));
                self.edit.set(Some(sid));
            }
        });
    }

    /// Del: a thing other diagrams also show only leaves this one; otherwise it's deleted.
    pub fn remove_selected(self) {
        let group = self.group.get_untracked();
        let ids = match self.sel.get_untracked() {
            _ if group.len() > 1 => group,
            Some(Sel::Thing(id) | Sel::State(id) | Sel::Link(id)) => vec![id],
            None => return,
        };
        let here = self.here();
        let Some((ops, hidden)) = self.full.with_untracked(|f| {
            let f = f.as_ref()?;
            let mut hidden = 0;
            let ops: Vec<Op> = ids
                .into_iter()
                .filter_map(|id| {
                    if f.thing(id).is_some() {
                        if f.opd(here).is_some_and(|o| o.refines == Some(id)) {
                            return None; // the frame of this very diagram
                        }
                        if f.shown_in(id).len() > 1 {
                            hidden += 1;
                            return Some(Op::Hide { opd: here, thing: id });
                        }
                    }
                    Some(Op::Remove { id })
                })
                .collect();
            Some((ops, hidden))
        }) else {
            return;
        };
        if ops.is_empty() {
            return self.say("This diagram details it — delete the diagram from the Diagrams list instead");
        }
        self.select(None);
        self.send(ops, |_| {});
        if hidden > 0 {
            self.say("Removed from this diagram (still in the model and its other diagrams)");
        }
    }

    pub fn delete_everywhere(self, id: Id) {
        self.select(None);
        self.send(vec![Op::Remove { id }], |_| {});
    }

    /// Opens (creating if needed) the diagram that zooms into or unfolds `id`.
    pub fn refine(self, id: Id, unfold: bool) {
        let from = Some(self.here());
        self.send(vec![Op::Refine { thing: id, unfold, from }], move |ids| {
            if let Some(&d) = ids.first() {
                self.go(d);
            }
        });
    }

    /// Out-zooming: the selected things go inside a new process with its own in-zoom diagram; the
    /// new process takes their place here, ready to be named.
    pub fn group_selected(self) {
        let (Some(full), things) = (self.full.get_untracked(), self.group.get_untracked()) else { return };
        if things.is_empty() {
            return;
        }
        let name = (1..).map(|i| if i == 1 { "Processing".to_string() } else { format!("Processing {i}") }).find(|n| full.find(n).is_none()).unwrap();
        self.send(vec![Op::Group { opd: Some(self.here()), things, name }], move |ids| {
            if let Some(&p) = ids.first() {
                self.select(Some(Sel::Thing(p)));
                self.edit.set(Some(p));
                self.say("Grouped: double-click the new process to see its subprocesses");
            }
        });
    }

    /// Shows a thing from elsewhere in the model in this diagram, at the center of the view.
    pub fn show_existing(self, id: Id) {
        let (px, py, z) = self.view.get_untracked();
        let (cw, ch) = self.svg_rect().map(|r| (r.width(), r.height())).unwrap_or((800.0, 600.0));
        let (x, y) = (((cw / 2.0 - px) / z - 60.0).round(), ((ch / 2.0 - py) / z - 30.0).round());
        let opd = self.here();
        self.send(vec![Op::Show { opd, thing: id, x: Some(x), y: Some(y), inside: None }], move |_| self.select(Some(Sel::Thing(id))));
    }

    pub fn rename(self, id: Id, name: String) {
        let Some(m) = self.model.get_untracked() else { return };
        let current = m.thing(id).map(|t| t.name.clone()).or_else(|| m.state(id).map(|(_, s)| s.name.clone()));
        if current.as_deref() != Some(name.trim()) && !name.trim().is_empty() {
            self.send(vec![Op::Rename { id, name }], |_| {});
        }
    }

    /// Moves things locally right away, then persists (one undo step). A hand-shaped link whose two ends
    /// both move goes along with them.
    fn move_things(self, moves: Vec<(Id, f64, f64)>) {
        let opd = Some(self.here());
        let mut route_ops = vec![];
        self.model.update(|m| {
            if let Some(m) = m.as_mut() {
                let delta = |id: Id| {
                    let (_, x, y) = moves.iter().find(|mv| mv.0 == id)?;
                    m.thing(id).map(|t| (x - t.x, y - t.y))
                };
                let shifts: Vec<(Id, P)> = m
                    .links
                    .iter()
                    .filter(|l| l.route.is_some() && delta(l.to).is_some())
                    .filter_map(|l| Some((l.id, delta(l.from)?)))
                    .collect();
                for (lid, (dx, dy)) in shifts {
                    if let Some(r) = m.links.iter_mut().find(|l| l.id == lid).and_then(|l| l.route.as_mut()) {
                        for p in &mut r.points {
                            (p.0, p.1) = (p.0 + dx, p.1 + dy);
                        }
                        route_ops.push(Op::SetRoute { opd, link: lid, points: r.points.clone(), from: r.from, to: r.to });
                    }
                }
                for &(id, x, y) in &moves {
                    if let Some(t) = m.things.iter_mut().find(|t| t.id == id) {
                        (t.x, t.y, t.auto) = (x, y, false);
                    }
                }
                m.refit_frame();
            }
        });
        let mut ops: Vec<Op> = moves.into_iter().map(|(id, x, y)| Op::Move { opd, id, x, y, auto: false }).collect();
        ops.extend(route_ops);
        self.send(ops, |_| {});
    }

    /// The handle of the selected link under `p`.
    fn handle_at(self, p: P) -> Option<(Id, Handle)> {
        let Some(Sel::Link(lid)) = self.sel.get_untracked() else { return None };
        let tol = 8.0 / self.view.get_untracked().2.min(1.5);
        let routes = self.routes.get_untracked();
        self.model.with_untracked(|m| {
            let hs = link_handles(m.as_ref()?, &routes, lid);
            // Bends and ends before the add-grips between them.
            hs.iter()
                .filter(|(h, at)| !matches!(h, Handle::Add(_)) && dist(*at, p) < tol)
                .chain(hs.iter().filter(|(h, at)| matches!(h, Handle::Add(_)) && dist(*at, p) < tol))
                .next()
                .map(|(h, _)| (lid, *h))
        })
    }

    /// The link drawn automatically again in this diagram.
    pub fn reset_route(self, lid: Id) {
        self.send(vec![Op::SetRoute { opd: Some(self.here()), link: lid, points: vec![], from: None, to: None }], |_| {});
    }

    /// Shows a route being shaped, before it's saved.
    fn preview_route(self, r: &opm_core::Route) {
        self.model.update(|m| {
            if let Some(l) = m.as_mut().and_then(|m| m.links.iter_mut().find(|l| l.id == r.link)) {
                l.route = Some(r.clone());
            }
        });
    }

    /// Full layout of this diagram; the layout then owns every position again (automatic arranging may move them).
    pub fn tidy(self) {
        let Some(mut full) = self.full.get_untracked() else { return };
        let here = self.here();
        let moves = opm_core::layout::tidy_opd(&full, here);
        let ops: Vec<Op> = moves.into_iter().map(|(id, x, y)| Op::Move { opd: Some(here), id, x, y, auto: true }).collect();
        for op in ops.clone() {
            let _ = full.apply(op);
        }
        let target = full.view(here);
        let view = self.fit_view(&target);
        self.glide(target, view);
        self.send(ops, |_| {});
    }

    fn nudge(self, dx: f64, dy: f64) {
        let Some(m) = self.model.get_untracked() else { return };
        let moves = self.group.get_untracked().iter().filter_map(|&id| m.thing(id).map(|t| (id, t.x + dx, t.y + dy))).collect::<Vec<_>>();
        if !moves.is_empty() {
            self.move_things(moves);
        }
    }

    pub fn set_link_style(self, style: LinkStyle) {
        self.style.set(style);
        if let Some(s) = storage() {
            let _ = s.set_item("opm-link-style", style.key());
        }
    }

    /// Selected things plus the links among them, as clipboard JSON.
    pub fn copy_selection(self) -> Option<String> {
        let group = self.group.get_untracked();
        if group.is_empty() {
            return None;
        }
        self.model.with_untracked(|m| {
            let m = m.as_ref()?;
            let things: Vec<Thing> = m.things.iter().filter(|t| group.contains(&t.id)).cloned().collect();
            let clip = Clip {
                opm_clip: 1,
                at: things.iter().map(|t| (t.id, t.x, t.y)).collect(),
                things,
                links: m.links.iter().filter(|l| group.contains(&l.from) && group.contains(&l.to)).cloned().collect(),
            };
            serde_json::to_string(&clip).ok()
        })
    }

    /// Pastes clipboard JSON under the pointer (or offset from the originals) and selects the copies.
    pub fn paste(self, text: &str) -> bool {
        let Ok(clip) = serde_json::from_str::<Clip>(text) else { return false };
        let Some(m) = self.model.get_untracked() else { return false };
        if clip.things.is_empty() {
            return false;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for t in &clip.things {
            let (w, h) = size(t);
            let (x, y) = clip.at.iter().find(|a| a.0 == t.id).map_or((t.x, t.y), |a| (a.1, a.2));
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + w), y1.max(y + h));
        }
        let (dx, dy) = match self.pointer.get_value() {
            Some(p) => ((p.0 - (x0 + x1) / 2.0).round(), (p.1 - (y0 + y1) / 2.0).round()),
            None => (32.0, 32.0),
        };
        let op = Op::Insert { things: clip.things, links: clip.links, at: clip.at, dx, dy, opd: Some(self.here()) };
        // Same op on a copy tells which ids the server will hand out (ids are sequential).
        let mut probe = m.clone();
        let before = probe.next_id;
        if let Err(e) = probe.apply(op.clone()) {
            self.say(e);
            return true;
        }
        let new: Vec<Id> = probe.things.iter().filter(|t| t.id >= before).map(|t| t.id).collect();
        self.send(vec![op], move |_| self.select_things(new));
        true
    }

    /// Things whose ports are visible: the hovered one and the selection.
    fn port_things(self) -> Vec<Id> {
        let mut v = self.group.get_untracked();
        v.truncate(if v.len() == 1 { 1 } else { 0 });
        if let Some(id) = self.hot.get_untracked().and_then(Hit::thing) {
            v.push(id);
        }
        v
    }

    fn pointer_down(self, e: ev::PointerEvent) {
        self.menu.set(None);
        let Some(m) = self.model.get_untracked() else { return };
        let (_, p) = self.to_model(e.client_x(), e.client_y());
        let (px, py, z) = self.view.get_untracked();
        let pan = Drag::Pan { start: (e.client_x() as f64, e.client_y() as f64), pan: (px, py) };
        let drag = if e.button() == 1 || self.space.get_untracked() {
            pan
        } else if e.button() != 0 {
            return;
        } else if let Some((link, handle)) = self.handle_at(p) {
            let route = m.links.iter().find(|l| l.id == link).and_then(|l| l.route.clone()).unwrap_or(opm_core::Route { link, ..Default::default() });
            Drag::Route { link, handle, route, moved: false }
        } else {
            match hit(&m, &self.port_things(), p, z, Some(&self.routes.get_untracked())) {
                Some(Hit::Port(id)) => Drag::Link { from: id, state: None, at: p, moved: false, target: None },
                Some(Hit::State(id, sid)) => Drag::Link { from: id, state: Some(sid), at: p, moved: false, target: None },
                Some(Hit::Thing(id)) => {
                    let mut group = self.group.get_untracked();
                    if e.shift_key() {
                        if let Some(i) = group.iter().position(|&g| g == id) {
                            group.remove(i);
                        } else {
                            group.push(id);
                        }
                        return self.select_things(group);
                    }
                    if !group.contains(&id) {
                        group = vec![id];
                        self.select_things(group.clone());
                    }
                    // Dragging an in-zoomed frame carries what's inside it.
                    if group.iter().any(|g| m.thing(*g).is_some_and(|t| t.frame.is_some())) {
                        let inner: Vec<Id> = m.things.iter().filter(|t| t.inside && !group.contains(&t.id)).map(|t| t.id).collect();
                        group.extend(inner);
                    }
                    let orig = group.iter().filter_map(|&g| m.thing(g).map(|t| (g, t.x, t.y))).collect();
                    Drag::Move { start: p, orig, primary: id, snap: m.clone(), moved: false, guides: vec![] }
                }
                Some(Hit::Link(id)) => return self.select(Some(Sel::Link(id))),
                Some(Hit::Handle(..)) => return,
                None => {
                    let base = if e.shift_key() { self.group.get_untracked() } else { vec![] };
                    self.select_things(base.clone());
                    Drag::Marquee { start: p, at: p, base }
                }
            }
        };
        if let Some(svg) = self.svg.get_untracked() {
            let _ = svg.set_pointer_capture(e.pointer_id());
        }
        self.drag.set(Some(drag));
    }

    fn pointer_move(self, e: ev::PointerEvent) {
        let (_, p) = self.to_model(e.client_x(), e.client_y());
        self.pointer.set_value(Some(p));
        let z = self.view.get_untracked().2;
        let Some(mut d) = self.drag.get_untracked() else {
            // Idle: track what is under the pointer for hover affordances.
            let routes = self.routes.get_untracked();
            let ports = self.port_things();
            let h = self.handle_at(p).map(|(l, h)| Hit::Handle(l, h)).or_else(|| self.model.with_untracked(|m| m.as_ref().and_then(|m| hit(m, &ports, p, z, Some(&routes)))));
            if h != self.hot.get_untracked() {
                self.hot.set(h);
            }
            return;
        };
        match &mut d {
            Drag::Pan { start, pan } => {
                self.view.set((pan.0 + e.client_x() as f64 - start.0, pan.1 + e.client_y() as f64 - start.1, z));
                return;
            }
            Drag::Move { start, orig, primary, snap, moved, guides } => {
                let (mut dx, mut dy) = (p.0 - start.0, p.1 - start.1);
                if !*moved && dx.abs() + dy.abs() < 3.0 / z {
                    return;
                }
                *moved = true;
                // Smart guides: snap the dragged thing's edges/centers to other things'.
                let moving: Vec<Id> = orig.iter().map(|o| o.0).collect();
                let (_, ox, oy) = *orig.iter().find(|o| o.0 == *primary).unwrap();
                let (w, h) = size(snap.thing(*primary).unwrap());
                let tol = 6.0 / z;
                let (mut best_x, mut best_y): (Option<(f64, &Thing)>, Option<(f64, &Thing)>) = (None, None);
                for t in snap.things.iter().filter(|t| !moving.contains(&t.id) && t.frame.is_none()) {
                    let (tw, th) = size(t);
                    for (mine, theirs) in [(0.0, 0.0), (w / 2.0, tw / 2.0), (w, tw)] {
                        let delta = t.x + theirs - (ox + dx + mine);
                        if delta.abs() < tol && best_x.is_none_or(|b| delta.abs() < b.0.abs()) {
                            best_x = Some((delta, t));
                        }
                    }
                    for (mine, theirs) in [(0.0, 0.0), (h / 2.0, th / 2.0), (h, th)] {
                        let delta = t.y + theirs - (oy + dy + mine);
                        if delta.abs() < tol && best_y.is_none_or(|b| delta.abs() < b.0.abs()) {
                            best_y = Some((delta, t));
                        }
                    }
                }
                dx += best_x.map_or(0.0, |b| b.0);
                dy += best_y.map_or(0.0, |b| b.0);
                let (nx, ny) = (ox + dx, oy + dy);
                guides.clear();
                // Guides: through the aligned edge/center, spanning both things.
                for (snapped, t) in best_x.into_iter().map(|b| (true, b.1)).chain(best_y.into_iter().map(|b| (false, b.1))) {
                    let (tw, th) = size(t);
                    if snapped {
                        let xs = [nx, nx + w / 2.0, nx + w];
                        let x = *[t.x, t.x + tw / 2.0, t.x + tw].iter().find(|v| xs.iter().any(|x| (*x - **v).abs() < 0.5)).unwrap_or(&nx);
                        guides.push(((x, ny.min(t.y) - 12.0), (x, (ny + h).max(t.y + th) + 12.0)));
                    } else {
                        let ys = [ny, ny + h / 2.0, ny + h];
                        let y = *[t.y, t.y + th / 2.0, t.y + th].iter().find(|v| ys.iter().any(|y| (*y - **v).abs() < 0.5)).unwrap_or(&ny);
                        guides.push(((nx.min(t.x) - 12.0, y), ((nx + w).max(t.x + tw) + 12.0, y)));
                    }
                }
                let moves: Vec<_> = orig.iter().map(|&(id, x, y)| (id, (x + dx).round(), (y + dy).round())).collect();
                let (rdx, rdy) = (dx.round(), dy.round());
                self.model.update(|m| {
                    if let Some(m) = m.as_mut() {
                        for &(id, x, y) in &moves {
                            if let Some(t) = m.things.iter_mut().find(|t| t.id == id) {
                                (t.x, t.y) = (x, y);
                            }
                        }
                        // Hand-shaped links between moving things travel with them.
                        for l in m.links.iter_mut().filter(|l| moving.contains(&l.from) && moving.contains(&l.to)) {
                            let before = snap.links.iter().find(|o| o.id == l.id).and_then(|o| o.route.as_ref());
                            if let (Some(r), Some(b)) = (l.route.as_mut(), before) {
                                r.points = b.points.iter().map(|p| (p.0 + rdx, p.1 + rdy)).collect();
                            }
                        }
                        m.refit_frame();
                    }
                });
            }
            Drag::Route { link, handle, route, moved } => {
                *moved = true;
                let p = (p.0.round(), p.1.round());
                let tol = 6.0 / z;
                // Snap a point in line with its neighbors, so straight runs are easy.
                let snap_to = |p: P, near: &[P]| {
                    let mut p = p;
                    for n in near {
                        if (p.0 - n.0).abs() < tol {
                            p.0 = n.0;
                        }
                        if (p.1 - n.1).abs() < tol {
                            p.1 = n.1;
                        }
                    }
                    p
                };
                let ends = self.routes.with_untracked(|r| r.links.get(link).and_then(|g| Some((*g.pts.first()?.first()?, *g.pts.first()?.last()?))));
                let ctrl = |pts: &[P]| -> Vec<P> { ends.map(|(a, b)| std::iter::once(a).chain(pts.iter().copied()).chain([b]).collect()).unwrap_or_default() };
                match *handle {
                    Handle::Add(i) => {
                        let i = i.min(route.points.len());
                        route.points.insert(i, p);
                        *handle = Handle::Bend(i);
                    }
                    Handle::Bend(i) if i < route.points.len() => {
                        let c = ctrl(&route.points);
                        let near: Vec<P> = [c.get(i), c.get(i + 2)].into_iter().flatten().copied().collect();
                        route.points[i] = snap_to(p, &near);
                    }
                    Handle::End(to) => {
                        let lk = self.model.with_untracked(|m| m.as_ref().and_then(|m| m.links.iter().find(|l| l.id == *link).map(|l| if to { l.to } else { l.from })));
                        let t = lk.and_then(|id| self.model.with_untracked(|m| m.as_ref().and_then(|m| m.thing(id).cloned())));
                        if let Some(t) = t {
                            // The border point toward the pointer, as a fraction of the box.
                            let (w, h) = size(&t);
                            let b = clip(&shape_of(&t), p);
                            let frac = |v: f64| (v.clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
                            let a = Some((frac((b.0 - t.x) / w), frac((b.1 - t.y) / h)));
                            if to { route.to = a } else { route.from = a }
                        }
                    }
                    Handle::Bend(_) => {}
                }
                self.preview_route(route);
            }
            Drag::Link { from, at, moved, target, .. } => {
                *at = p;
                *moved = true;
                *target = self.model.with_untracked(|m| {
                    match m.as_ref().and_then(|m| hit(m, &[], p, z, None)) {
                        Some(Hit::Thing(id)) if id != *from => Some((id, None)),
                        Some(Hit::State(id, sid)) if id != *from => Some((id, Some(sid))),
                        _ => None,
                    }
                });
            }
            Drag::Marquee { start, at, base } => {
                *at = p;
                let (x0, y0, x1, y1) = (start.0.min(p.0), start.1.min(p.1), start.0.max(p.0), start.1.max(p.1));
                let mut ids = base.clone();
                self.model.with_untracked(|m| {
                    for t in m.iter().flat_map(|m| &m.things).filter(|t| t.frame.is_none()) {
                        let (w, h) = size(t);
                        if t.x < x1 && t.x + w > x0 && t.y < y1 && t.y + h > y0 && !ids.contains(&t.id) {
                            ids.push(t.id);
                        }
                    }
                });
                if ids != self.group.get_untracked() {
                    self.select_things(ids);
                }
            }
        }
        self.drag.set(Some(d));
    }

    fn pointer_up(self, e: ev::PointerEvent) {
        let Some(d) = self.drag.get_untracked() else { return };
        self.drag.set(None);
        let (local, p) = self.to_model(e.client_x(), e.client_y());
        match d {
            Drag::Pan { .. } | Drag::Marquee { .. } => {}
            Drag::Route { link, route, moved, .. } => {
                if moved {
                    self.send(vec![Op::SetRoute { opd: Some(self.here()), link, points: route.points, from: route.from, to: route.to }], |_| {});
                }
            }
            Drag::Move { orig, moved, .. } => {
                if moved {
                    let m = self.model.get_untracked().unwrap();
                    let moves = orig.iter().filter_map(|o| m.thing(o.0).map(|t| (t.id, t.x, t.y))).collect();
                    self.move_things(moves);
                }
            }
            Drag::Link { from, state, moved, target, .. } => match target {
                Some((to, to_state)) => {
                    self.menu.set(Some(Menu { from, from_state: state, target: Target::Thing(to, to_state), at: local, created: false }))
                }
                None if moved => self.menu.set(Some(Menu { from, from_state: state, target: Target::New(p), at: local, created: false })),
                None => self.select(Some(state.map_or(Sel::Thing(from), Sel::State))),
            },
        }
    }

    fn double_click(self, e: ev::MouseEvent) {
        let (_, p) = self.to_model(e.client_x(), e.client_y());
        let Some(m) = self.model.get_untracked() else { return };
        if let Some((lid, h)) = self.handle_at(p) {
            let Some(mut r) = m.links.iter().find(|l| l.id == lid).and_then(|l| l.route.clone()) else { return };
            match h {
                Handle::Bend(i) if i < r.points.len() => {
                    r.points.remove(i);
                }
                Handle::End(true) => r.to = None,
                Handle::End(false) => r.from = None,
                _ => return,
            }
            return self.send(vec![Op::SetRoute { opd: Some(self.here()), link: lid, points: r.points, from: r.from, to: r.to }], |_| {});
        }
        match hit(&m, &[], p, self.view.get_untracked().2, Some(&self.routes.get_untracked())) {
            Some(Hit::Thing(id)) => match m.thing(id).and_then(|t| t.detail) {
                Some(d) => self.go(d),
                None => self.edit.set(Some(id)),
            },
            Some(Hit::State(_, sid)) => {
                self.select(Some(Sel::State(sid)));
                self.edit.set(Some(sid));
            }
            Some(_) => {}
            None => self.add_thing(if e.shift_key() { Kind::Process } else { Kind::Object }, Some(p), move |id| self.edit.set(Some(id))),
        }
    }

    pub fn key(self, e: ev::KeyboardEvent) {
        let typing = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
            .is_some_and(|el| matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT"));
        if typing || self.search.get_untracked() || self.model.get_untracked().is_none() {
            return;
        }
        let ctrl = e.ctrl_key() || e.meta_key();
        let step = if e.shift_key() { 10.0 } else { 1.0 };
        match (ctrl, e.key().to_lowercase().as_str()) {
            (true, "z") if e.shift_key() => self.redo(),
            (true, "z") => self.undo(),
            (true, "y") => self.redo(),
            (true, "d") => {
                // Duplicate in place, slightly offset.
                if let Some(json) = self.copy_selection() {
                    let pointer = self.pointer.get_value();
                    self.pointer.set_value(None);
                    self.paste(&json);
                    self.pointer.set_value(pointer);
                }
            }
            (true, "g") => self.group_selected(),
            (true, "a") => self.select_things(self.model.with_untracked(|m| m.iter().flat_map(|m| &m.things).map(|t| t.id).collect())),
            (true, _) => return,
            (_, " ") => self.space.set(true),
            (_, "o") => self.add_thing(Kind::Object, None, move |id| self.edit.set(Some(id))),
            (_, "p") => self.add_thing(Kind::Process, None, move |id| self.edit.set(Some(id))),
            (_, "s") => self.add_state(),
            (_, "f") => self.fit(),
            (_, "t") => self.tidy(),
            (_, "arrowleft") => self.nudge(-step, 0.0),
            (_, "arrowright") => self.nudge(step, 0.0),
            (_, "arrowup") => self.nudge(0.0, -step),
            (_, "arrowdown") => self.nudge(0.0, step),
            (_, "delete" | "backspace") => self.remove_selected(),
            (_, "enter" | "f2") => {
                if let Some(Sel::Thing(id) | Sel::State(id)) = self.sel.get_untracked() {
                    self.edit.set(Some(id));
                }
            }
            (_, "/") => self.search.set(true),
            (_, "escape") => {
                self.menu.set(None);
                self.select(None);
            }
            _ => return,
        }
        e.prevent_default();
    }
}

// ---------- drawing ----------

fn markers() -> impl IntoView {
    view! {
        <defs>
            <marker id="arrow" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="12" markerHeight="12" markerUnits="userSpaceOnUse" orient="auto-start-reverse">
                <path d="M1,1.5 L11,6 L1,10.5 Z" class="mk-fill" />
            </marker>
            <marker id="open" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="12" markerHeight="12" markerUnits="userSpaceOnUse" orient="auto-start-reverse">
                <path d="M1,1.5 L11,6 L1,10.5" class="mk-line" />
            </marker>
            <marker id="dot" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="12" markerHeight="12" markerUnits="userSpaceOnUse">
                <circle cx="6" cy="6" r="5" class="mk-fill" />
            </marker>
            <marker id="ring" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="12" markerHeight="12" markerUnits="userSpaceOnUse">
                <circle cx="6" cy="6" r="4.5" class="mk-ring" />
            </marker>
        </defs>
    }
}

fn marker_ends(k: &LinkKind, pair: bool) -> (Option<&'static str>, Option<&'static str>) {
    match k {
        LinkKind::Agent => (None, Some("url(#dot)")),
        LinkKind::Instrument => (None, Some("url(#ring)")),
        LinkKind::Consumption | LinkKind::Result | LinkKind::Invocation | LinkKind::Overtime | LinkKind::Undertime => (None, Some("url(#arrow)")),
        LinkKind::Effect if pair => (None, Some("url(#arrow)")),
        LinkKind::Effect => (Some("url(#arrow)"), Some("url(#arrow)")),
        LinkKind::Tagged(_) => (None, Some("url(#open)")),
        _ => (None, None),
    }
}

/// Structural triangle in local coordinates, apex along +x.
fn symbol(k: &LinkKind) -> Option<AnyView> {
    let tri = "11,0 -8,-11 -8,11";
    Some(match k {
        LinkKind::Aggregation => view! { <polygon points=tri class="sym fill" /> }.into_any(),
        LinkKind::Exhibition => view! {
            <polygon points=tri class="sym" />
            <polygon points="4,0 -4,-5 -4,5" class="sym fill" />
        }
        .into_any(),
        LinkKind::Generalization => view! { <polygon points=tri class="sym" /> }.into_any(),
        LinkKind::Classification => view! {
            <polygon points=tri class="sym" />
            <circle cx="-1" cy="0" r="3.6" class="sym fill" />
        }
        .into_any(),
        // Time exceptions: one bar across the link (overtime), two (undertime).
        LinkKind::Overtime => view! { <path d="M0,-8 L0,8" class="tick" /> }.into_any(),
        LinkKind::Undertime => view! { <path d="M-3,-8 L-3,8 M3,-8 L3,8" class="tick" /> }.into_any(),
        _ => return None,
    })
}

/// Small notation sample for menus.
pub fn glyph(k: &LinkKind) -> impl IntoView {
    let (ms, me) = marker_ends(k, false);
    let d = Route::Line((3.0, 9.0), (39.0, 9.0)).d(*k == LinkKind::Invocation);
    let sym = symbol(k).map(|s| view! { <g transform="translate(21,9) rotate(180) scale(.62)">{s}</g> });
    view! {
        <svg class="glyph" viewBox="0 0 42 18" width="42" height="18">
            <path d=d class="link" marker-start=ms marker-end=me />
            {sym}
        </svg>
    }
}

/// The piece of a link route that ends nearest `c`, walked from that end.
fn piece_from(g: &LinkGeo, c: P) -> Option<Vec<P>> {
    let near = |p: &Vec<P>| dist(p[0], c).min(dist(*p.last().unwrap(), c));
    let mut p = g.pts.iter().filter(|p| p.len() >= 2).min_by(|a, b| near(a).total_cmp(&near(b)))?.clone();
    if dist(*p.last().unwrap(), c) < dist(p[0], c) {
        p.reverse();
    }
    Some(p)
}

/// The point `d` along `poly` from its start, moved `side` px to the left of the route.
fn along(poly: &[P], d: f64, side: f64) -> P {
    let mut left = d;
    for (i, w) in poly.windows(2).enumerate() {
        let len = dist(w[0], w[1]);
        if len > 0.0 && (left <= len || i + 2 == poly.len()) {
            let k = (left / len).min(1.0);
            let (ux, uy) = ((w[1].0 - w[0].0) / len, (w[1].1 - w[0].1) / len);
            return (w[0].0 + ux * len * k - uy * side, w[0].1 + uy * len * k + ux * side);
        }
        left -= len;
    }
    poly[0]
}

/// Where a link's small labels go: the process end (for "c"/"e") and the counted end (multiplicity).
pub struct Ends {
    process: Option<P>,
    counted: P,
}

pub fn link_ends(m: &Model, l: &Link) -> Option<Ends> {
    let center = |t: &Thing| {
        let (w, h) = size(t);
        (t.x + w / 2.0, t.y + h / 2.0)
    };
    let (f, t) = (m.thing(l.from)?, m.thing(l.to)?);
    let (process, object) = match (f.kind, t.kind) {
        (Kind::Object, Kind::Process) => (Some(center(t)), Some(center(f))),
        (Kind::Process, Kind::Object) => (Some(center(f)), Some(center(t))),
        _ => (None, None),
    };
    Some(Ends { process, counted: if l.kind.is_procedural() { object.unwrap_or(center(t)) } else { center(t) } })
}

/// XOR (one arc) or OR (two arcs) across the links of a fan, around the end they share.
pub fn fan_view(m: &Model, routes: &Routes, fan: &opm_core::Fan) -> Option<AnyView> {
    let end = m.thing(m.fan_end(fan)?)?;
    let (w, h) = size(end);
    let c = (end.x + w / 2.0, end.y + h / 2.0);
    let qs: Vec<P> = fan.links.iter().filter_map(|id| piece_from(routes.links.get(id)?, c)).map(|p| along(&p, 26.0, 0.0)).collect();
    if qs.len() < 2 {
        return None;
    }
    let r = qs.iter().map(|&q| dist(q, c)).sum::<f64>() / qs.len() as f64;
    let mut angles: Vec<f64> = qs.iter().map(|q| (q.1 - c.1).atan2(q.0 - c.0)).collect();
    angles.sort_by(f64::total_cmp);
    // The arc covers everything but the widest gap between neighboring links.
    let tau = std::f64::consts::TAU;
    let n = angles.len();
    let gap = |i: usize| (angles[(i + 1) % n] - angles[i]).rem_euclid(tau);
    let widest = (0..n).max_by(|&a, &b| gap(a).total_cmp(&gap(b)))?;
    let (a0, a1) = (angles[(widest + 1) % n], angles[widest]);
    let span = (a1 - a0).rem_euclid(tau);
    let arc = |r: f64| {
        let large = if span > std::f64::consts::PI { 1 } else { 0 };
        format!("M{},{} A{r},{r} 0 {large} 1 {},{}", c.0 + r * a0.cos(), c.1 + r * a0.sin(), c.0 + r * a1.cos(), c.1 + r * a1.sin())
    };
    let second = (fan.kind == FanKind::Or).then(|| view! { <path class="fan" d=arc(r + 5.0) /> });
    Some(view! { <g><path class="fan" d=arc(r) />{second}</g> }.into_any())
}

fn link_view(l: &Link, ends: &Ends, g: &LinkGeo, cls: &str, style: LinkStyle) -> impl IntoView {
    let kind = &l.kind;
    let (ms, me) = marker_ends(kind, g.d.len() == 2);
    let lines = g.d.iter().map(|d| view! { <path d=d.clone() class=format!("link {cls}") marker-start=ms marker-end=me /> }).collect_view();
    // Mid-link decoration: structural triangle pointing at the whole/general/class (the `from` end),
    // a lightning bolt for invocations on curved/orthogonal routes, or a tag label.
    let ((mx, my), angle) = g.mid;
    let deco = if let LinkKind::Tagged(tag) = kind {
        let angle = if angle.abs() > 90.0 { angle - 180.0 } else { angle };
        (!tag.is_empty()).then(|| {
            view! {
                <text class="tag" text-anchor="middle" transform=format!("translate({mx},{my}) rotate({angle}) translate(0,-8)")>
                    {tag.clone()}
                </text>
            }
            .into_any()
        })
    } else if *kind == LinkKind::Invocation && style != LinkStyle::Straight {
        Some(view! { <path class="bolt" d="M-7,-8 L2,-1 L-2,1 L7,8" transform=format!("translate({mx},{my}) rotate({})", angle + 90.0) /> }.into_any())
    } else {
        symbol(kind).map(|s| view! { <g transform=format!("translate({mx},{my}) rotate({angle})")>{s}</g> }.into_any())
    };
    // "c" (condition) or "e" (event) by the process end; the multiplicity by the object or part end.
    let letter = match l.modifier {
        Modifier::Condition => Some("c"),
        Modifier::Event => Some("e"),
        Modifier::None => None,
    };
    let modifier = letter.zip(ends.process).and_then(|(letter, c)| {
        let (x, y) = along(&piece_from(g, c)?, 24.0, 10.0);
        Some(view! { <text class="mod" x=x y=y text-anchor="middle" dominant-baseline="central">{letter}</text> })
    });
    let count = (!l.multiplicity.is_empty()).then(|| piece_from(g, ends.counted)).flatten().map(|p| {
        let (x, y) = along(&p, 20.0, -11.0);
        view! { <text class="mult" x=x y=y text-anchor="middle" dominant-baseline="central">{l.multiplicity.clone()}</text> }
    });
    view! { <g>{lines}{deco}{modifier}{count}</g> }
}

fn thing_view(t: &Thing, cls: &str, sel_state: Option<Id>, now: Option<Id>) -> AnyView {
    let (w, h) = size(t);
    let cls = format!(
        "thing {} {cls}{}{}",
        if t.kind == Kind::Object { "obj" } else { "proc" },
        if t.affiliation == Affiliation::Environmental { " env" } else { "" },
        if t.detail.is_some() { " refined" } else { "" },
    );
    // The in-zoomed process: a large ellipse around its subprocesses, named at the top.
    if t.frame.is_some() {
        return view! {
            <g class=format!("{cls} frame")>
                <ellipse cx=t.x + w / 2.0 cy=t.y + h / 2.0 rx=w / 2.0 ry=h / 2.0 class="body" />
                <text class="name" x=t.x + w / 2.0 y=frame_label_y(t.y, h) text-anchor="middle" dominant-baseline="central">{t.name.clone()}</text>
            </g>
        }
        .into_any();
    }
    let body = |dx: f64, class: &'static str| match t.kind {
        Kind::Object => view! { <rect x=t.x + dx y=t.y + dx width=w height=h rx="4" class=class /> }.into_any(),
        Kind::Process => view! { <ellipse cx=t.x + w / 2.0 + dx cy=t.y + h / 2.0 + dx rx=w / 2.0 ry=h / 2.0 class=class /> }.into_any(),
    };
    let label_y = label_y(t);
    // Semi-folded: parts (filled triangle) and attributes (triangle in a triangle) listed under the name.
    let parts = t
        .parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (x, y) = (t.x + 16.0, t.y + PARTS_TOP + i as f64 * PART_ROW + PART_ROW / 2.0);
            let tri = format!("M{x} {} L{} {} L{} {} Z", y - 5.0, x - 5.5, y + 4.5, x + 5.5, y + 4.5);
            view! {
                <g class="part">
                    <path d=tri class=if p.attribute { "glyph attr" } else { "glyph" } />
                    {p.attribute.then(|| view! { <path d=format!("M{x} {} L{} {} L{} {} Z", y - 1.0, x - 2.5, y + 3.0, x + 2.5, y + 3.0) class="glyph inner" /> })}
                    <text x=x + 12.0 y=y dominant-baseline="central">{part_label(p)}</text>
                </g>
            }
        })
        .collect_view();
    let value = value_box(t).zip(value_label(t)).map(|((x, y, sw, sh), label)| view! {
        <g class=if value_out_of_range(t) { "state value bad" } else { "state value" }>
            <title>{if value_out_of_range(t) { "Outside its allowed range" } else { "Its value" }}</title>
            <rect x=x y=y width=sw height=sh rx="5" />
            <text x=x + sw / 2.0 y=y + sh / 2.0 text-anchor="middle" dominant-baseline="central">{label}</text>
        </g>
    });
    let dur = duration_label(t).map(|d| view! {
        <text class="dur" x=t.x + w / 2.0 y=label_y + 18.0 text-anchor="middle" dominant-baseline="central">{d}</text>
    });
    let more = more_box(t).map(|(x, y, sw, sh)| view! {
        <g class="state more">
            <title>"More states, hidden in this diagram (see the inspector)"</title>
            <rect x=x y=y width=sw height=sh rx="12" />
            <text x=x + sw / 2.0 y=y + sh / 2.0 text-anchor="middle" dominant-baseline="central">{MORE}</text>
        </g>
    });
    let states = state_boxes(t)
        .into_iter()
        .filter_map(|b| Some((b, t.states.iter().find(|s| s.id == b.0)?)))
        .map(|((sid, x, y, sw, sh), s)| {
            let c = format!(
                "state{}{}{}{}",
                if sel_state == Some(sid) { " sel" } else { "" },
                if s.initial { " initial" } else { "" },
                if s.final_ { " final" } else { "" },
                if now == Some(sid) { " now" } else { "" },
            );
            view! {
                <g class=c>
                    <rect x=x y=y width=sw height=sh rx="12" />
                    {s.final_.then(|| view! { <rect class="inner" x=x + 3.0 y=y + 3.0 width=sw - 6.0 height=sh - 6.0 rx="9" /> })}
                    <text x=x + sw / 2.0 y=y + sh / 2.0 text-anchor="middle" dominant-baseline="central">{s.name.clone()}</text>
                </g>
            }
        })
        .collect_view();
    view! {
        <g class=cls>
            {(t.essence == Essence::Physical).then(|| body(5.0, "shadow"))}
            {body(0.0, "body")}
            <text class="name" x=t.x + w / 2.0 y=label_y text-anchor="middle" dominant-baseline="central">{t.name.clone()}</text>
            {parts}
            {states}
            {more}
            {value}
            {dur}
            {t.detail.is_some().then(|| view! { <title>"Double-click to open its detail diagram"</title> })}
        </g>
    }
    .into_any()
}

#[component]
pub fn Canvas() -> impl IntoView {
    let ui = ui();
    // Keyed lists: an item's DOM is rebuilt only when its key (everything it draws) changes,
    // so moving one thing touches that thing and its links, not the whole diagram.
    let lit = move |id: Id| ui.hover.with(|h| h.contains(&id)) || ui.hot.get().and_then(Hit::thing) == Some(id);
    let thing_items = move || {
        let group = ui.group.get();
        let fresh = ui.fresh.get();
        let sel_state = match ui.sel.get() {
            Some(Sel::State(s)) => Some(s),
            _ => None,
        };
        let target = ui.drag.with(|d| match d {
            Some(Drag::Link { target: Some((id, _)), .. }) => Some(*id),
            _ => None,
        });
        let sim = ui.sim.get();
        ui.model.with(|m| {
            m.iter()
                .flat_map(|m| &m.things)
                .map(|t| {
                    let mut cls = String::new();
                    // While simulating: objects that don't exist fade, the current state lights up,
                    // the process that just ran glows and the next one is outlined.
                    let mut now = None;
                    if let Some(sim) = &sim {
                        match t.kind {
                            Kind::Object => match sim.exists.get(&t.id) {
                                Some(s) => now = *s,
                                None => cls += " absent",
                            },
                            Kind::Process if sim.last == Some(t.id) => cls += " ran",
                            Kind::Process if sim.upcoming() == Some(t.id) && !sim.blocked => cls += " upnext",
                            Kind::Process if sim.blocked && sim.upcoming() == Some(t.id) => cls += " stuck",
                            _ => {}
                        }
                    }
                    if group.contains(&t.id) {
                        cls += " sel";
                    }
                    if lit(t.id) {
                        cls += " lit";
                    }
                    if target == Some(t.id) {
                        cls += " target";
                    }
                    if fresh.contains(&t.id) {
                        cls += " fresh";
                    }
                    let ss = sel_state.filter(|s| t.states.iter().any(|x| x.id == *s));
                    let states: Vec<String> = t.states.iter().map(|s| format!("{}:{}:{}{}", s.id, s.name, s.initial, s.final_)).collect();
                    let key = format!(
                        "{}|{}|{}|{}|{:?}|{:?}|{}|{cls}|{ss:?}|{now:?}|{:?}|{:?}|{:?}|{}|{:?}",
                        t.id,
                        t.x,
                        t.y,
                        t.name,
                        t.essence,
                        t.affiliation,
                        states.join(","),
                        t.frame,
                        t.detail,
                        t.state_order,
                        t.more_states,
                        t.parts,
                    ) + &format!("|{:?}|{:?}", t.duration, t.measure);
                    (key, t.clone(), cls, ss, now)
                })
                .collect::<Vec<_>>()
        })
    };
    let link_items = move || {
        let sel = ui.sel.get();
        let hot = ui.hot.get();
        let routes = ui.routes.get();
        ui.model.with(|m| {
            m.iter()
                .flat_map(|m| &m.links)
                .filter_map(|l| {
                    let g = routes.links.get(&l.id)?.clone();
                    let ends = link_ends(m.as_ref()?, l)?;
                    let cls = if sel == Some(Sel::Link(l.id)) {
                        "sel"
                    } else if hot == Some(Hit::Link(l.id)) || ui.hover.with(|h| h.contains(&l.id)) {
                        "lit"
                    } else {
                        ""
                    };
                    Some((format!("{}|{}|{cls}|{:?}|{}", l.id, g.rev, l.modifier, l.multiplicity), l.clone(), ends, g, cls, routes.style))
                })
                .collect::<Vec<_>>()
        })
    };
    let fans = move || {
        let routes = ui.routes.get();
        ui.model.with(|m| m.as_ref().map(|m| m.fans.iter().filter_map(|f| fan_view(m, &routes, f)).collect_view()))
    };
    let overlays = move || {
        let group = ui.group.get();
        let hot = ui.hot.get();
        let drag = ui.drag.get();
        ui.model.with(|m| {
            let m = m.as_ref()?;
            // Ports on the hovered thing and a single selection (not while dragging things around).
            let show_ports = !matches!(drag, Some(Drag::Move { .. } | Drag::Pan { .. } | Drag::Marquee { .. }));
            let mut port_ids = if show_ports && group.len() == 1 { group.clone() } else { vec![] };
            if show_ports {
                port_ids.extend(hot.and_then(Hit::thing));
            }
            if let Some(Drag::Link { from, .. }) = &drag {
                port_ids = vec![*from];
            }
            let port_views = port_ids
                .iter()
                .filter_map(|&id| m.thing(id))
                .flat_map(|t| {
                    let hot_port = hot == Some(Hit::Port(t.id));
                    ports(t).map(move |(x, y)| view! { <circle class=if hot_port { "port hot" } else { "port" } cx=x cy=y r="5" /> })
                })
                .collect_view();
            let overlay = match &drag {
                Some(Drag::Link { from, state, at, moved: true, target }) => m.thing(*from).map(|t| {
                    let src = pill_or_body(t, *state);
                    let end = target
                        .and_then(|(id, s)| m.thing(id).map(|tt| pill_or_body(tt, s)))
                        .map(|ts| seg(&src, &ts))
                        .unwrap_or_else(|| (clip(&src, *at), *at));
                    view! {
                        <line x1=end.0 .0 y1=end.0 .1 x2=end.1 .0 y2=end.1 .1 class="rubber" marker-end="url(#arrow)" />
                        {target.is_none().then(|| view! { <circle cx=at.0 cy=at.1 r="4" class="rubber-dot" /> })}
                    }
                    .into_any()
                }),
                Some(Drag::Marquee { start, at, .. }) => Some(
                    view! {
                        <rect class="marquee" x=start.0.min(at.0) y=start.1.min(at.1) width=(at.0 - start.0).abs() height=(at.1 - start.1).abs() />
                    }
                    .into_any(),
                ),
                Some(Drag::Move { guides, .. }) => Some(
                    guides
                        .iter()
                        .map(|&(a, b)| view! { <line class="guide" x1=a.0 y1=a.1 x2=b.0 y2=b.1 /> })
                        .collect_view()
                        .into_any(),
                ),
                _ => None,
            };
            // Grips on the selected link (and while it's being shaped).
            let shaping = match &drag {
                Some(Drag::Route { link, .. }) => Some(*link),
                None => match ui.sel.get() {
                    Some(Sel::Link(l)) => Some(l),
                    _ => None,
                },
                _ => None,
            };
            let grips = shaping
                .map(|lid| {
                    link_handles(m, &ui.routes.get(), lid)
                        .into_iter()
                        .map(|(h, (x, y))| match h {
                            Handle::End(_) => view! { <circle class="grip end" cx=x cy=y r="5" /> }.into_any(),
                            Handle::Bend(_) => view! { <rect class="grip bend" x=x - 5.0 y=y - 5.0 width="10" height="10" rx="2" /> }.into_any(),
                            Handle::Add(_) => view! { <circle class="grip add" cx=x cy=y r="4.5" /> }.into_any(),
                        })
                        .collect_view()
                });
            Some(view! { {port_views}{overlay}{grips} })
        })
    };
    let transform = move || {
        let (px, py, z) = ui.view.get();
        format!("translate({px},{py}) scale({z})")
    };
    let grid = move || {
        let (px, py, z) = ui.view.get();
        format!("background-size:{s}px {s}px;background-position:{px}px {py}px", s = 24.0 * z)
    };
    let empty = move || ui.model.with(|m| m.as_ref().is_some_and(|m| m.things.is_empty()));
    let cursor = move || {
        let c = match (ui.drag.get(), ui.hot.get()) {
            (Some(Drag::Pan { .. }), _) => "grabbing",
            (Some(Drag::Link { .. }), _) => "linking",
            (Some(Drag::Move { .. }), _) => "moving",
            (None, _) if ui.space.get() => "grab",
            (None, Some(Hit::Port(_) | Hit::State(..))) => "linking",
            (None, Some(Hit::Thing(_))) => "movable",
            (None, Some(Hit::Link(_))) => "pointing",
            (None, Some(Hit::Handle(..))) => "movable",
            (Some(Drag::Route { .. }), _) => "moving",
            _ => "",
        };
        format!("canvas {c}")
    };
    view! {
        <div class=cursor style=grid>
            <svg
                node_ref=ui.svg
                on:pointerdown=move |e| ui.pointer_down(e)
                on:pointermove=move |e| ui.pointer_move(e)
                on:pointerup=move |e| ui.pointer_up(e)
                on:pointerleave=move |_| {
                    ui.pointer.set_value(None);
                    if ui.drag.get_untracked().is_none() { ui.hot.set(None) }
                }
                on:dblclick=move |e| ui.double_click(e)
                on:wheel=move |e: ev::WheelEvent| {
                    e.prevent_default();
                    let (local, _) = ui.to_model(e.client_x(), e.client_y());
                    if e.ctrl_key() || e.delta_mode() != 0 || e.delta_y().abs() > 40.0 {
                        ui.zoom_by((-e.delta_y() * 0.0025).exp(), local);
                    } else {
                        let (px, py, z) = ui.view.get_untracked();
                        ui.view.set((px - e.delta_x(), py - e.delta_y(), z));
                    }
                }
            >
                {markers()}
                <g transform=transform>
                    <g>
                        <For each=thing_items key=|i| i.0.clone() children=|(_, t, cls, ss, now)| thing_view(&t, &cls, ss, now) />
                    </g>
                    // Links on top so state-specific links visibly touch their state pill.
                    <g>
                        <For each=link_items key=|i| i.0.clone() children=|(_, l, ends, g, cls, style)| link_view(&l, &ends, &g, cls, style) />
                    </g>
                    <g>{fans}</g>
                    {overlays}
                </g>
            </svg>
            <Show when=empty>
                <div class="empty">
                    <svg viewBox="0 0 120 50" width="120" height="50">
                        <rect x="2" y="8" width="46" height="32" rx="3" class="ghost obj" />
                        <ellipse cx="88" cy="24" rx="28" ry="18" class="ghost proc" />
                    </svg>
                    <h2>"Start with a thing"</h2>
                    <p><kbd>"O"</kbd>" object · "<kbd>"P"</kbd>" process · double-click the canvas · or ask ChatGPT"</p>
                    <p class="muted">"Hover a thing and drag one of its dots to link it — drop on empty space to create and link."</p>
                </div>
            </Show>
            <Crumbs />
            <crate::SimBar />
            <ContextBar />
            <RenameBox />
            <LinkMenu />
            <div class="hints">
                <span><kbd>"Space"</kbd>" drag to pan"</span>
                <span><kbd>"Shift"</kbd>" multi-select"</span>
                <span><kbd>"T"</kbd>" tidy"</span>
            </div>
            <div class="styles" role="group" aria-label="Link style">
                {[(LinkStyle::Straight, "Straight links", "M5 19 19 5"), (LinkStyle::Curved, "Curved links", "M5 19C5 10 19 14 19 5"), (LinkStyle::Orthogonal, "Right-angle links", "M5 19v-7h14V5")]
                    .into_iter()
                    .map(|(st, title, d)| view! {
                        <button title=title class:on=move || ui.style.get() == st on:click=move |_| ui.set_link_style(st)>
                            <svg viewBox="0 0 24 24"><path d=d class="ic line" /></svg>
                        </button>
                    })
                    .collect_view()}
            </div>
            <div class="zoom">
                <button title="Zoom out" on:click=move |_| ui.zoom_by(1.0 / 1.2, center(ui))>"−"</button>
                <button class="pct" title="Fit (F)" on:click=move |_| ui.fit()>
                    {move || format!("{:.0}%", ui.view.get().2 * 100.0)}
                </button>
                <button title="Zoom in" on:click=move |_| ui.zoom_by(1.2, center(ui))>"+"</button>
            </div>
        </div>
    }
}

fn center(ui: Ui) -> P {
    ui.svg_rect().map(|r| (r.width() / 2.0, r.height() / 2.0)).unwrap_or_default()
}

fn to_screen(ui: Ui, p: P) -> P {
    let (px, py, z) = ui.view.get();
    (p.0 * z + px, p.1 * z + py)
}

/// Floating toolbar above the selection: quick edits where the eye already is.
#[component]
fn ContextBar() -> impl IntoView {
    let ui = ui();
    move || {
        if ui.drag.get().is_some() || ui.menu.get().is_some() || ui.edit.get().is_some() {
            return None;
        }
        let group = ui.group.get();
        let sel = ui.sel.get();
        let m = ui.model.get()?;
        let bar = |at: P, inner: AnyView| {
            let (x, y) = to_screen(ui, at);
            view! {
                <div class="ctx" style=format!("left:{x}px;top:{y}px") on:pointerdown=|e| e.stop_propagation()>
                    {inner}
                </div>
            }
            .into_any()
        };
        if group.len() > 1 {
            let (mut x0, mut x1, mut y0) = (f64::MAX, f64::MIN, f64::MAX);
            for t in group.iter().filter_map(|&id| m.thing(id)) {
                let (w, _) = size(t);
                (x0, x1, y0) = (x0.min(t.x), x1.max(t.x + w), y0.min(t.y));
            }
            return Some(bar(
                ((x0 + x1) / 2.0, y0),
                view! {
                    <span class="ctx-label">{group.len()} " selected"</span>
                    {group.iter().any(|&id| m.thing(id).is_some_and(|t| t.kind == Kind::Process && t.frame.is_none())).then(|| view! {
                        <button class="ctx-btn" title="Group into a new process with its own in-zoom diagram (Ctrl+G)" on:click=move |_| ui.group_selected()>
                            <svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="10" ry="8" class="ic line" /><ellipse cx="9" cy="10" rx="3.5" ry="2" class="ic line" /><ellipse cx="15" cy="14.5" rx="3.5" ry="2" class="ic line" /></svg>
                        </button>
                    })}
                    <button class="ctx-btn danger-ic" title="Delete (Del)" on:click=move |_| ui.remove_selected()>{icon_trash()}</button>
                }
                .into_any(),
            ));
        }
        match sel? {
            Sel::Thing(id) => {
                let t = m.thing(id)?;
                let (w, _) = size(t);
                let phys = t.essence == Essence::Physical;
                let env = t.affiliation == Affiliation::Environmental;
                let is_obj = t.kind == Kind::Object;
                let refine = match (t.frame.is_some(), t.detail) {
                    (true, _) => None,
                    (false, Some(d)) => Some(
                        view! {
                            <span class="ctx-sep"></span>
                            <button class="ctx-btn" title="Open its detail diagram (double-click)" on:click=move |_| ui.go(d)>
                                <svg viewBox="0 0 24 24"><path d="M14 4h6v6M20 4l-8 8M18 14v6H4V6h6" class="ic line" /></svg>
                            </button>
                        }
                        .into_any(),
                    ),
                    (false, None) => Some(
                        view! {
                            <span class="ctx-sep"></span>
                            {(!is_obj).then(|| view! {
                                <button class="ctx-btn" title="Zoom in: a detail diagram with its subprocesses" on:click=move |_| ui.refine(id, false)>
                                    <svg viewBox="0 0 24 24"><circle cx="10.5" cy="10.5" r="6.5" class="ic line" /><path d="M15.5 15.5 21 21M10.5 7.5v6M7.5 10.5h6" class="ic line" /></svg>
                                </button>
                            })}
                            <button class="ctx-btn" title="Unfold: a detail diagram with its parts and attributes" on:click=move |_| ui.refine(id, true)>
                                <svg viewBox="0 0 24 24"><rect x="8" y="3" width="8" height="6" rx="1" class="ic line" /><rect x="3" y="15" width="7" height="6" rx="1" class="ic line" /><rect x="14" y="15" width="7" height="6" rx="1" class="ic line" /><path d="M12 9v3M6.5 15v-3h11v3" class="ic line" /></svg>
                            </button>
                        }
                        .into_any(),
                    ),
                };
                Some(bar(
                    (t.x + w / 2.0, t.y),
                    view! {
                        <button class="ctx-btn" title="Rename (Enter)" on:click=move |_| ui.edit.set(Some(id))>
                            <svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4" class="ic line" /></svg>
                        </button>
                        <span class="ctx-sep"></span>
                        <button class="ctx-btn" class:on=phys title="Physical (shadow)"
                            on:click=move |_| ui.send(vec![Op::Update { id, essence: Some(if phys { Essence::Informatical } else { Essence::Physical }), affiliation: None }], |_| {})>
                            <svg viewBox="0 0 24 24"><rect x="7" y="7" width="13" height="11" rx="1.5" class="ic fillmuted" /><rect x="4" y="4" width="13" height="11" rx="1.5" class="ic line bg" /></svg>
                        </button>
                        <button class="ctx-btn" class:on=env title="Environmental (dashed)"
                            on:click=move |_| ui.send(vec![Op::Update { id, essence: None, affiliation: Some(if env { Affiliation::Systemic } else { Affiliation::Environmental }) }], |_| {})>
                            <svg viewBox="0 0 24 24"><rect x="4" y="6" width="16" height="12" rx="1.5" class="ic line" stroke-dasharray="3 2.5" /></svg>
                        </button>
                        {is_obj.then(|| view! {
                            <button class="ctx-btn" title="Add state (S)" on:click=move |_| ui.add_state()>
                                <svg viewBox="0 0 24 24"><rect x="3" y="8" width="13" height="8" rx="4" class="ic state" /><path d="M19 9v6M16 12h6" class="ic line" /></svg>
                            </button>
                        })}
                        {(!is_obj).then(|| {
                            let looped = m.links.iter().find(|l| l.from == id && l.to == id).map(|l| l.id);
                            view! {
                                <button class="ctx-btn" class:on=looped.is_some() title="Loop: it invokes itself (repeats while its conditions hold)"
                                    on:click=move |_| ui.send(vec![match looped {
                                        Some(lid) => Op::Remove { id: lid },
                                        None => Op::AddLink { from: id, to: id, kind: LinkKind::Invocation, from_state: None, to_state: None },
                                    }], |_| {})>
                                    <svg viewBox="0 0 24 24"><path d="M19 12a7 7 0 1 1-2.1-5" class="ic line" /><path d="M17.5 2.5v4.5H13" class="ic line" /></svg>
                                </button>
                            }
                        })}
                        {refine}
                        <span class="ctx-sep"></span>
                        <button class="ctx-btn danger-ic" title="Delete (Del)" on:click=move |_| ui.remove_selected()>{icon_trash()}</button>
                    }
                    .into_any(),
                ))
            }
            Sel::Link(lid) => {
                let l = m.links.iter().find(|l| l.id == lid)?.clone();
                // Above the whole link, so it never covers the grips used to shape it.
                let (((mx, _), _), top) = ui.routes.get().links.get(&lid).map(|g| (g.mid, g.bbox.1))?;
                let (f, t) = (m.thing(l.from)?, m.thing(l.to)?);
                let kinds = valid_link_kinds(f.kind, t.kind)
                    .into_iter()
                    .map(|k| {
                        let active = std::mem::discriminant(&k) == std::mem::discriminant(&l.kind);
                        let l = l.clone();
                        let title = kind_label(&k);
                        view! {
                            <button class="ctx-btn" class:on=active title=title
                                on:click=move |_| if !active {
                                    let op = link_op(l.from, l.to, k.clone(), l.from_state, l.to_state);
                                    ui.send(vec![Op::Remove { id: l.id }, op], move |ids| {
                                        if let Some(&id) = ids.get(1) { ui.select(Some(Sel::Link(id))) }
                                    });
                                }>
                                {glyph(&k)}
                            </button>
                        }
                    })
                    .collect_view();
                let reset = l.route.is_some().then(|| view! {
                    <span class="ctx-sep"></span>
                    <button class="ctx-btn" title="Reset route: draw it automatically again" on:click=move |_| ui.reset_route(lid)>
                        <svg viewBox="0 0 24 24"><path d="M4 12a8 8 0 1 0 2.3-5.6M4 4v4.5h4.5" class="ic line" /></svg>
                    </button>
                });
                Some(bar(
                    (mx, top - 6.0),
                    view! {
                        {kinds}
                        {reset}
                        <span class="ctx-sep"></span>
                        <button class="ctx-btn danger-ic" title="Delete (Del)" on:click=move |_| ui.remove_selected()>{icon_trash()}</button>
                    }
                    .into_any(),
                ))
            }
            Sel::State(_) => None,
        }
    }
}

/// Where this diagram sits in the tree: SD › SD1 Making › SD1.1 Brewing.
#[component]
fn Crumbs() -> impl IntoView {
    let ui = ui();
    move || {
        ui.opd.track();
        let here = ui.here();
        ui.full.with(|f| {
            let f = f.as_ref()?;
            let mut chain = vec![];
            let mut at = Some(here);
            while let Some(o) = at {
                chain.push(o);
                at = f.opd(o).and_then(|o| o.parent);
            }
            chain.reverse();
            let n = chain.len();
            let items = chain
                .into_iter()
                .enumerate()
                .map(|(i, o)| {
                    let label = f.title(o);
                    view! {
                        {(i > 0).then(|| view! { <span class="sep">"›"</span> })}
                        <button class:here=i + 1 == n on:click=move |_| ui.go(o)>{label}</button>
                    }
                })
                .collect_view();
            Some(view! { <nav class="crumbs" on:pointerdown=|e| e.stop_propagation()>{items}</nav> })
        })
    }
}

fn icon_trash() -> impl IntoView {
    view! { <svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" class="ic line" /></svg> }
}

#[component]
fn RenameBox() -> impl IntoView {
    let ui = ui();
    let input: NodeRef<leptos::html::Input> = NodeRef::new();
    Effect::new(move |_| {
        if let Some(i) = input.get() {
            let _ = i.focus();
            i.select();
        }
    });
    let commit = move |save: bool| {
        let Some(id) = ui.edit.get_untracked() else { return };
        ui.edit.set(None);
        if save {
            if let Some(i) = input.get_untracked() {
                ui.rename(id, i.value());
            }
        }
    };
    move || {
        let id = ui.edit.get()?;
        let (px, py, z) = ui.view.get_untracked();
        let (name, (cx, cy), w) = ui.model.with_untracked(|m| {
            let m = m.as_ref()?;
            if let Some(t) = m.thing(id) {
                let (w, _) = size(t);
                Some((t.name.clone(), (t.x + w / 2.0, label_y(t)), w))
            } else {
                let (t, s) = m.state(id)?;
                let b = state_boxes(t).into_iter().find(|b| b.0 == id)?;
                Some((s.name.clone(), (b.1 + b.3 / 2.0, b.2 + b.4 / 2.0), b.3 + 40.0))
            }
        })?;
        let style = format!(
            "left:{}px;top:{}px;width:{}px;transform:translate(-50%,-50%) scale({z})",
            cx * z + px,
            cy * z + py,
            w.max(140.0)
        );
        Some(view! {
            <input
                class="rename"
                node_ref=input
                style=style
                prop:value=name
                on:keydown=move |e| match e.key().as_str() {
                    "Enter" => commit(true),
                    "Escape" => commit(false),
                    _ => {}
                }
                on:blur=move |_| commit(true)
            />
        })
    }
}

/// After a link drag: pick the link kind (with its OPL preview), or create a thing where it was dropped.
#[component]
fn LinkMenu() -> impl IntoView {
    let ui = ui();
    move || {
        let menu = ui.menu.get()?;
        let m = ui.model.get_untracked()?;
        let f = m.thing(menu.from)?;
        // Open toward the inside of the canvas so it never gets clipped.
        let (cw, ch) = ui.svg_rect().map(|r| (r.width(), r.height())).unwrap_or((1e4, 1e4));
        let (x, y) = menu.at;
        let style = format!(
            "{};{}",
            if x + 340.0 > cw { format!("right:{}px", cw - x + 10.0) } else { format!("left:{}px", x + 10.0) },
            if y > ch * 0.55 { format!("bottom:{}px", ch - y + 10.0) } else { format!("top:{}px", y + 10.0) },
        );
        let body = match menu.target {
            Target::New(p) => {
                let pick = move |kind: Kind| {
                    let menu = menu.clone();
                    move |_| {
                        let menu = menu.clone();
                        ui.add_thing(kind, Some(p), move |id| {
                            ui.menu.set(Some(Menu { target: Target::Thing(id, None), created: true, ..menu }));
                        });
                    }
                };
                view! {
                    <div class="menu-title">"Create & link from " {f.name.clone()}</div>
                    <div class="create-row">
                        <button class="create" on:click=pick(Kind::Object)>
                            <svg viewBox="0 0 24 24"><rect x="3" y="6" width="18" height="12" rx="1.5" class="ic obj" /></svg>
                            "Object"
                        </button>
                        <button class="create" on:click=pick(Kind::Process)>
                            <svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="9.5" ry="6.5" class="ic proc" /></svg>
                            "Process"
                        </button>
                    </div>
                }
                .into_any()
            }
            Target::Thing(to, to_state) => {
                let t = m.thing(to)?;
                let options = valid_link_kinds(f.kind, t.kind)
                    .into_iter()
                    .map(|kind| {
                        let op = link_op(menu.from, to, kind.clone(), menu.from_state, to_state);
                        // Preview the exact OPL sentence this link would add.
                        let mut probe = m.clone();
                        let preview = probe.apply(op.clone()).map(|id| {
                            opl::render(&probe).into_iter().find(|s| s.refs.contains(&id)).map(|s| s.text).unwrap_or_default()
                        });
                        let ok = preview.is_ok();
                        let text = preview.unwrap_or_else(|e| e);
                        let created = menu.created;
                        view! {
                            <button
                                class="opt"
                                disabled=!ok
                                on:click=move |_| {
                                    ui.menu.set(None);
                                    ui.send(vec![op.clone()], move |_| {
                                        if created {
                                            ui.select(Some(Sel::Thing(to)));
                                            ui.edit.set(Some(to));
                                        }
                                    });
                                }
                            >
                                {glyph(&kind)}
                                <span class="opt-text">
                                    <span class="opt-kind">{kind_label(&kind)}</span>
                                    <span class="opt-opl">{text}</span>
                                </span>
                            </button>
                        }
                    })
                    .collect_view();
                view! {
                    <div class="menu-title">{f.name.clone()} " → " {t.name.clone()}</div>
                    {options}
                }
                .into_any()
            }
        };
        Some(view! {
            <div class="menu" style=style on:pointerdown=|e| e.stop_propagation()>
                {body}
            </div>
        })
    }
}
