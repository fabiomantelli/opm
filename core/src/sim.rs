//! Step-by-step simulation of one diagram: its processes run in time order (top to bottom, level ones in
//! parallel), consuming, changing and creating objects, so the logic of the model can be watched and
//! checked. Processes take their durations, so the run also tells how long it takes. Invocations jump
//! (a process invoking itself or an earlier one loops), time exceptions start their handlers, and
//! XOR/OR alternatives are picked by their probabilities. The diagram gives the order; what each
//! process needs and does comes from the whole model, drawn or not.
use crate::{FanKind, Id, Kind, Link, LinkKind, Model, Modifier, Thing};
use std::collections::HashMap;

/// Runs after which a simulation stops: a loop that never ends.
pub const MAX_RUNS: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sim {
    /// Objects that exist now, with their current state (None: no states).
    pub exists: HashMap<Id, Option<Id>>,
    /// Processes that start on their own, in time order. Ones only started by an invocation or a time
    /// exception aren't here: they run when started.
    pub order: Vec<Id>,
    pub next: usize,
    /// Started by an invocation or exception: they run before the timeline goes on.
    pub queue: Vec<Id>,
    /// The process that just ran or was skipped.
    pub last: Option<Id>,
    /// Stopped at a process whose inputs aren't there (or in an endless loop).
    pub blocked: bool,
    pub log: Vec<String>,
    /// Simulated time, in seconds: when the current step started, and when the latest run ended.
    pub clock: f64,
    pub now: f64,
    /// Timeline step of each process in `order` (level processes share one: they run in parallel).
    pub step_of: HashMap<Id, usize>,
    /// The step the last process ran in, if it came from the timeline.
    pub last_step: Option<usize>,
    pub runs: usize,
    /// Drives the choice among alternatives; set it before stepping for a different run.
    pub seed: u64,
}

/// The state an object starts or appears in: the one marked initial, else the first.
fn start_state(t: &Thing) -> Option<Id> {
    t.states.iter().find(|s| s.initial).or(t.states.first()).map(|s| s.id)
}

/// The object end of a link between an object and a process.
fn object_end(v: &Model, l: &Link) -> Option<Id> {
    [l.from, l.to].into_iter().find(|&e| v.thing(e).is_some_and(|t| t.kind == Kind::Object))
}

/// "350 µs", "20 ms", "1.5 s", "3 min", "2 h".
pub fn fmt_time(secs: f64) -> String {
    let round = |v: f64| (v * 100.0).round() / 100.0;
    match secs {
        s if s == 0.0 => "0".into(),
        s if s < 0.001 => format!("{} µs", round(s * 1e6)),
        s if s < 1.0 => format!("{} ms", round(s * 1e3)),
        s if s < 60.0 => format!("{} s", round(s)),
        s if s < 3600.0 => format!("{} min", round(s / 60.0)),
        s => format!("{} h", round(s / 3600.0)),
    }
}

impl Sim {
    /// Starts diagram `opd` of `m`. Objects some process here creates don't exist yet.
    pub fn new(m: &Model, opd: Id) -> Sim {
        let view = m.view(opd);
        let v = &view;
        let steps = v.timeline();
        let pos: HashMap<Id, usize> = steps.iter().enumerate().flat_map(|(i, s)| s.iter().map(move |&p| (p, i))).collect();
        // Started by others: handlers of time exceptions, and processes invoked from an earlier step
        // (an invocation back to an earlier step, or to itself, is a loop and leaves it in the timeline).
        let started = |p: Id| {
            m.links.iter().any(|l| {
                l.to == p
                    && match l.kind {
                        LinkKind::Overtime | LinkKind::Undertime => true,
                        LinkKind::Invocation => l.from != p && pos.get(&l.from).is_none_or(|&f| pos.get(&p).is_some_and(|&t| f < t)),
                        _ => false,
                    }
            })
        };
        let order: Vec<Id> = steps.concat().into_iter().filter(|&p| !started(p)).collect();
        let all: Vec<Id> = steps.concat();
        let made: Vec<Id> = m.links.iter().filter(|l| l.kind == LinkKind::Result && all.contains(&l.from)).map(|l| l.to).collect();
        let exists = m.things.iter().filter(|t| t.kind == Kind::Object && !made.contains(&t.id)).map(|t| (t.id, start_state(t))).collect();
        Sim {
            exists,
            order,
            step_of: pos,
            seed: 0x9E37_79B9_7F4A_7C15,
            log: vec!["Start: every object exists except those these processes create.".into()],
            ..Default::default()
        }
    }

    pub fn done(&self) -> bool {
        self.blocked || self.upcoming().is_none()
    }

    /// The process that runs next.
    pub fn upcoming(&self) -> Option<Id> {
        self.queue.first().or(self.order.get(self.next)).copied()
    }

