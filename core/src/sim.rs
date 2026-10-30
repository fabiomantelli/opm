//! Step-by-step simulation of one diagram: its processes run in time order (top to bottom), consuming,
//! changing and creating objects, so the logic of the model can be watched and checked. The diagram
//! gives the order; what each process needs and does comes from the whole model, drawn or not.
use crate::{FanKind, Id, Kind, Link, LinkKind, Model, Modifier, Thing};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sim {
    /// Objects that exist now, with their current state (None: no states).
    pub exists: HashMap<Id, Option<Id>>,
    /// Processes in the order they run.
    pub order: Vec<Id>,
    pub next: usize,
    /// The process that just ran or was skipped.
    pub last: Option<Id>,
    /// Stopped at a process whose inputs aren't there.
    pub blocked: bool,
    pub log: Vec<String>,
}

/// The state an object starts or appears in: the one marked initial, else the first.
fn start_state(t: &Thing) -> Option<Id> {
    t.states.iter().find(|s| s.initial).or(t.states.first()).map(|s| s.id)
}

/// The object end of a link between an object and a process.
fn object_end(v: &Model, l: &Link) -> Option<Id> {
    [l.from, l.to].into_iter().find(|&e| v.thing(e).is_some_and(|t| t.kind == Kind::Object))
}

impl Sim {
    /// Starts diagram `opd` of `m`. Objects some process here creates don't exist yet.
    pub fn new(m: &Model, opd: Id) -> Sim {
        let view = m.view(opd);
        let v = &view;
        // ponytail: order by position (the layout's time axis), parallel steps one after another left to
        // right; follow invocation links if models need jumps, interleave if parallel races matter.
        let order: Vec<Id> = v.timeline().concat();
        let made: Vec<Id> = v.links.iter().filter(|l| l.kind == LinkKind::Result && order.contains(&l.from)).map(|l| l.to).collect();
        let made: Vec<Id> = m.links.iter().filter(|l| l.kind == LinkKind::Result && order.contains(&l.from)).map(|l| l.to).chain(made).collect();
        let exists = m.things.iter().filter(|t| t.kind == Kind::Object && !made.contains(&t.id)).map(|t| (t.id, start_state(t))).collect();
        Sim { exists, order, log: vec!["Start: every object exists except those these processes create.".into()], ..Default::default() }
    }

    pub fn done(&self) -> bool {
        self.blocked || self.next >= self.order.len()
    }

    /// Runs the next process (`m` is the whole model). Returns false when there is nothing left to run.
    pub fn step(&mut self, m: &Model) -> bool {
        let v = m;
        if self.done() {
            return false;
        }
        let p = self.order[self.next];
        let name = |id: Id| v.thing(id).map_or("?".to_string(), |t| t.name.clone());
        let state = |id: Option<Id>| id.and_then(|s| v.state(s)).map(|(_, s)| s.name.clone());
        self.last = Some(p);
        let touches: Vec<&Link> = v.links.iter().filter(|l| l.from == p || l.to == p).collect();
        let needs: Vec<&Link> = touches.iter().copied().filter(|l| l.modifiable() && object_end(v, l).is_some()).collect();
        // Inputs are judged on what existed before this process ran.
        let before = self.exists.clone();
        let met = |l: &Link| {
            let o = object_end(v, l).unwrap();
            before.get(&o).is_some_and(|cur| l.from_state.is_none() || *cur == l.from_state)
        };
        // Alternatives (XOR/OR) among inputs: one of them is enough.
        let enough = |l: &Link| match v.fan_of(l.id) {
            Some(f) => f.links.iter().filter_map(|id| needs.iter().find(|x| x.id == *id)).any(|x| met(x)),
            None => met(l),
        };
        let what = |l: &Link| {
            let o = name(object_end(v, l).unwrap());
            match state(l.from_state) {
                Some(s) => format!("{o} ({s})"),
                None => o,
            }
        };
        if let Some(l) = needs.iter().find(|l| l.modifier == Modifier::Condition && !enough(l)) {
            self.log.push(format!("{} skipped: no {}.", name(p), what(l)));
            self.next += 1;
            return true;
        }
        if let Some(l) = needs.iter().find(|l| !enough(l)) {
            self.log.push(format!("{} can't run: it needs {}.", name(p), what(l)));
            self.blocked = true;
            return false;
        }
        let mut did = vec![];
        let mut chosen: Vec<Id> = vec![];
        for l in touches {
            let Some(o) = object_end(v, l) else { continue };
            // An input alternative that isn't there just isn't used.
            if l.kind != LinkKind::Result && v.fan_of(l.id).is_some() && !met(l) {
                continue;
            }
            match l.kind {
                LinkKind::Consumption => {
                    self.exists.remove(&o);
                    did.push(format!("consumed {}", name(o)));
                }
                LinkKind::Effect => {
                    let cur = self.exists.get(&o).copied().flatten();
                    let to = l.to_state.or(cur);
                    self.exists.insert(o, to);
                    did.push(match (state(cur), state(to)) {
                        (Some(a), Some(b)) if a != b => format!("changed {} from {a} to {b}", name(o)),
                        _ => format!("affected {}", name(o)),
                    });
                }
                LinkKind::Result => {
                    // ponytail: an XOR result takes its first alternative; let the user pick if branches matter.
                    if let Some(f) = v.fan_of(l.id).filter(|f| f.kind == FanKind::Xor) {
                        if f.links.iter().any(|x| chosen.contains(x)) {
                            continue;
                        }
                        chosen.push(l.id);
                    }
                    let t = v.thing(o).unwrap();
                    let s = l.to_state.or(start_state(t));
                    self.exists.insert(o, s);
                    did.push(match state(s) {
                        Some(s) if l.to_state.is_some() => format!("yielded {} ({s})", name(o)),
                        _ => format!("yielded {}", name(o)),
                    });
                }
                _ => {}
            }
        }
        let list = match did.len() {
            0 => "ran".to_string(),
            _ => did.join(", "),
        };
        self.log.push(format!("{} {list}.", name(p)));
        self.next += 1;
        true
    }
}
