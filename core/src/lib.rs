//! OPM (ISO 19450) model: things, states, links, the tree of diagrams (OPDs) that show them,
//! and the single write path `Model::apply`.
//!
//! Every thing and link exists once in the model; diagrams are views. A diagram lists which things it
//! shows and where; a link shows wherever both its ends do. Detail diagrams refine a thing:
//! in-zooming a process (its subprocesses drawn inside it, in time order top to bottom) or
//! unfolding a thing (its parts, attributes, specializations).
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub mod check;
pub mod layout;
pub mod opl;
pub mod sim;
pub mod sysml;

pub type Id = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Object,
    Process,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Essence {
    Physical,
    #[default]
    Informatical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Affiliation {
    #[default]
    Systemic,
    Environmental,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: Id,
    pub name: String,
    /// The state the object starts in (drawn with a thick border).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub initial: bool,
    /// A state the object ends in (drawn with a double border).
    #[serde(default, rename = "final", skip_serializing_if = "std::ops::Not::not")]
    pub final_: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thing {
    pub id: Id,
    pub name: String,
    pub kind: Kind,
    #[serde(default)]
    pub essence: Essence,
    #[serde(default)]
    pub affiliation: Affiliation,
    #[serde(default)]
    pub states: Vec<State>,
    // The fields below describe the thing in one diagram. `Model::view` fills them in for drawing;
    // they are never stored (positions live in `Opd::shows`). Older files kept x/y here: read once to migrate.
    #[serde(default, skip_serializing)]
    pub x: f64,
    #[serde(default, skip_serializing)]
    pub y: f64,
    /// Position chosen by the layout, not the user: automatic arranging may move it.
    #[serde(default, skip_serializing)]
    pub auto: bool,
    /// In-zoomed in this diagram: drawn as a frame of this size around its subprocesses.
    #[serde(default, skip_serializing)]
    pub frame: Option<(f64, f64)>,
    /// Inside the in-zoomed frame: a subprocess or internal object of it.
    #[serde(default, skip_serializing)]
    pub inside: bool,
    /// Another diagram refines this thing (open it with a double-click).
    #[serde(default, skip_serializing)]
    pub detail: Option<Id>,
    /// Order to draw the state pills in this diagram: states linked to things on the left come first,
    /// so their links don't cross. Empty = as listed. See `Model::order_states`.
    #[serde(default, skip_serializing)]
    pub state_order: Vec<Id>,
}

/// A thing shown in a diagram.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    pub thing: Id,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub auto: bool,
    /// Inside the in-zoomed process of this diagram.
    #[serde(default)]
    pub inside: bool,
}

/// An object-process diagram. The root (SD) has no parent and refines nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opd {
    pub id: Id,
    #[serde(default)]
    pub parent: Option<Id>,
    #[serde(default)]
    pub refines: Option<Id>,
    /// Unfolding (parts/attributes) rather than in-zooming (subprocesses).
    #[serde(default)]
    pub unfold: bool,
    #[serde(default)]
    pub shows: Vec<Shown>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LinkKind {
    // structural
    Aggregation,
    Exhibition,
    Generalization,
    Classification,
    Tagged(String),
    // procedural
    Agent,
    Instrument,
    Consumption,
    Result,
    Effect,
    Invocation,
}

impl LinkKind {
    pub fn is_procedural(&self) -> bool {
        use LinkKind::*;
        matches!(self, Agent | Instrument | Consumption | Result | Effect | Invocation)
    }
}

/// Control modifier of an enabling or transforming link (ISO 19450 "c" and "e" links).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Modifier {
    #[default]
    None,
    /// The process happens only if the object exists (in that state); otherwise it is skipped.
    Condition,
    /// The object appearing (in that state) starts the process.
    Event,
}

impl Modifier {
    pub fn is_none(&self) -> bool {
        *self == Modifier::None
    }
}

/// `from_state` / `to_state` always refer to states of the object end of the link:
/// input state and output state (Effect uses both: "changes X from a to b").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub id: Id,
    pub from: Id,
    pub to: Id,
    pub kind: LinkKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_state: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_state: Option<Id>,
    #[serde(default, skip_serializing_if = "Modifier::is_none")]
    pub modifier: Modifier,
    /// How many of the object/part end: "4", "0..1", "1..*", "2..5", "*". Empty = one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub multiplicity: String,
}

impl Link {
    /// Can carry a condition or event: agent, instrument, consumption or effect between an object and a process.
    pub fn modifiable(&self) -> bool {
        use LinkKind::*;
        matches!(self.kind, Agent | Instrument | Consumption | Effect)
    }

    /// Can carry a multiplicity: all but generalization, classification and invocation.
    pub fn countable(&self) -> bool {
        use LinkKind::*;
        !matches!(self.kind, Generalization | Classification | Invocation)
    }
}