    /// A number in [0, 1) (xorshift).
    fn random(&mut self) -> f64 {
        let mut x = self.seed.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Picks among `links` by their probabilities (unset ones share what's left equally).
    fn pick(&mut self, links: &[&Link]) -> Option<Id> {
        let given: f64 = links.iter().filter_map(|l| l.probability).sum();
        let unset = links.iter().filter(|l| l.probability.is_none()).count();
        let rest = if unset > 0 { (1.0 - given).max(0.0) / unset as f64 } else { 0.0 };
        let weights: Vec<f64> = links.iter().map(|l| l.probability.unwrap_or(rest)).collect();
        let total: f64 = weights.iter().sum();
        if total <= 0.0 {
            return links.first().map(|l| l.id);
        }
        let mut r = self.random() * total;
        for (l, w) in links.iter().zip(weights) {
            if r < w {
                return Some(l.id);
            }
            r -= w;
        }
        links.last().map(|l| l.id)
    }

    /// Runs the next process (`m` is the whole model). Returns false when there is nothing left to run.
    pub fn step(&mut self, m: &Model) -> bool {
        let v = m;
        if self.done() {
            return false;
        }
        if self.runs >= MAX_RUNS {
            self.log.push(format!("Stopped after {MAX_RUNS} runs: a loop never ends (give it a condition that becomes false)."));
            self.blocked = true;
            return false;
        }
        let queued = !self.queue.is_empty();
        let p = self.upcoming().unwrap();
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
        let advance = |s: &mut Sim| {
            if queued {
                s.queue.remove(0);
            } else {
                s.next += 1;
            }
        };
        if let Some(l) = needs.iter().find(|l| l.modifier == Modifier::Condition && !enough(l)) {
            self.log.push(format!("{} skipped: no {}.", name(p), what(l)));
            advance(self);
            return true;
        }
        if let Some(l) = needs.iter().find(|l| !enough(l)) {
            self.log.push(format!("{} can't run: it needs {}.", name(p), what(l)));
            self.blocked = true;
            return false;
        }
        advance(self);
        self.runs += 1;

        // Time: level processes of one timeline step start together; anything else starts when the
        // latest run ended.
        let step = (!queued).then(|| self.step_of.get(&p).copied()).flatten();
        if step.is_none() || step != self.last_step {
            self.clock = self.now;
        }
        self.last_step = step;
        let d = v.thing(p).and_then(|t| t.duration.clone());
        let took = d.as_ref().and_then(|d| d.typical()).unwrap_or(0.0);
        let end = self.clock + took;
        self.now = self.now.max(end);

        // Results that are XOR/OR alternatives: pick which happen.
        let mut skip: Vec<Id> = vec![];
        for f in v.fans.iter().filter(|f| f.links.iter().any(|id| touches.iter().any(|l| l.id == *id && l.kind == LinkKind::Result))) {
            let ls: Vec<&Link> = f.links.iter().filter_map(|id| touches.iter().copied().find(|l| l.id == *id)).collect();
            let keep = self.pick_some(f.kind, &ls);
            skip.extend(ls.iter().map(|l| l.id).filter(|id| !keep.contains(id)));
        }
        let mut did = vec![];
        for l in touches.iter().copied() {
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
                LinkKind::Result if !skip.contains(&l.id) => {
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
        let at = if self.now > 0.0 { format!(" [{} → {}]", fmt_time(self.clock), fmt_time(end)) } else { String::new() };
        self.log.push(format!("{} {list}.{at}", name(p)));

        // Time exceptions: ran longer than its max, or shorter than its min.
        if let Some(d) = &d {
            let scale = crate::Duration::scale(&d.unit).unwrap_or(1.0);
            let over = d.max.is_some_and(|x| took > x * scale + 1e-12);
            let under = d.min.is_some_and(|x| took < x * scale - 1e-12);
            for l in v.links.iter().filter(|l| l.from == p && ((over && l.kind == LinkKind::Overtime) || (under && l.kind == LinkKind::Undertime))) {
                let why = if over { "took longer than its maximum" } else { "ended before its minimum" };
                self.log.push(format!("{} {why}: {} starts.", name(p), name(l.to)));
                self.queue.push(l.to);
            }
        }

        // Invocations: alternatives are picked; an invoked process in the timeline is jumped to (a loop
        // when it is this one or an earlier one), others run next.
        let calls: Vec<&Link> = v.links.iter().filter(|l| l.from == p && l.kind == LinkKind::Invocation).collect();
        let mut chosen: Vec<Id> = vec![];
        let mut seen: Vec<Id> = vec![];
        for l in &calls {
            if seen.contains(&l.id) {
                continue;
            }
            match v.fan_of(l.id) {
                Some(f) => {
                    let ls: Vec<&Link> = f.links.iter().filter_map(|id| calls.iter().copied().find(|c| c.id == *id)).collect();
                    seen.extend(ls.iter().map(|l| l.id));
                    chosen.extend(self.pick_some(f.kind, &ls));
                }
                None => chosen.push(l.id),
            }
        }
        let mut jumped = false;
        for id in chosen {
            let t = calls.iter().find(|l| l.id == id).unwrap().to;
            match self.order.iter().position(|&x| x == t) {
                Some(i) if !jumped && self.queue.is_empty() => {
                    self.next = i;
                    jumped = true;
                    if t == p {
                        self.log.push(format!("{} runs again.", name(p)));
                    }
                }
                _ => self.queue.push(t),
            }
        }
        true
    }

    /// Which links of an XOR (exactly one) or OR (one or more) group happen this time.
    fn pick_some(&mut self, kind: FanKind, ls: &[&Link]) -> Vec<Id> {
        match kind {
            FanKind::Xor => self.pick(ls).into_iter().collect(),
            FanKind::Or => {
                let mut keep: Vec<Id> = vec![];
                for l in ls {
                    if self.random() < l.probability.unwrap_or(0.5) {
                        keep.push(l.id);
                    }
                }
                if keep.is_empty() {
                    keep.extend(self.pick(ls));
                }
                keep
            }
        }
    }
}
