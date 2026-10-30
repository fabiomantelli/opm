//! OPM Studio editor: SVG canvas + inspector + live OPL, synced with the server over WebSocket.
use futures::StreamExt;
use gloo_net::{http::Request, websocket::{futures::WebSocket, Message}};
use gloo_timers::future::TimeoutFuture;
use leptos::{ev, prelude::*, task::spawn_local};
use canvas::{kind_label, Canvas};
use opm_core::{layout::size, opl, Affiliation, Essence, FanKind, Id, Kind, Model, Modifier, Op};

mod canvas;
mod files;
mod routes;

pub type P = (f64, f64);

// ---------- OPL highlighting ----------

/// Splits OPL text into plain runs and thing names (longest name wins, whole words only).
fn name_runs(text: &str, m: &Model) -> Vec<(String, Option<Kind>)> {
    let mut names: Vec<_> = m.things.iter().map(|t| (t.name.as_str(), t.kind)).collect();
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
    let (mut out, mut plain, mut i) = (vec![], String::new(), 0);
    while i < text.len() {
        let found = names.iter().find(|(n, _)| {
            text[i..].starts_with(n) && !word(text[..i].chars().last()) && !word(text[i + n.len()..].chars().next())
        });
        if let Some((n, k)) = found {
            if !plain.is_empty() {
                out.push((std::mem::take(&mut plain), None));
            }
            out.push((n.to_string(), Some(*k)));
            i += n.len();
        } else {
            let c = text[i..].chars().next().unwrap();
            plain.push(c);
            i += c.len_utf8();
        }
    }
    if !plain.is_empty() {
        out.push((plain, None));
    }
    out
}

// ---------- state ----------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sel {
    Thing(Id),
    State(Id),
    Link(Id),
}

#[derive(Clone)]
pub enum Drag {
    Pan { start: P, pan: P },
    /// `orig`: start positions of every thing being moved; `primary` is the one under the pointer.
    Move { start: P, orig: Vec<(Id, f64, f64)>, primary: Id, snap: Model, moved: bool, guides: Vec<(P, P)> },
    Link { from: Id, state: Option<Id>, at: P, moved: bool, target: Option<(Id, Option<Id>)> },
    Marquee { start: P, at: P, base: Vec<Id> },
}

#[derive(Clone)]
pub enum Target {
    Thing(Id, Option<Id>),
    /// Dropped on empty canvas at this model point.
    New(P),
}

#[derive(Clone)]
pub struct Menu {
    from: Id,
    from_state: Option<Id>,
    target: Target,
    /// Canvas-local position.
    at: P,
    /// The target was just created from this menu (rename it after linking).
    created: bool,
}

#[derive(Clone, Copy)]
pub struct Ui {
    mid: RwSignal<Option<String>>,
    /// The whole model as the server has it.
    full: RwSignal<Option<Model>>,
    /// Diagram asked for in the URL (0 or unknown = the root).
    opd: RwSignal<Id>,
    /// Diagram drawn in `model` right now.
    showing: StoredValue<Id>,
    /// The current diagram's view of `full` (see `Model::view`): what the canvas draws and edits.
    model: RwSignal<Option<Model>>,
    models: RwSignal<Vec<(String, String)>>,
    /// Single selection (inspector); a selected thing is also in `group`.
    sel: RwSignal<Option<Sel>>,
    /// All selected things (multi-select).
    group: RwSignal<Vec<Id>>,
    /// What the pointer is over while idle.
    hot: RwSignal<Option<canvas::Hit>>,
    /// Space held: drag pans.
    space: RwSignal<bool>,
    style: RwSignal<routes::LinkStyle>,
    /// All link routes, recomputed once per model/style change and shared by drawing and hit-testing.
    routes: Memo<std::sync::Arc<routes::Routes>>,
    /// Last pointer position over the canvas (model coordinates), for paste.
    pointer: StoredValue<Option<P>>,
    drag: RwSignal<Option<Drag>>,
    /// pan x, pan y, zoom
    view: RwSignal<(f64, f64, f64)>,
    menu: RwSignal<Option<Menu>>,
    edit: RwSignal<Option<Id>>,
    hover: RwSignal<Vec<Id>>,
    toast: RwSignal<Option<String>>,
    live: RwSignal<bool>,
    chatgpt: RwSignal<bool>,
    /// The Ctrl+K finder is open.
    search: RwSignal<bool>,
    /// A simulation of the diagram on screen, while one runs.
    sim: RwSignal<Option<opm_core::sim::Sim>>,
    /// Stepping by itself.
    playing: RwSignal<bool>,
    past: StoredValue<Vec<Model>>,
    future: StoredValue<Vec<Model>>,
    fitted: StoredValue<bool>,
    /// Bumped to cancel a running glide.
    anim: StoredValue<u64>,
    /// Things that just appeared, highlighted for a moment.
    fresh: RwSignal<Vec<Id>>,
    svg: NodeRef<leptos::svg::Svg>,
}

pub fn ui() -> Ui {
    expect_context()
}