/// "4", "0..1", "1..*", "2..5", "*".
pub fn valid_multiplicity(s: &str) -> bool {
    let num = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit());
    match s.split_once("..") {
        None => s == "*" || num(s),
        Some((a, "*")) => num(a),
        Some((a, b)) => num(a) && num(b) && a.parse::<u64>().ok() <= b.parse::<u64>().ok(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FanKind {
    /// Exactly one of the links happens.
    Xor,
    /// At least one of them happens.
    Or,
}

/// Links of the same kind sharing one end, grouped as alternatives ("yields either A or B").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fan {
    pub id: Id,
    pub kind: FanKind,
    pub links: Vec<Id>,
}

/// A requirement and the things that satisfy it (traceability).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    pub id: Id,
    /// Short reference, unique in the model: "REQ-1".
    pub key: String,
    pub text: String,
    #[serde(default)]
    pub satisfied_by: Vec<Id>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub things: Vec<Thing>,
    pub links: Vec<Link>,
    /// The diagram tree; the first one is the root (SD).
    #[serde(default)]
    pub opds: Vec<Opd>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fans: Vec<Fan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<Requirement>,
    pub next_id: Id,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Creates a thing shown in `opd` (default: the root).
    AddThing {
        name: String,
        kind: Kind,
        #[serde(default)]
        essence: Essence,
        #[serde(default)]
        affiliation: Affiliation,
        #[serde(default)]
        opd: Option<Id>,
        /// None = auto-place.
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        /// In an in-zoom diagram: inside the zoomed process. Default: processes inside, objects outside.
        #[serde(default)]
        inside: Option<bool>,
    },
    /// Shows an existing thing in another diagram too (or updates `inside` if already shown).
    Show {
        opd: Id,
        thing: Id,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
        #[serde(default)]
        inside: Option<bool>,
    },
    /// Takes a thing out of one diagram; it stays in the model while some diagram still shows it.
    Hide { opd: Id, thing: Id },
    /// Adds the diagram refining `thing` (in-zoom for processes, or unfold), under `from`. Returns the
    /// existing one if there is one.
    Refine {
        thing: Id,
        #[serde(default)]
        unfold: bool,
        #[serde(default)]
        from: Option<Id>,
    },
    /// Deletes a diagram and its sub-diagrams; things no diagram shows anymore go too.
    RemoveOpd { opd: Id },
    AddState { thing: Id, name: String },
    AddLink {
        from: Id,
        to: Id,
        kind: LinkKind,
        #[serde(default)]
        from_state: Option<Id>,
        #[serde(default)]
        to_state: Option<Id>,
    },
    /// Thing or state.
    Rename { id: Id, name: String },
    /// `auto`: the layout placed it (see `Thing::auto`); a hand move leaves it false.
    Move {
        #[serde(default)]
        opd: Option<Id>,
        id: Id,
        x: f64,
        y: f64,
        #[serde(default)]
        auto: bool,
    },
    Update {
        id: Id,
        #[serde(default)]
        essence: Option<Essence>,
        #[serde(default)]
        affiliation: Option<Affiliation>,
    },
    /// Link details: the object's input/output states, condition/event, multiplicity ("" clears it).
    SetLink {
        id: Id,
        #[serde(default)]
        states: Option<(Option<Id>, Option<Id>)>,
        #[serde(default)]
        modifier: Option<Modifier>,
        #[serde(default)]
        multiplicity: Option<String>,
    },
    SetState {
        id: Id,
        #[serde(default)]
        initial: Option<bool>,
        #[serde(default, rename = "final")]
        final_: Option<bool>,
    },
    /// Groups links (same kind, one shared end) as XOR/OR alternatives; `kind: None` ungroups them.
    /// Returns the fan id (or the first link's id when ungrouping).
    Fan {
        links: Vec<Id>,
        #[serde(default)]
        kind: Option<FanKind>,
    },
    /// Adds a requirement; `key` defaults to the next "REQ-n".
    AddRequirement {
        text: String,
        #[serde(default)]
        key: Option<String>,
    },
    EditRequirement {
        id: Id,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        text: Option<String>,
    },
    /// Marks `thing` as satisfying `requirement` (or not).
    Trace { requirement: Id, thing: Id, satisfies: bool },
    /// Thing (from the whole model, with its links and detail diagrams), state, link, or requirement.
    Remove { id: Id },
    /// Paste into `opd`: things (with states) and the links among them, carrying any ids, placed at
    /// `at` (by original id) shifted by (dx, dy). They get fresh ids and unique names ("X copy").
    /// Returns the first new thing's id.
    Insert {
        things: Vec<Thing>,
        #[serde(default)]
        links: Vec<Link>,
        #[serde(default)]
        at: Vec<(Id, f64, f64)>,
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
        #[serde(default)]
        opd: Option<Id>,
    },
}

/// Link kinds ISO 19450 allows from `from` to `to`.
pub fn valid_link_kinds(from: Kind, to: Kind) -> Vec<LinkKind> {
    use Kind::*;
    use LinkKind::*;
    let mut v = match (from, to) {
        (Object, Process) => vec![Agent, Instrument, Consumption, Effect, Exhibition],
        (Process, Object) => vec![Result, Effect, Exhibition],
        (Process, Process) => vec![Invocation, Aggregation, Generalization, Classification, Exhibition],
        (Object, Object) => vec![Aggregation, Exhibition, Generalization, Classification],
    };
    v.push(Tagged(String::new()));
    v
}

