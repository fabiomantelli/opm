//! Model checks: OPM well-formedness and in-zoom balancing, each pointing at the diagram to look at.
use crate::{Id, Kind, LinkKind, Model};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub opd: Id,
    pub text: String,
    pub refs: Vec<Id>,
}

pub fn check(m: &Model) -> Vec<Issue> {
    use LinkKind::*;
    let mut out = vec![];
    let mut push = |opd: Id, text: String, refs: Vec<Id>| out.push(Issue { opd, text, refs });
    let name = |id: Id| m.thing(id).map_or("?", |t| t.name.as_str());
    let home = |id: Id| m.shown_in(id).first().copied().unwrap_or(m.root());
    let procedural = |a: Id, b: Id| m.links.iter().any(|l| l.kind.is_procedural() && ((l.from == a && l.to == b) || (l.from == b && l.to == a)));
    let neighbors = |id: Id| -> Vec<Id> {
        let mut v: Vec<Id> = m
            .links
            .iter()
            .filter(|l| l.kind.is_procedural())
            .filter_map(|l| if l.from == id { Some(l.to) } else if l.to == id { Some(l.from) } else { None })
            .collect();
        v.sort();
        v.dedup();
        v
    };
    // (diagram, in-zoomed process, things inside it)
    let zooms: Vec<(Id, Id, Vec<Id>)> = m.opds.iter().filter(|o| !o.unfold).filter_map(|o| Some((o.id, o.refines?, m.inner(o.id)))).collect();

    for t in &m.things {
        let linked = m.links.iter().any(|l| l.from == t.id || l.to == t.id);
        if !linked && t.kind == Kind::Object {
            push(home(t.id), format!("{} isn't linked to anything.", t.name), vec![t.id]);
        }
        let zoomed = zooms.iter().any(|z| z.1 == t.id);
        let transforms = m.links.iter().any(|l| (l.from == t.id || l.to == t.id) && matches!(l.kind, Consumption | Result | Effect));
        if t.kind == Kind::Process && !zoomed && !transforms {
            push(home(t.id), format!("{} doesn't transform any object: give it a consumption, result or effect link.", t.name), vec![t.id]);
        }
    }

    for (z, p, inner) in &zooms {
        let subs: Vec<Id> = inner.iter().copied().filter(|&i| m.thing(i).is_some_and(|t| t.kind == Kind::Process)).collect();
        match subs.as_slice() {
            [] => push(*z, format!("{} is in-zoomed but has no subprocesses yet: add them inside it.", name(*p)), vec![*p]),
            [s] => push(*z, format!("{} zooms into only one subprocess ({}); in-zooming shows two or more.", name(*p), name(*s)), vec![*p, *s]),
            _ => {}
        }
        if subs.is_empty() {
            continue;
        }
        // Balancing, downward: whatever the process touches, one of its subprocesses must touch.
        for x in neighbors(*p).into_iter().filter(|x| !inner.contains(x)) {
            if !inner.iter().any(|&i| procedural(x, i)) {
                push(*z, format!("{} is linked to {}, but to none of its subprocesses.", name(x), name(*p)), vec![x, *p]);
            }
        }
        // Upward: what the subprocesses touch outside must show on the process one level up.
        for &s in &subs {
            for x in neighbors(s).into_iter().filter(|x| *x != *p && !inner.contains(x)) {
                if m.thing(x).is_some_and(|t| t.kind == Kind::Object) && !procedural(x, *p) {
                    push(*z, format!("{} links to {}, but {} doesn't: link {} to {} one level up.", name(s), name(x), name(*p), name(x), name(*p)), vec![x, s]);
                }
            }
        }
    }
    // Time and quantities.
    for t in &m.things {
        if let Some(q) = &t.measure {
            let v = q.value.unwrap_or(f64::NAN);
            if q.min.is_some_and(|a| v < a) || q.max.is_some_and(|b| v > b) {
                push(home(t.id), format!("{} is {} {}, outside its allowed range.", t.name, v, q.unit), vec![t.id]);
            }
        }
    }
    for l in m.links.iter().filter(|l| matches!(l.kind, Overtime | Undertime)) {
        let d = m.thing(l.from).and_then(|p| p.duration.as_ref());
        let (missing, what) = match l.kind {
            Overtime => (d.and_then(|d| d.max).is_none(), "a maximum"),
            _ => (d.and_then(|d| d.min).is_none(), "a minimum"),
        };
        if missing {
            push(home(l.from), format!("{} has a time exception but no {what} duration: set it in its panel.", name(l.from)), vec![l.from, l.to]);
        }
    }
    for r in &m.requirements {
        if r.satisfied_by.is_empty() {
            push(m.root(), format!("{} isn't satisfied by anything yet: trace it to the thing that meets it.", r.key), vec![]);
        }
    }
    out
}