fn e2s(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn fetch_json<T: serde::de::DeserializeOwned>(req: Result<Request, gloo_net::Error>) -> Result<T, String> {
    let r = req.map_err(e2s)?.send().await.map_err(e2s)?;
    if r.ok() {
        r.json().await.map_err(e2s)
    } else {
        Err(r.text().await.unwrap_or_else(|_| format!("HTTP {}", r.status())))
    }
}

async fn list_models() -> Vec<(String, String)> {
    fetch_json(Request::get("/api/models").build()).await.unwrap_or_default()
}

pub fn storage() -> Option<web_sys::Storage> {
    window().local_storage().ok().flatten()
}

fn set_theme(dark: bool) {
    let _ = document().document_element().unwrap().set_attribute("data-theme", if dark { "dark" } else { "light" });
    if let Some(s) = storage() {
        let _ = s.set_item("opm-theme", if dark { "dark" } else { "light" });
    }
}

fn is_dark() -> bool {
    match document().document_element().unwrap().get_attribute("data-theme").as_deref() {
        Some("dark") => true,
        Some("light") => false,
        _ => window().match_media("(prefers-color-scheme: dark)").ok().flatten().is_some_and(|q| q.matches()),
    }
}

impl Ui {
    fn say(self, msg: impl Into<String>) {
        let msg = msg.into();
        self.toast.set(Some(msg.clone()));
        spawn_local(async move {
            TimeoutFuture::new(3500).await;
            if self.toast.get_untracked().as_deref() == Some(msg.as_str()) {
                self.toast.set(None);
            }
        });
    }

    fn svg_rect(self) -> Option<web_sys::DomRect> {
        self.svg.get_untracked().map(|s| s.get_bounding_client_rect())
    }

    /// Client coordinates -> (canvas-local, model) coordinates.
    fn to_model(self, cx: i32, cy: i32) -> (P, P) {
        let r = self.svg_rect();
        let (l, t) = r.map(|r| (r.left(), r.top())).unwrap_or_default();
        let (px, py, z) = self.view.get_untracked();
        let local = (cx as f64 - l, cy as f64 - t);
        (local, ((local.0 - px) / z, (local.1 - py) / z))
    }

    /// The diagram on screen.
    pub fn here(self) -> Id {
        let opd = self.opd.get_untracked();
        self.full.with_untracked(|f| f.as_ref().map_or(0, |f| if f.opd(opd).is_some() { opd } else { f.root() }))
    }

    /// Opens diagram `opd` (through the URL, so Back works).
    pub fn go(self, opd: Id) {
        let (Some(mid), Some(root)) = (self.mid.get_untracked(), self.full.with_untracked(|f| f.as_ref().map(|f| f.root()))) else { return };
        let _ = window().location().set_hash(&if opd == root { mid } else { format!("{mid}/{opd}") });
    }

    /// Shows the current diagram of `full` without animation, framed.
    fn show_here(self) {
        let here = self.here();
        let Some(v) = self.full.with_untracked(|f| f.as_ref().map(|f| f.view(here))) else { return };
        self.anim.update_value(|g| *g += 1);
        self.showing.set_value(here);
        self.model.set(Some(v));
        self.sim.set(None);
        self.playing.set(false);
        self.select(None);
        self.menu.set(None);
        self.fit();
    }

    /// Records the model for undo, posts the ops; the server's WebSocket echo updates the view.
    fn send(self, ops: Vec<Op>, then: impl FnOnce(Vec<Id>) + 'static) {
        let (Some(mid), Some(snap)) = (self.mid.get_untracked(), self.full.get_untracked()) else { return };
        self.past.update_value(|p| {
            p.push(snap.clone());
            if p.len() > 200 {
                p.remove(0);
            }
        });
        self.future.update_value(|f| f.clear());
        spawn_local(async move {
            let req = Request::post(&format!("/api/models/{mid}/ops")).json(&ops);
            match fetch_json::<Vec<Id>>(req).await {
                Ok(ids) => then(ids),
                Err(e) => {
                    self.past.update_value(|p| {
                        p.pop();
                    });
                    self.model.set(Some(snap.view(self.here())));
                    self.full.set(Some(snap));
                    self.say(e);
                }
            }
        });
    }

    fn restore(self, from: StoredValue<Vec<Model>>, to: StoredValue<Vec<Model>>) {
        let (Some(mid), Some(cur)) = (self.mid.get_untracked(), self.full.get_untracked()) else { return };
        let mut stack = from.get_value();
        let Some(prev) = stack.pop() else { return };
        from.set_value(stack);
        to.update_value(|s| s.push(cur));
        self.full.set(Some(prev.clone()));
        self.glide(prev.view(self.here()), None);
        self.select(None);
        spawn_local(async move {
            let res = match Request::put(&format!("/api/models/{mid}")).json(&prev) {
                Ok(r) => match r.send().await {
                    Ok(r) if r.ok() => Ok(()),
                    Ok(r) => Err(r.text().await.unwrap_or_default()),
                    Err(e) => Err(e2s(e)),
                },
                Err(e) => Err(e2s(e)),
            };
            if let Err(e) = res {
                self.say(e);
            }
        });
    }

    fn undo(self) {
        self.restore(self.past, self.future);
    }

    fn redo(self) {
        self.restore(self.future, self.past);
    }

    fn fit(self) {
        if let Some(v) = self.model.with_untracked(|m| m.as_ref().and_then(|m| self.fit_view(m))) {
            self.view.set(v);
        }
    }

    /// The view (pan x, pan y, zoom) that shows all of `m`.
    fn fit_view(self, m: &Model) -> Option<(f64, f64, f64)> {
        let r = self.svg_rect()?;
        if m.things.is_empty() {
            return Some((0.0, 0.0, 1.0));
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for t in &m.things {
            let (w, h) = size(t);
            (x0, y0, x1, y1) = (x0.min(t.x), y0.min(t.y), x1.max(t.x + w), y1.max(t.y + h));
        }
        let z = ((r.width() - 120.0) / (x1 - x0)).min((r.height() - 120.0) / (y1 - y0)).clamp(0.2, 1.4);
        let px = r.width() / 2.0 - (x0 + x1) / 2.0 * z;
        let py = r.height() / 2.0 - (y0 + y1) / 2.0 * z;
        Some((px, py, z))
    }

    /// Shows `target`, sliding moved things (and the view, when given) into place instead of jumping,
    /// so it's easy to follow what changed.
    fn glide(self, target: Model, view: Option<(f64, f64, f64)>) {
        let gen = self.anim.get_value() + 1;
        self.anim.set_value(gen);
        let starts: std::collections::HashMap<Id, P> =
            self.model.with_untracked(|m| m.iter().flat_map(|m| &m.things).map(|t| (t.id, (t.x, t.y))).collect());
        let moved = target.things.iter().any(|t| starts.get(&t.id).is_some_and(|&(x, y)| (x - t.x).abs() + (y - t.y).abs() > 1.0));
        // Big models just jump: every frame re-routes all links.
        let slide = moved && target.things.len() <= 150;
        if !slide {
            self.model.set(Some(target.clone()));
            if view.is_none() {
                return;
            }
        }
        let v0 = self.view.get_untracked();
        spawn_local(async move {
            let Some(perf) = window().performance() else { return };
            let t0 = perf.now();
            loop {
                // A newer glide takes over; a drag the user started skips to the end.
                if self.anim.get_value() != gen {
                    return;
                }
                if self.drag.with_untracked(|d| d.is_some()) {
                    if slide {
                        self.model.set(Some(target));
                    }
                    return;
                }
                let k = ((perf.now() - t0) / 320.0).min(1.0);
                let e = 1.0 - (1.0 - k).powi(3);
                let lerp = |a: f64, b: f64| a + (b - a) * e;
                if slide {
                    let mut m = target.clone();
                    for t in &mut m.things {
                        if let Some(&(x, y)) = starts.get(&t.id) {
                            (t.x, t.y) = (lerp(x, t.x), lerp(y, t.y));
                        }
                    }
                    m.refit_frame();
                    self.model.set(Some(m));
                }
                if let Some((px, py, z)) = view {
                    self.view.set((lerp(v0.0, px), lerp(v0.1, py), lerp(v0.2, z)));
                }
                if k >= 1.0 {
                    return;
                }
                TimeoutFuture::new(16).await;
            }
        });
    }

    /// A model update from the server: another tab, ChatGPT, Claude, or the echo of our own edit.
    fn receive(self, full: Model) {
        self.full.set(Some(full));
        let here = self.here();
        // First load, or a diagram that only just arrived (opened right after creating it).
        if !self.fitted.get_value() || self.showing.get_value() != here {
            self.fitted.set_value(true);
            return self.show_here();
        }
        let m = self.full.with_untracked(|f| f.as_ref().map(|f| f.view(here))).unwrap();
        let (Some(r), (px, py, z)) = (self.svg_rect(), self.view.get_untracked()) else { return self.model.set(Some(m)) };
        let (new, out_of_view) = self.model.with_untracked(|prev| {
            let before: std::collections::HashMap<Id, P> = prev.iter().flat_map(|p| &p.things).map(|t| (t.id, (t.x, t.y))).collect();
            let changed: Vec<_> = m.things.iter().filter(|t| before.get(&t.id) != Some(&(t.x, t.y))).collect();
            let hidden = changed.iter().any(|t| {
                let (w, h) = size(t);
                let (sx, sy) = (px + t.x * z, py + t.y * z);
                sx < 0.0 || sy < 0.0 || sx + w * z > r.width() || sy + h * z > r.height()
            });
            let new: Vec<Id> = changed.iter().filter(|t| !before.contains_key(&t.id)).map(|t| t.id).collect();
            (new, hidden)
        });
        // Frame the whole model only when something that just changed landed off-screen.
        let view = if out_of_view { self.fit_view(&m) } else { None };
        self.glide(m, view);
        if !new.is_empty() {
            self.fresh.update(|f| f.extend(&new));
            spawn_local(async move {
                TimeoutFuture::new(1600).await;
                self.fresh.update(|f| f.retain(|id| !new.contains(id)));
            });
        }
    }

    /// Opens diagram `opd`, selects `id` there and centers it, with a short glow.
    pub fn focus(self, opd: Id, id: Id) {
        if self.here() != opd {
            self.go(opd);
        }
        spawn_local(async move {
            // The diagram arrives through the hash change; wait for it.
            for _ in 0..80 {
                if self.showing.get_value() == opd && self.model.with_untracked(|m| m.as_ref().is_some_and(|m| m.thing(id).is_some())) {
                    break;
                }
                TimeoutFuture::new(25).await;
            }
            let Some(t) = self.model.with_untracked(|m| m.as_ref().and_then(|m| m.thing(id).cloned())) else { return };
            let Some(r) = self.svg_rect() else { return };
            let (w, h) = size(&t);
            let z = self.view.get_untracked().2.max(0.8);
            self.view.set((r.width() / 2.0 - (t.x + w / 2.0) * z, r.height() / 2.0 - (t.y + h / 2.0) * z, z));
            self.select(Some(Sel::Thing(id)));
            self.fresh.update(|f| f.push(id));
            TimeoutFuture::new(1600).await;
            self.fresh.update(|f| f.retain(|x| *x != id));
        });
    }

    pub fn simulate(self) {
        let here = self.here();
        let sim = self.full.with_untracked(|f| f.as_ref().map(|f| opm_core::sim::Sim::new(f, here)));
        self.playing.set(false);
        self.select(None);
        self.sim.set(sim);
    }

    /// Runs the next process; false when the run is over.
    fn sim_step(self) -> bool {
        let Some(mut sim) = self.sim.get_untracked() else { return false };
        let more = self.full.with_untracked(|f| f.as_ref().is_some_and(|f| sim.step(f)));
        self.sim.set(Some(sim));
        more
    }

    fn sim_play(self) {
        if self.playing.get_untracked() {
            return self.playing.set(false);
        }
        self.playing.set(true);
        spawn_local(async move {
            while self.playing.get_untracked() && self.sim_step() {
                TimeoutFuture::new(900).await;
            }
            self.playing.set(false);
        });
    }

    fn zoom_by(self, f: f64, around: P) {
        let (px, py, z) = self.view.get_untracked();
        let nz = (z * f).clamp(0.2, 3.0);
        let k = nz / z;
        self.view.set((around.0 - (around.0 - px) * k, around.1 - (around.1 - py) * k, nz));
    }

}

/// Keeps the editor in sync with the server for model `mid` (reconnects until the model changes).
fn connect(ui: Ui, mid: String) {
    spawn_local(async move {
        let url = {
            let loc = window().location();
            let proto = if loc.protocol().unwrap_or_default() == "https:" { "wss" } else { "ws" };
            format!("{proto}://{}/ws/{mid}", loc.host().unwrap_or_default())
        };
        loop {
            if ui.mid.get_untracked().as_deref() != Some(mid.as_str()) {
                return;
            }
            if let Ok(mut ws) = WebSocket::open(&url) {
                while let Some(msg) = ws.next().await {
                    if ui.mid.get_untracked().as_deref() != Some(mid.as_str()) {
                        return;
                    }
                    let Ok(Message::Text(s)) = msg else { break };
                    let Ok(m) = serde_json::from_str::<Model>(&s) else { continue };
                    ui.live.set(true);
                    // Don't fight a drag in progress.
                    if !matches!(ui.drag.get_untracked(), Some(Drag::Move { .. })) {
                        ui.receive(m);
                    }
                }
            }
            ui.live.set(false);
            TimeoutFuture::new(1500).await;
        }
    });
}

/// "#model" or "#model/diagram".
fn hash_parts() -> (Option<String>, Id) {
    let h = window().location().hash().unwrap_or_default();
    let h = h.trim_start_matches('#');
    let (mid, opd) = h.split_once('/').unwrap_or((h, ""));
    ((!mid.is_empty()).then(|| mid.to_string()), opd.parse().unwrap_or(0))
}

// ---------- panels ----------

#[component]
fn Inspector() -> impl IntoView {
    let ui = ui();
    move || {
        let sel = ui.sel.get()?;
        ui.model.with(|m| {
            let m = m.as_ref()?;
            Some(match sel {
                Sel::Thing(id) => {
                    let t = m.thing(id)?.clone();
                    let is_obj = t.kind == Kind::Object;
                    let phys = t.essence == Essence::Physical;
                    let env = t.affiliation == Affiliation::Environmental;
                    let states = t
                        .states
                        .iter()
                        .map(|s| {
                            let sid = s.id;
                            view! {
                                <span class="chip state-chip">
                                    <span on:dblclick=move |_| { ui.select(Some(Sel::State(sid))); ui.edit.set(Some(sid)); }>{s.name.clone()}</span>
                                    <button title="Remove state" on:click=move |_| ui.send(vec![Op::Remove { id: sid }], |_| {})>"×"</button>
                                </span>
                            }
                        })
                        .collect_view();
                    let here = ui.here();
                    let (shown, zoom_here) = ui.full.with(|f| {
                        let f = f.as_ref()?;
                        let shown: Vec<(Id, String)> = f.shown_in(id).into_iter().map(|o| (o, f.label(o))).collect();
                        Some((shown, f.opd(here).is_some_and(|o| o.refines.is_some() && !o.unfold)))
                    })?;
                    let elsewhere = shown.len() > 1;
                    let is_frame = t.frame.is_some();
                    let inside = t.inside;
                    let set_inside = move |v: bool| ui.send(vec![Op::Show { opd: here, thing: id, x: None, y: None, inside: Some(v) }], |_| {});
                    let diagrams = shown
                        .into_iter()
                        .map(|(o, label)| view! { <button class="chip link-chip" class:on=o == here on:click=move |_| ui.go(o)>{label}</button> })
                        .collect_view();
                    let satisfies = {
                        let mine: Vec<(Id, String, String)> = m.satisfies(id).into_iter().map(|r| (r.id, r.key.clone(), r.text.clone())).collect();
                        let others: Vec<(Id, String)> = m.requirements.iter().filter(|r| !r.satisfied_by.contains(&id)).map(|r| (r.id, format!("{} {}", r.key, r.text))).collect();
                        let chips = mine
                            .into_iter()
                            .map(|(rid, key, text)| view! {
                                <span class="chip req-chip" title=text>
                                    {key}
                                    <button title="No longer satisfies it" on:click=move |_| ui.send(vec![Op::Trace { requirement: rid, thing: id, satisfies: false }], |_| {})>"×"</button>
                                </span>
                            })
                            .collect_view();
                        let picker = (!others.is_empty()).then(|| view! {
                            <select class="chip add" on:change=move |e| {
                                if let Ok(rid) = event_target_value(&e).parse::<Id>() {
                                    ui.send(vec![Op::Trace { requirement: rid, thing: id, satisfies: true }], |_| {});
                                }
                            }>
                                <option value="" selected>"+ requirement"</option>
                                {others.into_iter().map(|(rid, label)| view! { <option value=rid.to_string()>{label}</option> }).collect_view()}
                            </select>
                        });
                        let none = (m.requirements.is_empty()).then(|| view! { <span class="muted small">"Add requirements in the Requirements panel."</span> });
                        view! { {chips}{picker}{none} }
                    };
                    let refine = match t.detail {
                        Some(d) => view! { <button class="ghost-btn" on:click=move |_| ui.go(d)>"Open detail diagram"</button> }.into_any(),
                        None if is_frame => ().into_any(),
                        None => view! {
                            {(!is_obj).then(|| view! { <button class="ghost-btn" title="A detail diagram with its subprocesses" on:click=move |_| ui.refine(id, false)>"Zoom in"</button> })}
                            <button class="ghost-btn" title="A detail diagram with its parts and attributes" on:click=move |_| ui.refine(id, true)>"Unfold"</button>
                        }
                        .into_any(),
                    };
                    view! {
                        <section class="inspector">
                            <div class="eyebrow">
                                <span class=if is_obj { "dot obj" } else { "dot proc" }></span>
                                {if is_obj { "Object" } else { "Process" }}
                            </div>
                            <input
                                class="title-input"
                                prop:value=t.name.clone()
                                on:change=move |e| ui.rename(id, event_target_value(&e))
                            />
                            <div class="seg">
                                <button class:on=!phys on:click=move |_| ui.send(vec![Op::Update { id, essence: Some(Essence::Informatical), affiliation: None }], |_| {})>"Informatical"</button>
                                <button class:on=phys on:click=move |_| ui.send(vec![Op::Update { id, essence: Some(Essence::Physical), affiliation: None }], |_| {})>"Physical"</button>
                            </div>
                            <div class="seg">
                                <button class:on=!env on:click=move |_| ui.send(vec![Op::Update { id, essence: None, affiliation: Some(Affiliation::Systemic) }], |_| {})>"Systemic"</button>
                                <button class:on=env on:click=move |_| ui.send(vec![Op::Update { id, essence: None, affiliation: Some(Affiliation::Environmental) }], |_| {})>"Environmental"</button>
                            </div>
                            {(zoom_here && !is_frame).then(|| view! {
                                <div class="seg">
                                    <button class:on=inside on:click=move |_| set_inside(true)>"Inside (part of it)"</button>
                                    <button class:on=!inside on:click=move |_| set_inside(false)>"Outside"</button>
                                </div>
                            })}
                            {is_obj.then(|| view! {
                                <div class="label">"States"</div>
                                <div class="chips">
                                    {states}
                                    <button class="chip add" on:click=move |_| ui.add_state()>"+ state"</button>
                                </div>
                            })}
                            <div class="label">"Satisfies"</div>
                            <div class="chips">{satisfies}</div>
                            <div class="label">"Shown in"</div>
                            <div class="chips">{diagrams}</div>
                            <div class="row-btns">{refine}</div>
                            {(!is_frame).then(|| view! {
                                <button class="danger" on:click=move |_| ui.remove_selected()>
                                    {if elsewhere { "Remove from this diagram".to_string() } else { format!("Delete {}", if is_obj { "object" } else { "process" }) }}
                                </button>
                            })}
                            {elsewhere.then(|| view! {
                                <button class="danger" on:click=move |_| ui.delete_everywhere(id)>"Delete from the whole model"</button>
                            })}
                        </section>
                    }
                    .into_any()
                }
                Sel::State(sid) => {
                    let (t, s) = m.state(sid)?;
                    view! {
                        <section class="inspector">
                            <div class="eyebrow"><span class="dot state"></span>"State of " {t.name.clone()}</div>
                            <input class="title-input" prop:value=s.name.clone() on:change=move |e| ui.rename(sid, event_target_value(&e)) />
                            <div class="seg">
                                <button class:on=s.initial title="The state it starts in" on:click={let v = !s.initial; move |_| ui.send(vec![Op::SetState { id: sid, initial: Some(v), final_: None }], |_| {})}>"Initial"</button>
                                <button class:on=s.final_ title="A state it ends in" on:click={let v = !s.final_; move |_| ui.send(vec![Op::SetState { id: sid, initial: None, final_: Some(v) }], |_| {})}>"Final"</button>
                            </div>
                            <p class="muted small">"Drag from a state to a process to make the link state-specific."</p>
                            <button class="danger" on:click=move |_| ui.remove_selected()>"Delete state"</button>
                        </section>
                    }
                    .into_any()
                }
                Sel::Link(lid) => {
                    let l = m.links.iter().find(|l| l.id == lid)?.clone();
                    let sentence = opl::render(m).into_iter().find(|s| s.refs.contains(&lid)).map(|s| s.text).unwrap_or_default();
                    let (f, t) = (m.thing(l.from)?, m.thing(l.to)?);
                    // State pickers for procedural links touching an object with states.
                    let obj = match (f.kind, t.kind) {
                        (Kind::Object, Kind::Process) => Some(f),
                        (Kind::Process, Kind::Object) => Some(t),
                        _ => None,
                    }
                    .filter(|o| l.kind.is_procedural() && !o.states.is_empty());
                    let label = kind_label(&l.kind);
                    let (cur_in, cur_out) = (l.from_state, l.to_state);
                    let pickers = obj.map(|o| {
                        let opts = |cur: Option<Id>| {
                            let mut v = vec![view! { <option value="" selected=cur.is_none()>"(any)"</option> }.into_any()];
                            v.extend(o.states.iter().map(|s| view! { <option value=s.id.to_string() selected=cur == Some(s.id)>{s.name.clone()}</option> }.into_any()));
                            v
                        };
                        let relink = move |from_state: Option<Option<Id>>, to_state: Option<Option<Id>>| {
                            let states = Some((from_state.unwrap_or(cur_in), to_state.unwrap_or(cur_out)));
                            ui.send(vec![Op::SetLink { id: lid, states, modifier: None, multiplicity: None }], |_| {});
                        };
                        let relink2 = relink;
                        let parse = |v: String| v.parse::<Id>().ok();
                        view! {
                            <label class="label">"Input state"</label>
                            <select on:change=move |e| relink(Some(parse(event_target_value(&e))), None)>{opts(cur_in)}</select>
                            <label class="label">"Output state"</label>
                            <select on:change=move |e| relink2(None, Some(parse(event_target_value(&e))))>{opts(cur_out)}</select>
                        }
                    });
                    let set = move |modifier: Option<Modifier>, multiplicity: Option<String>| {
                        ui.send(vec![Op::SetLink { id: lid, states: None, modifier, multiplicity }], |_| {});
                    };
                    let between = f.kind != t.kind;
                    let modifier = (l.modifiable() && between).then(|| {
                        let cur = l.modifier;
                        let opt = move |m: Modifier, text: &'static str, tip: &'static str| view! {
                            <button class:on=cur == m title=tip on:click=move |_| set(Some(m), None)>{text}</button>
                        };
                        view! {
                            <div class="label">"Trigger"</div>
                            <div class="seg">
                                {opt(Modifier::None, "Normal", "Always part of the process")}
                                {opt(Modifier::Condition, "Condition (c)", "The process runs only if this object exists (in that state); otherwise it is skipped")}
                                {opt(Modifier::Event, "Event (e)", "This object appearing (in that state) starts the process")}
                            </div>
                        }
                    });
                    let count = l.countable().then(|| view! {
                        <label class="label">"Multiplicity"</label>
                        <input
                            class="field-input"
                            placeholder="1   (e.g. 4, 0..1, 1..*, 2..5)"
                            prop:value=l.multiplicity.clone()
                            on:change=move |e| set(None, Some(event_target_value(&e)))
                        />
                    });
                    // XOR/OR: this link and its siblings (same kind, same shared end) as alternatives.
                    let fan = m.fan_of(lid).cloned();
                    let siblings = |end: Id, at_from: bool| -> Vec<Id> {
                        m.links
                            .iter()
                            .filter(|o| o.kind == l.kind && (if at_from { o.from == end } else { o.to == end }))
                            .map(|o| o.id)
                            .collect()
                    };
                    let (a, b) = (siblings(l.from, true), siblings(l.to, false));
                    let process_end_is_to = t.kind == Kind::Process;
                    let members = match &fan {
                        Some(fan) => fan.links.clone(),
                        None if a.len() > b.len() || (a.len() == b.len() && !process_end_is_to) => a,
                        None => b,
                    };
                    let alternatives = (l.kind.is_procedural() && members.len() >= 2).then(|| {
                        let others: Vec<String> = members
                            .iter()
                            .filter(|&&id| id != lid)
                            .filter_map(|id| m.links.iter().find(|o| o.id == *id))
                            .filter_map(|o| m.thing(if o.from == l.from { o.to } else { o.from }).map(|t| t.name.clone()))
                            .collect();
                        let cur = fan.as_ref().map(|f| f.kind);
                        let members2 = members.clone();
                        let pick = move |k: Option<FanKind>| {
                            let members = members2.clone();
                            move |_| ui.send(vec![Op::Fan { links: members.clone(), kind: k }], |_| {})
                        };
                        view! {
                            <div class="label">"Alternatives with " {others.join(", ")}</div>
                            <div class="seg">
                                <button class:on=cur.is_none() on:click=pick(None)>"All"</button>
                                <button class:on=cur == Some(FanKind::Xor) title="Exactly one of them" on:click=pick(Some(FanKind::Xor))>"Either (XOR)"</button>
                                <button class:on=cur == Some(FanKind::Or) title="At least one of them" on:click=pick(Some(FanKind::Or))>"Or (OR)"</button>
                            </div>
                        }
                    });
                    view! {
                        <section class="inspector">
                            <div class="eyebrow"><span class="dot link"></span>{label} " link"</div>
                            <p class="sentence">{sentence}</p>
                            {pickers}
                            {modifier}
                            {count}
                            {alternatives}
                            <button class="danger" on:click=move |_| ui.remove_selected()>"Delete link"</button>
                        </section>
                    }
                    .into_any()
                }
            })
        })
    }
}

/// Ctrl+K: find any thing, state or diagram in the whole model and jump to it.
#[component]
fn Search() -> impl IntoView {
    let ui = ui();
    let query = RwSignal::new(String::new());
    let active = RwSignal::new(0usize);
    let input: NodeRef<leptos::html::Input> = NodeRef::new();
    Effect::new(move |_| {
        if ui.search.get() {
            spawn_local(async move {
                TimeoutFuture::new(0).await;
                if let Some(el) = input.get_untracked() {
                    let _ = el.focus();
                }
            });
        }
    });
    // (label, detail, diagram, thing to focus)
    let results = Memo::new(move |_| {
        let q = query.get().trim().to_lowercase();
        ui.full.with(|f| {
            let Some(f) = f.as_ref() else { return vec![] };
            let here = ui.here();
            let mut out: Vec<(u8, String, String, Id, Option<Id>)> = vec![];
            let rank = |name: &str| {
                let n = name.to_lowercase();
                if q.is_empty() || n.starts_with(&q) { Some(0) } else if n.contains(&q) { Some(1) } else { None }
            };
            for t in &f.things {
                let shown = f.shown_in(t.id);
                // Prefer the diagram on screen, else where it was first drawn.
                let Some(&opd) = shown.iter().find(|&&o| o == here).or(shown.first()) else { continue };
                let labels: Vec<String> = shown.iter().map(|&o| f.label(o)).collect();
                let kind = if t.kind == Kind::Object { "Object" } else { "Process" };
                if let Some(r) = rank(&t.name) {
                    out.push((r, t.name.clone(), format!("{kind} · {}", labels.join(", ")), opd, Some(t.id)));
                }
                for st in &t.states {
                    if let Some(r) = rank(&st.name).filter(|_| !q.is_empty()) {
                        out.push((r + 1, st.name.clone(), format!("State of {} · {}", t.name, labels.join(", ")), opd, Some(t.id)));
                    }
                }
            }
            for (o, _) in f.tree() {
                if let Some(r) = rank(&f.title(o)).or_else(|| rank(&f.label(o))) {
                    out.push((r, f.title(o), "Diagram".into(), o, None));
                }
            }
            out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
            out.truncate(40);
            out.into_iter().map(|(_, a, b, c, d)| (a, b, c, d)).collect::<Vec<_>>()
        })
    });
    let close = move || {
        ui.search.set(false);
        query.set(String::new());
        active.set(0);
    };
    let choose = move |i: usize| {
        let Some((_, _, opd, thing)) = results.with_untracked(|r| r.get(i).cloned()) else { return };
        close();
        match thing {
            Some(id) => ui.focus(opd, id),
            None => ui.go(opd),
        }
    };
    let key = move |e: ev::KeyboardEvent| {
        let n = results.with_untracked(|r| r.len());
        match e.key().as_str() {
            "ArrowDown" if n > 0 => active.update(|a| *a = (*a + 1) % n),
            "ArrowUp" if n > 0 => active.update(|a| *a = (*a + n - 1) % n),
            "Enter" => choose(active.get_untracked()),
            "Escape" => close(),
            _ => return,
        }
        e.prevent_default();
    };
    move || {
        ui.search.get().then(|| view! {
            <div class="backdrop top" on:click=move |_| close()>
                <div class="modal finder" on:click=|e| e.stop_propagation()>
                    <input
                        node_ref=input
                        class="finder-input"
                        placeholder="Find a thing, state or diagram…"
                        autofocus
                        prop:value=query
                        on:input=move |e| { query.set(event_target_value(&e)); active.set(0); }
                        on:keydown=key
                    />
                    <div class="finder-list">
                        {move || {
                            let rows = results.get();
                            if rows.is_empty() {
                                return view! { <p class="muted small">"Nothing found."</p> }.into_any();
                            }
                            rows.into_iter().enumerate().map(|(i, (name, detail, _, _))| view! {
                                <button
                                    class="finder-row"
                                    class:on=move || active.get() == i
                                    on:mouseenter=move |_| active.set(i)
                                    on:click=move |_| choose(i)
                                >
                                    <span>{name}</span>
                                    <span class="muted small">{detail}</span>
                                </button>
                            }).collect_view().into_any()
                        }}
                    </div>
                </div>
            </div>
        })
    }
}

/// The diagram tree: open, or delete, a diagram.
#[component]
fn Diagrams() -> impl IntoView {
    let ui = ui();
    let confirm = RwSignal::new(None::<Id>);
    move || {
        ui.opd.track();
        let here = ui.here();
        ui.full.with(|f| {
            let f = f.as_ref()?;
            let tree = f.tree();
            let n = tree.len();
            let items = tree
                .into_iter()
                .map(|(id, depth)| {
                    let title = f.title(id);
                    let parent = f.opd(id).and_then(|o| o.parent);
                    let delete = move |_| {
                        confirm.set(None);
                        // Leaving the deleted branch: go up to its parent.
                        let inside = ui.full.with_untracked(|f| {
                            let f = f.as_ref()?;
                            let mut at = Some(ui.here());
                            while let Some(o) = at {
                                if o == id {
                                    return Some(true);
                                }
                                at = f.opd(o).and_then(|o| o.parent);
                            }
                            Some(false)
                        });
                        if inside == Some(true) {
                            ui.go(parent.unwrap_or(0));
                        }
                        ui.send(vec![Op::RemoveOpd { opd: id }], |_| {});
                    };
                    view! {
                        <li class:active=id == here style=format!("padding-left:{}px", 6 + depth * 16)>
                            <button class="tree-item" on:click=move |_| ui.go(id)>{title}</button>
                            {parent.map(|_| move || if confirm.get() == Some(id) {
                                view! {
                                    <span class="confirm">
                                        "Delete?"
                                        <button class="yes" on:click=delete>"Yes"</button>
                                        <button on:click=move |_| confirm.set(None)>"No"</button>
                                    </span>
                                }
                                .into_any()
                            } else {
                                view! { <button class="tree-x" title="Delete this diagram (and what only it shows)" on:click=move |_| confirm.set(Some(id))>"×"</button> }.into_any()
                            })}
                        </li>
                    }
                })
                .collect_view();
            Some(view! {
                <section class="panel diagrams">
                    <header><h3>"Diagrams"</h3><span class="count">{n}</span></header>
                    <ul>{items}</ul>
                </section>
            })
        })
    }
}

/// Well-formedness and in-zoom balancing, each pointing at the diagram to look at.
#[component]
fn Checks() -> impl IntoView {
    let ui = ui();
    move || {
        ui.full.with(|f| {
            let f = f.as_ref()?;
            let issues = opm_core::check::check(f);
            let n = issues.len();
            let items = issues
                .into_iter()
                .map(|i| {
                    let (opd, refs) = (i.opd, i.refs.clone());
                    view! {
                        <li
                            on:click=move |_| ui.go(opd)
                            on:mouseenter=move |_| ui.hover.set(refs.clone())
                            on:mouseleave=move |_| ui.hover.set(vec![])
                        >
                            <span class="where">{f.label(opd)}</span>
                            {i.text}
                        </li>
                    }
                })
                .collect_view();
            Some(view! {
                <section class="panel checks" class:ok=n == 0>
                    <header><h3>"Checks"</h3><span class="count">{n}</span></header>
                    {if n == 0 {
                        view! { <p class="muted small all-good">"Balanced and well-formed."</p> }.into_any()
                    } else {
                        view! { <ul>{items}</ul> }.into_any()
                    }}
                </section>
            })
        })
    }
}

/// What the system must do, each traced to the things that satisfy it.
#[component]
fn Requirements() -> impl IntoView {
    let ui = ui();
    let draft = RwSignal::new(String::new());
    let add = move || {
        let text = draft.get_untracked();
        if !text.trim().is_empty() {
            draft.set(String::new());
            ui.send(vec![Op::AddRequirement { text, key: None }], |_| {});
        }
    };
    let list = move || {
        let selected = match ui.sel.get() {
            Some(Sel::Thing(id)) => Some(id),
            _ => None,
        };
        ui.full.with(|f| {
            let f = f.as_ref()?;
            let sel_name = selected.and_then(|id| f.thing(id)).map(|t| t.name.clone());
            Some(
                f.requirements
                    .iter()
                    .map(|r| {
                        let rid = r.id;
                        let chips = r
                            .satisfied_by
                            .iter()
                            .filter_map(|&t| f.thing(t))
                            .map(|t| {
                                let tid = t.id;
                                let opd = f.shown_in(tid).first().copied().unwrap_or(f.root());
                                view! {
                                    <span class="chip">
                                        <span class="go" on:click=move |_| ui.focus(opd, tid)>{t.name.clone()}</span>
                                        <button title="Remove this trace" on:click=move |_| ui.send(vec![Op::Trace { requirement: rid, thing: tid, satisfies: false }], |_| {})>"×"</button>
                                    </span>
                                }
                            })
                            .collect_view();
                        let trace = selected.filter(|t| !r.satisfied_by.contains(t)).zip(sel_name.clone()).map(|(tid, name)| view! {
                            <button class="chip add" title="The selected thing satisfies this requirement" on:click=move |_| ui.send(vec![Op::Trace { requirement: rid, thing: tid, satisfies: true }], |_| {})>
                                "+ " {name}
                            </button>
                        });
                        let missing = r.satisfied_by.is_empty();
                        view! {
                            <li class:missing=missing>
                                <div class="req-head">
                                    <input class="req-key" prop:value=r.key.clone() on:change=move |e| ui.send(vec![Op::EditRequirement { id: rid, key: Some(event_target_value(&e)), text: None }], |_| {}) />
                                    <input class="req-text" prop:value=r.text.clone() on:change=move |e| ui.send(vec![Op::EditRequirement { id: rid, key: None, text: Some(event_target_value(&e)) }], |_| {}) />
                                    <button class="ghost-btn icon" title="Delete requirement" on:click=move |_| ui.send(vec![Op::Remove { id: rid }], |_| {})>"×"</button>
                                </div>
                                <div class="chips">
                                    {chips}
                                    {trace}
                                    {(missing && selected.is_none()).then(|| view! { <span class="muted small">"Select the thing that satisfies it, then click + here."</span> })}
                                </div>
                            </li>
                        }
                    })
                    .collect_view(),
            )
        })
    };
    let n = move || ui.full.with(|f| f.as_ref().map_or(0, |f| f.requirements.len()));
    view! {
        <section class="panel reqs">
            <header><h3>"Requirements"</h3><span class="count">{n}</span></header>
            <ul>{list}</ul>
            <form class="req-new" on:submit=move |e| { e.prevent_default(); add() }>
                <input class="field-input" placeholder="New requirement, e.g. Serve in under 30 s (Enter)" prop:value=draft on:input=move |e| draft.set(event_target_value(&e)) />
            </form>
        </section>
    }
}

/// Play the diagram: step through its processes and watch objects come, go and change state.
#[component]
fn SimBar() -> impl IntoView {
    let ui = ui();
    move || {
        let sim = ui.sim.get()?;
        let name = |id: Id| ui.model.with_untracked(|m| m.as_ref().and_then(|m| m.thing(id)).map(|t| t.name.clone())).unwrap_or_default();
        let status = if sim.blocked {
            "Stopped".to_string()
        } else if sim.done() {
            "Finished".to_string()
        } else {
            format!("Next: {} ({}/{})", name(sim.order[sim.next]), sim.next + 1, sim.order.len())
        };
        let log = sim.log.iter().rev().take(6).rev().cloned().map(|l| view! { <li>{l}</li> }).collect_view();
        let done = sim.done();
        Some(view! {
            <div class="simbar">
                <div class="sim-head">
                    <strong>"Simulation"</strong>
                    <span class="muted small">{status}</span>
                    <span class="spacer"></span>
                    <button class="ghost-btn" title="Start over" on:click=move |_| ui.simulate()>"Reset"</button>
                    <button class="ghost-btn" disabled=done on:click=move |_| { ui.playing.set(false); ui.sim_step(); }>"Step"</button>
                    <button class="primary" disabled=done on:click=move |_| ui.sim_play()>{move || if ui.playing.get() { "Pause" } else { "Play" }}</button>
                    <button class="ghost-btn icon" title="Close" on:click=move |_| { ui.playing.set(false); ui.sim.set(None) }>"×"</button>
                </div>
                <ol class="sim-log">{log}</ol>
            </div>
        })
    }
}

#[component]
fn OplPanel() -> impl IntoView {
    let ui = ui();
    let sentences = move || {
        let sel = ui.sel.get();
        ui.model.with(|m| {
            let m = m.as_ref()?;
            let sel_id = match sel {
                Some(Sel::Thing(id) | Sel::Link(id)) => Some(id),
                Some(Sel::State(id)) => m.state(id).map(|(t, _)| t.id),
                None => None,
            };
            let list = opl::render(m)
                .into_iter()
                .map(|s| {
                    let active = sel_id.is_some_and(|id| s.refs.contains(&id));
                    let runs = name_runs(&s.text, m)
                        .into_iter()
                        .map(|(text, k)| match k {
                            Some(Kind::Object) => view! { <b class="o">{text}</b> }.into_any(),
                            Some(Kind::Process) => view! { <b class="p">{text}</b> }.into_any(),
                            None => text.into_any(),
                        })
                        .collect_view();
                    let refs = s.refs.clone();
                    let first = s.refs.first().copied();
                    let link = s.refs.get(1).copied().filter(|id| m.links.iter().any(|l| l.id == *id));
                    view! {
                        <li
                            class:active=active
                            on:mouseenter=move |_| ui.hover.set(refs.clone())
                            on:mouseleave=move |_| ui.hover.set(vec![])
                            on:click=move |_| ui.select(link.map(Sel::Link).or(first.map(Sel::Thing)))
                        >
                            {runs}
                        </li>
                    }
                })
                .collect_view();
            Some(list)
        })
    };
    let count = move || ui.model.with(|m| m.as_ref().map(|m| opl::render(m).len()).unwrap_or(0));
    let copy = move |_| {
        let text = ui.model.with_untracked(|m| {
            m.as_ref().map(|m| opl::render(m).into_iter().map(|s| s.text).collect::<Vec<_>>().join("\n")).unwrap_or_default()
        });
        let _ = window().navigator().clipboard().write_text(&text);
        ui.say("OPL of this diagram copied");
    };
    let copy_all = move |_| {
        let text = ui.full.with_untracked(|f| {
            f.as_ref()
                .map(|f| {
                    opl::render_all(f)
                        .into_iter()
                        .map(|(title, ss)| format!("{title}\n{}", ss.into_iter().map(|s| s.text).collect::<Vec<_>>().join("\n")))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .unwrap_or_default()
        });
        let _ = window().navigator().clipboard().write_text(&text);
        ui.say("OPL of every diagram copied");
    };
    view! {
        <section class="opl">
            <header>
                <h3>"OPL"</h3>
                <span class="count">{count}</span>
                <button class="ghost-btn" on:click=copy title="Copy this diagram's OPL">"Copy"</button>
                <button class="ghost-btn" on:click=copy_all title="Copy the OPL of every diagram">"All"</button>
            </header>
            <ol>{sentences}</ol>
        </section>
    }
}

#[component]
fn Toolbar() -> impl IntoView {
    let ui = ui();
    let can_state = move || {
        let sel = ui.sel.get();
        ui.model.with(|m| matches!((sel, m), (Some(Sel::Thing(id)), Some(m)) if m.thing(id).is_some_and(|t| t.kind == Kind::Object)))
    };
    let picking = RwSignal::new(false);
    let filter = RwSignal::new(String::new());
    let existing = move || {
        let here = ui.here();
        let q = filter.get().to_lowercase();
        ui.full.with(|f| {
            let f = f.as_ref()?;
            let mut v: Vec<(Id, String, Kind)> = f
                .things
                .iter()
                .filter(|t| !f.shown_in(t.id).contains(&here) && t.name.to_lowercase().contains(&q))
                .map(|t| (t.id, t.name.clone(), t.kind))
                .collect();
            v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
            Some(
                v.into_iter()
                    .map(|(id, name, kind)| {
                        view! {
                            <button on:click=move |_| { picking.set(false); ui.show_existing(id) }>
                                <span class=if kind == Kind::Object { "dot obj" } else { "dot proc" }></span>
                                {name}
                            </button>
                        }
                    })
                    .collect_view(),
            )
        })
    };
    view! {
        <nav class="toolbar">
            <button title="Object (O)" on:click=move |_| ui.add_thing(Kind::Object, None, move |id| ui.edit.set(Some(id)))>
                <svg viewBox="0 0 24 24"><rect x="3" y="6" width="18" height="12" rx="1.5" class="ic obj" /></svg>
            </button>
            <button title="Process (P)" on:click=move |_| ui.add_thing(Kind::Process, None, move |id| ui.edit.set(Some(id)))>
                <svg viewBox="0 0 24 24"><ellipse cx="12" cy="12" rx="9.5" ry="6.5" class="ic proc" /></svg>
            </button>
            <button title="State (S) — select an object first" disabled=move || !can_state() on:click=move |_| ui.add_state()>
                <svg viewBox="0 0 24 24"><rect x="4" y="8" width="16" height="9" rx="4.5" class="ic state" /></svg>
            </button>
            <button title="Show an existing thing here (same thing, another diagram)" on:click=move |_| { filter.set(String::new()); picking.update(|p| *p = !*p) }>
                <svg viewBox="0 0 24 24"><rect x="3" y="3" width="11" height="8" rx="1.5" class="ic line" /><rect x="10" y="13" width="11" height="8" rx="1.5" class="ic obj" /><path d="M8.5 11v5.5H10" class="ic line" /></svg>
            </button>
            <Show when=move || picking.get()>
                <div class="menu-catch" on:click=move |_| picking.set(false)></div>
                <div class="picker">
                    <div class="menu-title">"Show an existing thing here"</div>
                    <input class="field" placeholder="Filter…" autofocus prop:value=filter on:input=move |e| filter.set(event_target_value(&e)) />
                    <div class="picker-list">{existing}</div>
                </div>
            </Show>
            <button title="Simulate this diagram" class:on=move || ui.sim.with(|s| s.is_some()) on:click=move |_| if ui.sim.with_untracked(|s| s.is_some()) { ui.sim.set(None) } else { ui.simulate() }>
                <svg viewBox="0 0 24 24"><path d="M8 5.5v13l10.5-6.5z" class="ic line" /></svg>
            </button>
            <span class="sep"></span>
            <button title="Undo (Ctrl+Z)" on:click=move |_| ui.undo()>
                <svg viewBox="0 0 24 24"><path d="M9 14 4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3" class="ic line" /></svg>
            </button>
            <button title="Redo (Ctrl+Shift+Z)" on:click=move |_| ui.redo()>
                <svg viewBox="0 0 24 24"><path d="m15 14 5-5-5-5M20 9H10a6 6 0 0 0 0 12h3" class="ic line" /></svg>
            </button>
            <button title="Delete (Del)" disabled=move || ui.sel.get().is_none() && ui.group.get().is_empty() on:click=move |_| ui.remove_selected()>
                <svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" class="ic line" /></svg>
            </button>
            <span class="sep"></span>
            <button title="Tidy layout (T)" on:click=move |_| ui.tidy()>
                <svg viewBox="0 0 24 24"><path d="M4 5h6v4H4zM14 5h6v4h-6zM9 15h6v4H9zM7 9v3h10V9M12 12v3" class="ic line" /></svg>
            </button>
            <button title="Fit to screen (F)" on:click=move |_| ui.fit()>
                <svg viewBox="0 0 24 24"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" class="ic line" /></svg>
            </button>
        </nav>
    }
}

#[component]
fn ChatGptModal() -> impl IntoView {
    let ui = ui();
    let token = LocalResource::new(|| async { fetch_json::<String>(Request::get("/api/mcp").build()).await.unwrap_or_default() });
    let host = window().location().host().unwrap_or_default();
    move || {
        ui.chatgpt.get().then(|| {
            let host = host.clone();
            let urls = move || {
                let t = token.get().unwrap_or_default();
                (format!("http://{host}/mcp/{t}"), format!("https://<pc>.<tailnet>.ts.net/mcp/{t}"))
            };
            let urls2 = urls.clone();
            view! {
                <div class="backdrop" on:click=move |_| ui.chatgpt.set(false)>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <h2>"Draw from ChatGPT"</h2>
                        <p class="muted">"OPM Studio speaks MCP. ChatGPT needs a public HTTPS address, so expose this server with a tunnel."</p>
                        <ol class="steps">
                            <li>"Expose it (once):" <code>"tailscale funnel --bg 3000"</code></li>
                            <li>"ChatGPT → Settings → Apps & Connectors → Advanced → enable Developer mode → Create connector, no auth, URL:" <code>{move || urls().1}</code></li>
                            <li>"In a chat, enable the connector and ask: " <em>"“Model an espresso machine in OPM Studio.”"</em></li>
                        </ol>
                        <p class="muted small">"Local MCP URL (for MCP Inspector): " <code>{move || urls2().0}</code></p>
                        <p class="muted small">"The secret path is the only key — don't share it. Everything except /mcp is blocked through the tunnel."</p>
                        <button class="primary" on:click=move |_| ui.chatgpt.set(false)>"Done"</button>
                    </div>
                </div>
            }
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dialog {
    Rename,
    Versions,
    Delete,
}

#[derive(Clone, serde::Deserialize)]
struct Version {
    at: u64,
    by: String,
}

/// "Oct 30, 14:32" in the browser's locale.
fn when(at: u64) -> String {
    let d = web_sys::js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64));
    let opts = web_sys::js_sys::Object::new();
    for (k, v) in [("month", "short"), ("day", "numeric"), ("hour", "2-digit"), ("minute", "2-digit")] {
        let _ = web_sys::js_sys::Reflect::set(&opts, &k.into(), &v.into());
    }
    d.to_locale_string("default", &opts).into()
}

/// Every saved state of the model, newest first, by whoever made it; any one can be put back.
#[component]
fn Versions(dialog: RwSignal<Option<Dialog>>) -> impl IntoView {
    let ui = ui();
    let list = RwSignal::new(None::<Result<Vec<Version>, String>>);
    let mid = move || ui.mid.get_untracked().unwrap_or_default();
    spawn_local(async move { list.set(Some(fetch_json(Request::get(&format!("/api/models/{}/versions", mid())).build()).await)) });
    let restore = move |at: u64| {
        let mid = mid();
        spawn_local(async move {
            match Request::post(&format!("/api/models/{mid}/versions/{at}/restore")).send().await {
                Ok(r) if r.ok() => {
                    ui.say(format!("Restored the version from {}", when(at)));
                    dialog.set(None);
                }
                Ok(r) => ui.say(r.text().await.unwrap_or_default()),
                Err(e) => ui.say(e2s(e)),
            }
        });
    };
    view! {
        <h2>"Versions"</h2>
        <p class="muted">"Saved on every change, yours or the AI's. Restoring adds a new version, so it can be undone here too."</p>
        <div class="versions">
            {move || match list.get() {
                None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! { <p class="muted">{e}</p> }.into_any(),
                Some(Ok(v)) if v.is_empty() => view! { <p class="muted">"No versions yet: they start with the next change."</p> }.into_any(),
                Some(Ok(v)) => v.into_iter().enumerate().map(|(i, ver)| {
                    let (at, cls) = (ver.at, format!("by by-{}", ver.by.to_lowercase()));
                    view! {
                        <div class="version">
                            <span class="when">{when(at)}</span>
                            <span class=cls>{ver.by}</span>
                            <span class="spacer"></span>
                            {if i == 0 {
                                view! { <span class="muted small">"current"</span> }.into_any()
                            } else {
                                view! { <button class="ghost-btn" on:click=move |_| restore(at)>"Restore"</button> }.into_any()
                            }}
                        </div>
                    }
                }).collect_view().into_any(),
            }}
        </div>
        <div class="actions">
            <button class="ghost-btn" on:click=move |_| dialog.set(None)>"Close"</button>
        </div>
    }
}

/// The "⋯" menu next to the model name: rename, export, import, delete.
#[component]
fn ModelMenu() -> impl IntoView {
    use files::Format;
    let ui = ui();
    let open = RwSignal::new(false);
    let dialog = RwSignal::new(None::<Dialog>);
    let name = RwSignal::new(String::new());
    let item = move |label: &'static str, act: Box<dyn Fn()>| {
        view! { <button on:click=move |_| { open.set(false); act() }>{label}</button> }
    };
    let title = move || ui.model.with(|m| m.as_ref().map(|m| m.name.clone()).unwrap_or_default());
    let rename = move || {
        let n = name.get_untracked();
        if !n.trim().is_empty() {
            ui.rename_model(n);
        }
        dialog.set(None);
    };
    view! {
        <div class="model-menu">
            <button class="ghost-btn icon" title="Model: rename, versions, export, import, delete" on:click=move |_| open.update(|o| *o = !*o)>"⋯"</button>
            <Show when=move || open.get()>
                <div class="menu-catch" on:click=move |_| open.set(false)></div>
                <div class="dropdown">
                    {item("Rename…", Box::new(move || { name.set(title()); dialog.set(Some(Dialog::Rename)) }))}
                    {item("Versions…", Box::new(move || dialog.set(Some(Dialog::Versions))))}
                    <hr />
                    {item("Export JSON (to reuse)", Box::new(move || ui.export(Format::Json)))}
                    {item("Export SVG (image)", Box::new(move || ui.export(Format::Svg)))}
                    {item("Export PNG (image)", Box::new(move || ui.export(Format::Png)))}
                    {item("Export SysML v2", Box::new(move || ui.export(Format::SysMl)))}
                    {item("Report (HTML, printable)", Box::new(move || ui.report()))}
                    {item("Import JSON…", Box::new(move || ui.import_pick()))}
                    <hr />
                    <button class="danger-item" on:click=move |_| { open.set(false); dialog.set(Some(Dialog::Delete)) }>"Delete model…"</button>
                </div>
            </Show>
            {move || dialog.get().map(|d| view! {
                <div class="backdrop" on:click=move |_| dialog.set(None)>
                    <div class="modal small" on:click=|e| e.stop_propagation()>
                        {match d {
                            Dialog::Rename => view! {
                                <h2>"Rename model"</h2>
                                <form on:submit=move |e| { e.prevent_default(); rename() }>
                                    <input class="field" autofocus prop:value=name on:input=move |e| name.set(event_target_value(&e)) />
                                    <div class="actions">
                                        <button type="button" class="ghost-btn" on:click=move |_| dialog.set(None)>"Cancel"</button>
                                        <button type="submit" class="primary">"Rename"</button>
                                    </div>
                                </form>
                            }.into_any(),
                            Dialog::Versions => view! { <Versions dialog /> }.into_any(),
                            Dialog::Delete => view! {
                                <h2>"Delete “" {title} "”?"</h2>
                                <p class="muted">"It disappears from OPM Studio. The file is kept in data/.trash, so it can be recovered."</p>
                                <div class="actions">
                                    <button class="ghost-btn" on:click=move |_| dialog.set(None)>"Cancel"</button>
                                    <button class="primary danger-btn" on:click=move |_| { dialog.set(None); ui.delete_model() }>"Delete"</button>
                                </div>
                            }.into_any(),
                        }}
                    </div>
                </div>
            })}
        </div>
    }
}

#[component]
fn Home() -> impl IntoView {
    let ui = ui();
    let name = RwSignal::new(String::new());
    let create = move || {
        let n = name.get_untracked();
        if n.trim().is_empty() {
            return;
        }
        spawn_local(async move {
            let req = Request::post("/api/models").json(&serde_json::json!({ "name": n }));
            match fetch_json::<String>(req).await {
                Ok(id) => {
                    let _ = window().location().set_hash(&id);
                }
                Err(e) => ui.say(e),
            }
        });
    };
    view! {
        <main class="home">
            <div class="hero">
                <h1>"Model systems as objects and processes."</h1>
                <p class="muted">"Object-Process Methodology (ISO 19450): one diagram, one language. Draw it, read it, or let ChatGPT draft it."</p>
                <form class="new" on:submit=move |e| { e.prevent_default(); create(); }>
                    <input placeholder="New system, e.g. Coffee Making" prop:value=name on:input=move |e| name.set(event_target_value(&e)) />
                    <button class="primary" type="submit">"Create"</button>
                    <button class="ghost-btn" type="button" on:click=move |_| ui.import_pick()>"Import…"</button>
                </form>
                <p class="muted small">"You can also drop an exported .json file anywhere on this page."</p>
                <div class="cards">
                    {move || ui.models.get().into_iter().map(|(id, n)| view! {
                        <a class="card" href=format!("#{id}")>
                            <svg viewBox="0 0 40 24" width="40" height="24">
                                <rect x="1" y="3" width="16" height="12" rx="1.5" class="ic obj" />
                                <ellipse cx="29" cy="15" rx="10" ry="7" class="ic proc" />
                            </svg>
                            <span>{n}</span>
                        </a>
                    }).collect_view()}
                </div>
            </div>
        </main>
    }
}

#[component]
fn App() -> impl IntoView {
    let model = RwSignal::new(None);
    let drag: RwSignal<Option<Drag>> = RwSignal::new(None);
    let style = RwSignal::new(routes::LinkStyle::from_key(
        &storage().and_then(|s| s.get_item("opm-link-style").ok().flatten()).unwrap_or_default(),
    ));
    // Only flips when a move-drag starts or ends, so routes don't recompute on every drag event.
    let moving = Memo::new(move |_| drag.with(|d| matches!(d, Some(Drag::Move { .. }))));
    let ui = Ui {
        mid: RwSignal::new(hash_parts().0),
        full: RwSignal::new(None),
        opd: RwSignal::new(hash_parts().1),
        showing: StoredValue::new(0),
        model,
        models: RwSignal::new(vec![]),
        sel: RwSignal::new(None),
        drag,
        view: RwSignal::new((0.0, 0.0, 1.0)),
        menu: RwSignal::new(None),
        edit: RwSignal::new(None),
        hover: RwSignal::new(vec![]),
        group: RwSignal::new(vec![]),
        hot: RwSignal::new(None),
        space: RwSignal::new(false),
        style,
        routes: Memo::new(move |prev: Option<&std::sync::Arc<routes::Routes>>| {
            let dragging = moving.get();
            let style = style.get();
            model.with(|m| match m {
                Some(m) => std::sync::Arc::new(routes::Routes::compute(m, style, prev.map(|p| &**p), dragging)),
                None => std::sync::Arc::new(routes::Routes::compute(&Model::default(), style, None, false)),
            })
        }),
        pointer: StoredValue::new(None),
        toast: RwSignal::new(None),
        live: RwSignal::new(false),
        chatgpt: RwSignal::new(false),
        search: RwSignal::new(false),
        sim: RwSignal::new(None),
        playing: RwSignal::new(false),
        past: StoredValue::new(vec![]),
        future: StoredValue::new(vec![]),
        fitted: StoredValue::new(false),
        anim: StoredValue::new(0),
        fresh: RwSignal::new(vec![]),
        svg: NodeRef::new(),
    };
    provide_context(ui);

    if let Some(t) = storage().and_then(|s| s.get_item("opm-theme").ok().flatten()) {
        set_theme(t == "dark");
    }
    let dark = RwSignal::new(is_dark());

    let _ = window_event_listener(ev::hashchange, move |_| {
        let (mid, opd) = hash_parts();
        ui.opd.set(opd);
        if mid != ui.mid.get_untracked() {
            ui.mid.set(mid);
        }
    });
    // Another diagram of the same model: just show it.
    Effect::new(move |prev: Option<Id>| {
        let opd = ui.opd.get();
        if prev.is_some_and(|p| p != opd) {
            ui.show_here();
        }
        opd
    });
    ui.import_on_drop();
    let _ = window_event_listener(ev::keydown, move |e| {
        if (e.ctrl_key() || e.meta_key()) && e.key().eq_ignore_ascii_case("k") && ui.mid.get_untracked().is_some() {
            e.prevent_default();
            return ui.search.set(true);
        }
        ui.key(e)
    });
    // Clipboard: copy/cut/paste things (also across models and tabs), unless a text field has focus.
    let typing = || {
        document()
            .active_element()
            .is_some_and(|el| matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT"))
    };
    let _ = window_event_listener(ev::copy, move |e| {
        if let (false, Some(json)) = (typing(), ui.copy_selection()) {
            if let Some(dt) = e.clipboard_data() {
                let _ = dt.set_data("text/plain", &json);
                e.prevent_default();
            }
        }
    });
    let _ = window_event_listener(ev::cut, move |e| {
        if let (false, Some(json)) = (typing(), ui.copy_selection()) {
            if let Some(dt) = e.clipboard_data() {
                let _ = dt.set_data("text/plain", &json);
                e.prevent_default();
                ui.remove_selected();
            }
        }
    });
    let _ = window_event_listener(ev::paste, move |e| {
        let text = e.clipboard_data().and_then(|dt| dt.get_data("text/plain").ok()).unwrap_or_default();
        if !typing() && ui.paste(&text) {
            e.prevent_default();
        }
    });
    let _ = window_event_listener(ev::keyup, move |e| {
        if e.key() == " " {
            ui.space.set(false);
        }
    });

    Effect::new(move |_| {
        let mid = ui.mid.get();
        ui.model.set(None);
        ui.full.set(None);
        ui.select(None);
        ui.live.set(false);
        ui.past.set_value(vec![]);
        ui.future.set_value(vec![]);
        ui.fitted.set_value(false);
        spawn_local(async move { ui.models.set(list_models().await) });
        if let Some(mid) = mid {
            connect(ui, mid);
        }
    });

    let title = move || ui.model.with(|m| m.as_ref().map(|m| m.name.clone()).unwrap_or_default());
    view! {
        <div class="app">
            <header class="top">
                <a class="brand" href="#">
                    <svg viewBox="0 0 32 32" width="22" height="22">
                        <rect x="2" y="4" width="14" height="10" rx="1.5" class="ic obj" />
                        <ellipse cx="21" cy="22" rx="9" ry="6" class="ic proc" />
                    </svg>
                    "OPM Studio"
                </a>
                <Show when=move || ui.mid.get().is_some()>
                    <span class="crumb">"/"</span>
                    <select class="model-pick" on:change=move |e| { let _ = window().location().set_hash(&event_target_value(&e)); }>
                        {move || {
                            let cur = ui.mid.get();
                            ui.models.get().into_iter().map(|(id, n)| {
                                let selected = cur.as_deref() == Some(id.as_str());
                                view! { <option value=id selected=selected>{n}</option> }
                            }).collect_view()
                        }}
                        <option value="">"+ New model…"</option>
                    </select>
                    <ModelMenu />
                    <button class="ghost-btn" title="Find anything in the model (Ctrl+K)" on:click=move |_| ui.search.set(true)>"Find  " <kbd>"Ctrl K"</kbd></button>
                    <span class=move || if ui.live.get() { "live on" } else { "live" } title="Live sync with server">
                        {move || if ui.live.get() { "Live" } else { "Offline" }}
                    </span>
                </Show>
                <span class="spacer"></span>
                <button class="ghost-btn" on:click=move |_| ui.chatgpt.set(true)>"Connect ChatGPT"</button>
                <button
                    class="ghost-btn icon"
                    title="Toggle theme"
                    on:click=move |_| { let d = !dark.get_untracked(); set_theme(d); dark.set(d); }
                >
                    {move || if dark.get() { "☀" } else { "☾" }}
                </button>
            </header>
            {move || match ui.mid.get() {
                None => view! { <Home /> }.into_any(),
                Some(_) => view! {
                    <div class="workspace">
                        <Toolbar />
                        <Canvas />
                        <aside class="side">
                            <div class="side-title">{title}</div>
                            <Diagrams />
                            <Inspector />
                            <Checks />
                            <Requirements />
                            <OplPanel />
                        </aside>
                    </div>
                }
                .into_any(),
            }}
            <ChatGptModal />
            <Search />
            {move || ui.toast.get().map(|t| view! { <div class="toast">{t}</div> })}
        </div>
    }
}

fn main() {
    console_error_panic_hook_lite();
    leptos::mount::mount_to_body(App);
}

/// Panics show up in the browser console instead of a silent "unreachable".
fn console_error_panic_hook_lite() {
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&info.to_string().into())));
}