fn same_variant(a: &LinkKind, b: &LinkKind) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

impl Model {
    pub fn new(name: impl Into<String>) -> Self {
        let mut m = Model { name: name.into(), next_id: 1, ..Default::default() };
        m.normalize();
        m
    }

    /// Brings any stored model up to date: older files (no diagrams, positions on things) get a root
    /// diagram showing everything where it was.
    pub fn normalize(&mut self) {
        let max = self.things.iter().flat_map(|t| std::iter::once(t.id).chain(t.states.iter().map(|s| s.id)));
        let max = max.chain(self.links.iter().map(|l| l.id)).chain(self.opds.iter().map(|o| o.id)).chain(self.fans.iter().map(|f| f.id)).chain(self.requirements.iter().map(|r| r.id)).max().unwrap_or(0);
        self.next_id = self.next_id.max(max + 1);
        if self.opds.is_empty() {
            let shows = self.things.iter().map(|t| Shown { thing: t.id, x: t.x, y: t.y, auto: t.auto, inside: false }).collect();
            let id = self.fresh();
            self.opds.push(Opd { id, parent: None, refines: None, unfold: false, shows });
        }
    }

    /// Ids unique and every reference resolving: what a model from outside (import, undo) must satisfy.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = HashSet::new();
        let ids = self.things.iter().flat_map(|t| std::iter::once(t.id).chain(t.states.iter().map(|s| s.id)));
        for id in ids.chain(self.links.iter().map(|l| l.id)).chain(self.opds.iter().map(|o| o.id)).chain(self.fans.iter().map(|f| f.id)).chain(self.requirements.iter().map(|r| r.id)) {
            if id == 0 || id >= self.next_id || !seen.insert(id) {
                return Err(format!("bad or duplicate id {id}"));
            }
        }
        for l in &self.links {
            if self.thing(l.from).is_none() || self.thing(l.to).is_none() {
                return Err(format!("link {} points to a missing thing", l.id));
            }
            if [l.from_state, l.to_state].into_iter().flatten().any(|s| self.state(s).is_none()) {
                return Err(format!("link {} points to a missing state", l.id));
            }
        }
        if self.requirements.iter().any(|r| r.satisfied_by.iter().any(|t| self.thing(*t).is_none())) {
            return Err("a requirement points to a missing thing".into());
        }
        if self.fans.iter().any(|f| f.links.iter().any(|l| !self.links.iter().any(|x| x.id == *l))) {
            return Err("a fan points to a missing link".into());
        }
        if self.opds.iter().filter(|o| o.parent.is_none()).count() != 1 || self.opds.first().is_some_and(|o| o.parent.is_some()) {
            return Err("there must be exactly one root diagram, listed first".into());
        }
        for o in &self.opds {
            if o.parent.is_some_and(|p| self.opd(p).is_none()) || o.refines.is_some_and(|t| self.thing(t).is_none()) {
                return Err(format!("diagram {} points to something missing", o.id));
            }
            if o.shows.iter().any(|s| self.thing(s.thing).is_none()) {
                return Err(format!("diagram {} shows a missing thing", o.id));
            }
        }
        Ok(())
    }

    pub fn root(&self) -> Id {
        self.opds.first().map_or(0, |o| o.id)
    }

    pub fn opd(&self, id: Id) -> Option<&Opd> {
        self.opds.iter().find(|o| o.id == id)
    }

    fn opd_mut(&mut self, id: Id) -> Result<&mut Opd, String> {
        self.opds.iter_mut().find(|o| o.id == id).ok_or_else(|| "no such diagram".to_string())
    }

    /// The diagram refining `thing`, if any.
    pub fn detail_of(&self, thing: Id) -> Option<Id> {
        self.opds.iter().find(|o| o.refines == Some(thing)).map(|o| o.id)
    }

    /// "SD", "SD1", "SD1.2": position in the diagram tree.
    pub fn label(&self, opd: Id) -> String {
        let Some(o) = self.opd(opd) else { return "?".into() };
        let Some(parent) = o.parent else { return "SD".into() };
        let n = self.opds.iter().filter(|s| s.parent == Some(parent)).position(|s| s.id == opd).unwrap_or(0) + 1;
        match self.label(parent).as_str() {
            "SD" => format!("SD{n}"),
            p => format!("{p}.{n}"),
        }
    }

    /// "SD1: Producing in-zoomed".
    pub fn title(&self, opd: Id) -> String {
        let label = self.label(opd);
        match self.opd(opd).and_then(|o| Some((self.thing(o.refines?)?, o.unfold))) {
            Some((t, unfold)) => format!("{label}: {} {}", t.name, if unfold { "unfolded" } else { "in-zoomed" }),
            None => format!("{label}: System Diagram"),
        }
    }

    /// Diagrams in tree order (parents before children), with their depth.
    pub fn tree(&self) -> Vec<(Id, usize)> {
        fn walk(m: &Model, id: Id, depth: usize, out: &mut Vec<(Id, usize)>) {
            out.push((id, depth));
            for c in m.opds.iter().filter(|o| o.parent == Some(id)) {
                walk(m, c.id, depth + 1, out);
            }
        }
        let mut out = vec![];
        if !self.opds.is_empty() {
            walk(self, self.root(), 0, &mut out);
        }
        out
    }

    /// Diagrams showing `thing`.
    pub fn shown_in(&self, thing: Id) -> Vec<Id> {
        self.opds.iter().filter(|o| o.shows.iter().any(|s| s.thing == thing)).map(|o| o.id).collect()
    }

    /// Things inside the in-zoomed process of `opd`.
    pub fn inner(&self, opd: Id) -> Vec<Id> {
        self.opd(opd).map(|o| o.shows.iter().filter(|s| s.inside).map(|s| s.thing).collect()).unwrap_or_default()
    }

    /// Diagram `opd` as a drawable model: only what it shows, with positions filled in, the in-zoomed
    /// process sized as a frame around its inside, and parent-to-own-subprocess links left out.
    /// Diagrams and ids stay, so ops can be tried on the result.
    pub fn view(&self, opd: Id) -> Model {
        let opd = if self.opd(opd).is_some() { opd } else { self.root() };
        let mut v = Model { name: self.name.clone(), opds: self.opds.clone(), next_id: self.next_id, ..Default::default() };
        let Some(o) = self.opd(opd) else { return v };
        let zoomed = o.refines.filter(|_| !o.unfold);
        for s in &o.shows {
            let Some(t) = self.thing(s.thing) else { continue };
            let mut t = t.clone();
            (t.x, t.y, t.auto, t.inside) = (s.x, s.y, s.auto, s.inside && Some(s.thing) != zoomed);
            t.detail = self.detail_of(t.id).filter(|&d| d != opd);
            v.things.push(t);
        }
        if let Some(i) = zoomed.and_then(|p| v.things.iter().position(|t| t.id == p)) {
            let mut t = v.things.remove(i);
            t.frame = Some((0.0, 0.0));
            v.things.insert(0, t); // drawn behind its inside
            v.refit_frame();
        }
        let shown: HashSet<Id> = v.things.iter().map(|t| t.id).collect();
        let inner: HashSet<Id> = v.things.iter().filter(|t| t.inside).map(|t| t.id).collect();
        // External links to the zoomed process show on its frame only until a subprocess takes them over.
        let distributed = |x: Id| self.links.iter().any(|l| (l.from == x && inner.contains(&l.to)) || (l.to == x && inner.contains(&l.from)));
        v.links = self
            .links
            .iter()
            .filter(|l| shown.contains(&l.from) && shown.contains(&l.to))
            .filter(|l| match zoomed {
                Some(p) if l.from == p || l.to == p => {
                    let other = if l.from == p { l.to } else { l.from };
                    !inner.contains(&other) && !distributed(other)
                }
                _ => true,
            })
            .cloned()
            .collect();
        v.fans = self.fans.clone();
        v.prune_fans();
        v.requirements = self.requirements.clone();
        v.order_states();
        v
    }

    /// In a diagram view: orders each object's state pills by where the things linked to them are,
    /// left to right. States without links here keep their places.
    pub fn order_states(&mut self) {
        let centers: HashMap<Id, f64> = self.things.iter().map(|t| (t.id, t.x + layout::size(t).0 / 2.0)).collect();
        let mut orders = vec![];
        for t in self.things.iter().filter(|t| t.states.len() > 1) {
            let pull = |s: Id| {
                let xs: Vec<f64> = self
                    .links
                    .iter()
                    .filter(|l| l.from_state == Some(s) || l.to_state == Some(s))
                    .filter_map(|l| centers.get(if l.from == t.id { &l.to } else { &l.from }).copied())
                    .collect();
                (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
            };
            let mut order: Vec<Id> = t.states.iter().map(|s| s.id).collect();
            // The linked states trade places among the slots they hold, sorted by their pull.
            let slots: Vec<usize> = (0..order.len()).filter(|&i| pull(order[i]).is_some()).collect();
            let mut linked: Vec<(f64, Id)> = slots.iter().map(|&i| (pull(order[i]).unwrap(), order[i])).collect();
            linked.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (slot, (_, id)) in slots.iter().zip(linked) {
                order[*slot] = id;
            }
            orders.push((t.id, order));
        }
        for (id, order) in orders {
            if let Some(t) = self.things.iter_mut().find(|t| t.id == id) {
                t.state_order = order;
            }
        }
    }

    /// A thing's states in drawing order (see `Thing::state_order`).
    pub fn states_in_order(t: &Thing) -> Vec<&State> {
        let ordered: Vec<&State> = t.state_order.iter().filter_map(|id| t.states.iter().find(|s| s.id == *id)).collect();
        if ordered.len() == t.states.len() { ordered } else { t.states.iter().collect() }
    }

    /// Requirements `thing` satisfies.
    pub fn satisfies(&self, thing: Id) -> Vec<&Requirement> {
        self.requirements.iter().filter(|r| r.satisfied_by.contains(&thing)).collect()
    }

    fn check_key(&self, key: &str, except: Option<Id>) -> Result<String, String> {
        let key = key.trim();
        if key.is_empty() {
            return Err("requirement key is empty".into());
        }
        if self.requirements.iter().any(|r| r.key.eq_ignore_ascii_case(key) && Some(r.id) != except) {
            return Err(format!("requirement '{key}' already exists"));
        }
        Ok(key.to_string())
    }

    /// Drops links that are gone from fans, and fans left with fewer than two links.
    fn prune_fans(&mut self) {
        let links: HashSet<Id> = self.links.iter().map(|l| l.id).collect();
        for f in &mut self.fans {
            f.links.retain(|l| links.contains(l));
        }
        self.fans.retain(|f| f.links.len() >= 2);
    }

    /// The fan grouping link `id`, if any.
    pub fn fan_of(&self, id: Id) -> Option<&Fan> {
        self.fans.iter().find(|f| f.links.contains(&id))
    }

    /// The end every link of a fan shares.
    pub fn fan_end(&self, f: &Fan) -> Option<Id> {
        let ls: Vec<&Link> = f.links.iter().filter_map(|id| self.links.iter().find(|l| l.id == *id)).collect();
        let first = ls.first()?;
        [first.from, first.to].into_iter().find(|&e| ls.iter().all(|l| l.from == e || l.to == e))
    }

    /// In a diagram view: sizes the in-zoomed frame around what is inside it (after things moved).
    pub fn refit_frame(&mut self) {
        let inner: Vec<_> = self.things.iter().filter(|t| t.inside).map(|t| (t.x, t.y, layout::size(t))).collect();
        if let Some(f) = self.things.iter_mut().find(|t| t.frame.is_some()) {
            let (x, y, w, h) = layout::frame_box(&inner, (f.x, f.y));
            (f.x, f.y, f.frame) = (x, y, Some((w, h)));
        }
    }

    /// Removes what no longer hangs together: diagrams whose refined thing or parent is gone,
    /// appearances of deleted things, and things no diagram shows (with their links).
    fn cleanup(&mut self) {
        loop {
            let things: HashSet<Id> = self.things.iter().map(|t| t.id).collect();
            let opds: HashSet<Id> = self.opds.iter().map(|o| o.id).collect();
            let before = (self.things.len(), self.opds.len());
            let root = self.root();
            self.opds.retain(|o| o.id == root || (o.parent.is_some_and(|p| opds.contains(&p)) && o.refines.is_none_or(|t| things.contains(&t))));
            for o in &mut self.opds {
                o.shows.retain(|s| things.contains(&s.thing));
            }
            let shown: HashSet<Id> = self.opds.iter().flat_map(|o| o.shows.iter().map(|s| s.thing)).collect();
            self.things.retain(|t| shown.contains(&t.id));
            let things: HashSet<Id> = self.things.iter().map(|t| t.id).collect();
            self.links.retain(|l| things.contains(&l.from) && things.contains(&l.to));
            if (self.things.len(), self.opds.len()) == before {
                break;
            }
        }
        self.prune_fans();
        let things: HashSet<Id> = self.things.iter().map(|t| t.id).collect();
        for r in &mut self.requirements {
            r.satisfied_by.retain(|t| things.contains(t));
        }
    }

    /// A free-ish spot for a thing added without a position (automatic arranging tidies it afterwards).
    fn spot(&self, opd: Id) -> (f64, f64) {
        let n = self.opd(opd).map_or(0, |o| o.shows.len());
        (80.0 + (n % 5) as f64 * 220.0, 80.0 + (n / 5) as f64 * 130.0)
    }

    /// Default for `inside`: in an in-zoom diagram, new processes are subprocesses.
    fn default_inside(&self, opd: Id, kind: Kind) -> bool {
        self.opd(opd).is_some_and(|o| o.refines.is_some() && !o.unfold) && kind == Kind::Process
    }

    pub fn thing(&self, id: Id) -> Option<&Thing> {
        self.things.iter().find(|t| t.id == id)
    }

    /// Case-insensitive lookup by name.
    pub fn find(&self, name: &str) -> Option<&Thing> {
        self.things.iter().find(|t| t.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Thing owning state `id`, plus the state.
    pub fn state(&self, id: Id) -> Option<(&Thing, &State)> {
        self.things.iter().find_map(|t| t.states.iter().find(|s| s.id == id).map(|s| (t, s)))
    }

    fn fresh(&mut self) -> Id {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn check_name(&self, name: &str, except: Option<Id>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("name is empty".into());
        }
        if self.find(name).is_some_and(|t| Some(t.id) != except) {
            return Err(format!("a thing named '{name}' already exists"));
        }
        Ok(name.to_string())
    }

    /// The only way to change a model. Returns the id created or affected.
    pub fn apply(&mut self, op: Op) -> Result<Id, String> {
        match op {
            Op::AddThing { name, kind, essence, affiliation, opd, x, y, inside } => {
                let name = self.check_name(&name, None)?;
                let opd = opd.unwrap_or(self.root());
                let (ax, ay) = self.spot(opd);
                let inside = inside.unwrap_or(self.default_inside(opd, kind));
                self.opd(opd).ok_or("no such diagram")?;
                let id = self.fresh();
                self.things.push(Thing {
                    id,
                    name,
                    kind,
                    essence,
                    affiliation,
                    states: vec![],
                    x: 0.0,
                    y: 0.0,
                    auto: false,
                    frame: None,
                    inside: false,
                    detail: None,
                    state_order: vec![],
                });
                let shown = Shown { thing: id, x: x.unwrap_or(ax), y: y.unwrap_or(ay), auto: x.is_none(), inside };
                self.opd_mut(opd)?.shows.push(shown);
                Ok(id)
            }
            Op::Show { opd, thing, x, y, inside } => {
                let kind = self.thing(thing).ok_or("no such thing")?.kind;
                let (ax, ay) = self.spot(opd);
                let default_inside = self.default_inside(opd, kind);
                let o = self.opd_mut(opd)?;
                if let Some(s) = o.shows.iter_mut().find(|s| s.thing == thing) {
                    if let Some(i) = inside {
                        s.inside = i && o.refines != Some(thing);
                    }
                } else {
                    let inside = inside.unwrap_or(default_inside) && o.refines != Some(thing);
                    o.shows.push(Shown { thing, x: x.unwrap_or(ax), y: y.unwrap_or(ay), auto: x.is_none(), inside });
                }
                Ok(thing)
            }
            Op::Hide { opd, thing } => {
                let o = self.opd_mut(opd)?;
                if o.refines == Some(thing) {
                    return Err("this diagram details that thing; delete the diagram instead".into());
                }
                o.shows.retain(|s| s.thing != thing);
                self.cleanup();
                Ok(thing)
            }
            Op::Refine { thing, unfold, from } => {
                let t = self.thing(thing).ok_or("no such thing")?;
                if !unfold && t.kind != Kind::Process {
                    return Err(format!("'{}' is an object: unfold it (in-zooming here is for processes)", t.name));
                }
                if let Some(d) = self.detail_of(thing) {
                    return Ok(d);
                }
                let parent = from.filter(|&f| self.opd(f).is_some()).or_else(|| self.shown_in(thing).first().copied()).unwrap_or(self.root());
                // Start from the thing and what it relates to: its environment when zooming in,
                // its parts, attributes and specializations when unfolding.
                let related: Vec<Id> = self
                    .links
                    .iter()
                    .filter_map(|l| match (unfold, l.kind.is_procedural()) {
                        (false, true) if l.from == thing => Some(l.to),
                        (false, true) if l.to == thing => Some(l.from),
                        (true, false) if l.from == thing => Some(l.to),
                        _ => None,
                    })
                    .collect();
                let mut shows = vec![Shown { thing, x: 0.0, y: 0.0, auto: true, inside: false }];
                for id in related {
                    if !shows.iter().any(|s| s.thing == id) {
                        shows.push(Shown { thing: id, x: 0.0, y: 0.0, auto: true, inside: false });
                    }
                }
                let id = self.fresh();
                self.opds.push(Opd { id, parent: Some(parent), refines: Some(thing), unfold, shows });
                let moves = layout::tidy_opd(self, id);
                let o = self.opd_mut(id)?;
                for (t, x, y) in moves {
                    if let Some(s) = o.shows.iter_mut().find(|s| s.thing == t) {
                        (s.x, s.y) = (x, y);
                    }
                }
                Ok(id)
            }
            Op::RemoveOpd { opd } => {
                if opd == self.root() {
                    return Err("the system diagram can't be deleted".into());
                }
                self.opd(opd).ok_or("no such diagram")?;
                self.opds.retain(|o| o.id != opd);
                self.cleanup();
                Ok(opd)
            }
            Op::AddState { thing, name } => {
                let name = name.trim().to_string();
                let t = self.thing(thing).ok_or("no such thing")?;
                if t.kind != Kind::Object {
                    return Err("only objects have states".into());
                }
                if name.is_empty() || t.states.iter().any(|s| s.name.eq_ignore_ascii_case(&name)) {
                    return Err(format!("invalid or duplicate state '{name}'"));
                }
                let id = self.fresh();
                self.things.iter_mut().find(|t| t.id == thing).unwrap().states.push(State { id, name, initial: false, final_: false });
                Ok(id)
            }
            Op::AddLink { from, to, kind, from_state, to_state } => {
                if from == to {
                    return Err("a thing cannot link to itself".into());
                }
                let f = self.thing(from).ok_or("no such 'from' thing")?;
                let t = self.thing(to).ok_or("no such 'to' thing")?;
                let valid = valid_link_kinds(f.kind, t.kind);
                if !valid.iter().any(|k| same_variant(k, &kind)) {
                    return Err(format!(
                        "{kind:?} is not allowed from {:?} '{}' to {:?} '{}'; allowed: {valid:?}",
                        f.kind, f.name, t.kind, t.name
                    ));
                }
                if from_state.is_some() || to_state.is_some() {
                    let obj = match (kind.is_procedural(), f.kind, t.kind) {
                        (true, Kind::Object, Kind::Process) => f,
                        (true, Kind::Process, Kind::Object) => t,
                        _ => return Err("states only apply to procedural links between an object and a process".into()),
                    };
                    for s in [from_state, to_state].into_iter().flatten() {
                        if !obj.states.iter().any(|st| st.id == s) {
                            return Err(format!("state {s} does not belong to '{}'", obj.name));
                        }
                    }
                }
                if self.links.iter().any(|l| l.from == from && l.to == to && l.kind == kind) {
                    return Err("link already exists".into());
                }
                let id = self.fresh();
                self.links.push(Link { id, from, to, kind, from_state, to_state, modifier: Modifier::None, multiplicity: String::new() });
                Ok(id)
            }
            Op::Rename { id, name } => {
                if self.thing(id).is_some() {
                    let name = self.check_name(&name, Some(id))?;
                    self.things.iter_mut().find(|t| t.id == id).unwrap().name = name;
                } else {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return Err("name is empty".into());
                    }
                    let s = self
                        .things
                        .iter_mut()
                        .flat_map(|t| t.states.iter_mut())
                        .find(|s| s.id == id)
                        .ok_or("no such thing or state")?;
                    s.name = name;
                }
                Ok(id)
            }
            Op::Move { opd, id, x, y, auto } => {
                let opd = opd.unwrap_or(self.root());
                let s = self.opd_mut(opd)?.shows.iter_mut().find(|s| s.thing == id).ok_or("not shown in this diagram")?;
                (s.x, s.y, s.auto) = (x, y, auto);
                Ok(id)
            }
            Op::Update { id, essence, affiliation } => {
                let t = self.things.iter_mut().find(|t| t.id == id).ok_or("no such thing")?;
                if let Some(e) = essence {
                    t.essence = e;
                }
                if let Some(a) = affiliation {
                    t.affiliation = a;
                }
                Ok(id)
            }
            Op::SetLink { id, states, modifier, multiplicity } => {
                let l = self.links.iter().find(|l| l.id == id).ok_or("no such link")?.clone();
                if let Some((a, b)) = states {
                    let (f, t) = (self.thing(l.from).ok_or("no such thing")?, self.thing(l.to).ok_or("no such thing")?);
                    let obj = match (f.kind, t.kind) {
                        (Kind::Object, Kind::Process) => f,
                        (Kind::Process, Kind::Object) => t,
                        _ => return Err("states only apply to links between an object and a process".into()),
                    };
                    if [a, b].into_iter().flatten().any(|s| !obj.states.iter().any(|x| x.id == s)) {
                        return Err(format!("that state does not belong to '{}'", obj.name));
                    }
                }
                if modifier.is_some_and(|m| !m.is_none()) && !l.modifiable() {
                    return Err("only agent, instrument, consumption and effect links can be conditions or events".into());
                }
                let multiplicity = multiplicity.map(|m| m.trim().replace(' ', ""));
                if let Some(m) = multiplicity.as_deref().filter(|m| !m.is_empty()) {
                    if !l.countable() {
                        return Err(format!("{:?} links have no multiplicity", l.kind));
                    }
                    if !valid_multiplicity(m) {
                        return Err(format!("multiplicity '{m}' should look like 4, 0..1, 1..*, 2..5 or *"));
                    }
                }
                let l = self.links.iter_mut().find(|l| l.id == id).unwrap();
                if let Some((a, b)) = states {
                    (l.from_state, l.to_state) = (a, b);
                }
                if let Some(m) = modifier {
                    l.modifier = m;
                }
                if let Some(m) = multiplicity {
                    l.multiplicity = m;
                }
                Ok(id)
            }
            Op::SetState { id, initial, final_ } => {
                let s = self.things.iter_mut().flat_map(|t| t.states.iter_mut()).find(|s| s.id == id).ok_or("no such state")?;
                if let Some(v) = initial {
                    s.initial = v;
                }
                if let Some(v) = final_ {
                    s.final_ = v;
                }
                Ok(id)
            }
            Op::Fan { links, kind } => {
                let mut links = links;
                links.dedup();
                for f in &mut self.fans {
                    f.links.retain(|l| !links.contains(l));
                }
                self.prune_fans();
                let Some(kind) = kind else { return links.first().copied().ok_or_else(|| "no links".into()) };
                let ls: Vec<&Link> = links.iter().map(|id| self.links.iter().find(|l| l.id == *id).ok_or("no such link")).collect::<Result<_, _>>()?;
                if ls.len() < 2 {
                    return Err("a fan needs two or more links".into());
                }
                if !ls.iter().all(|l| l.kind.is_procedural() && l.kind == ls[0].kind) {
                    return Err("a fan groups procedural links of one kind (e.g. all results)".into());
                }
                let fan = Fan { id: 0, kind, links: links.clone() };
                if self.fan_end(&fan).is_none() {
                    return Err("the links of a fan must share one end".into());
                }
                let id = self.fresh();
                self.fans.push(Fan { id, ..fan });
                Ok(id)
            }
            Op::AddRequirement { text, key } => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    return Err("requirement text is empty".into());
                }
                let key = match key.filter(|k| !k.trim().is_empty()) {
                    Some(k) => self.check_key(&k, None)?,
                    None => {
                        let n = self.requirements.iter().filter_map(|r| r.key.strip_prefix("REQ-")?.parse::<u32>().ok()).max().unwrap_or(0);
                        format!("REQ-{}", n + 1)
                    }
                };
                let id = self.fresh();
                self.requirements.push(Requirement { id, key, text, satisfied_by: vec![] });
                Ok(id)
            }
            Op::EditRequirement { id, key, text } => {
                let key = key.map(|k| self.check_key(&k, Some(id))).transpose()?;
                let r = self.requirements.iter_mut().find(|r| r.id == id).ok_or("no such requirement")?;
                if let Some(k) = key {
                    r.key = k;
                }
                if let Some(t) = text.map(|t| t.trim().to_string()) {
                    if t.is_empty() {
                        return Err("requirement text is empty".into());
                    }
                    r.text = t;
                }
                Ok(id)
            }
            Op::Trace { requirement, thing, satisfies } => {
                self.thing(thing).ok_or("no such thing")?;
                let r = self.requirements.iter_mut().find(|r| r.id == requirement).ok_or("no such requirement")?;
                r.satisfied_by.retain(|t| *t != thing);
                if satisfies {
                    r.satisfied_by.push(thing);
                }
                Ok(requirement)
            }
            Op::Remove { id } => {
                if let Some(i) = self.requirements.iter().position(|r| r.id == id) {
                    self.requirements.remove(i);
                    return Ok(id);
                }
                if let Some(i) = self.things.iter().position(|t| t.id == id) {
                    self.things.remove(i);
                    self.links.retain(|l| l.from != id && l.to != id);
                    self.cleanup();
                } else if let Some(i) = self.links.iter().position(|l| l.id == id) {
                    self.links.remove(i);
                    self.prune_fans();
                } else if self.state(id).is_some() {
                    for t in &mut self.things {
                        t.states.retain(|s| s.id != id);
                    }
                    for l in &mut self.links {
                        if l.from_state == Some(id) {
                            l.from_state = None;
                        }
                        if l.to_state == Some(id) {
                            l.to_state = None;
                        }
                    }
                } else {
                    return Err("nothing with that id".into());
                }
                Ok(id)
            }
            Op::Insert { things, links, at, dx, dy, opd } => {
                let at: HashMap<Id, (f64, f64)> = at.into_iter().map(|(id, x, y)| (id, (x, y))).collect();
                let mut map = HashMap::new();
                let mut first = None;
                for t in things {
                    // "X copy" pasted again becomes "X copy 2", not "X copy copy".
                    let base = match t.name.rsplit_once(" copy") {
                        Some((b, n)) if n.trim().chars().all(|c| c.is_ascii_digit()) => b.to_string(),
                        _ => t.name.clone(),
                    };
                    let name = std::iter::once(t.name.clone())
                        .chain((1..).map(|i| if i == 1 { format!("{base} copy") } else { format!("{base} copy {i}") }))
                        .find(|n| self.find(n).is_none())
                        .unwrap();
                    let (essence, affiliation) = (t.essence, t.affiliation);
                    let (x, y) = at.get(&t.id).copied().unwrap_or((t.x, t.y));
                    let (x, y) = (Some(x + dx), Some(y + dy));
                    let id = self.apply(Op::AddThing { name, kind: t.kind, essence, affiliation, opd, x, y, inside: None })?;
                    first.get_or_insert(id);
                    map.insert(t.id, id);
                    for s in t.states {
                        let sid = self.apply(Op::AddState { thing: id, name: s.name })?;
                        self.apply(Op::SetState { id: sid, initial: Some(s.initial), final_: Some(s.final_) })?;
                        map.insert(s.id, sid);
                    }
                }
                for l in links {
                    let (Some(&from), Some(&to)) = (map.get(&l.from), map.get(&l.to)) else { continue };
                    let st = |s: Option<Id>| s.and_then(|s| map.get(&s).copied());
                    let id = self.apply(Op::AddLink { from, to, kind: l.kind, from_state: st(l.from_state), to_state: st(l.to_state) })?;
                    self.apply(Op::SetLink { id, states: None, modifier: Some(l.modifier), multiplicity: Some(l.multiplicity) })?;
                }
                first.ok_or_else(|| "nothing to insert".into())
            }
        }
    }
}
