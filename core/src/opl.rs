//! Object-Process Language: the English sentences equivalent to the diagram.
use crate::{Affiliation, Essence, FanKind, Id, Kind, Link, LinkKind, Modifier, Model};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sentence {
    pub text: String,
    /// Ids of the things/links the sentence talks about (for highlighting).
    pub refs: Vec<Id>,
}

/// "A", "A and B", "A, B, and C" (or with "or").
fn list(items: &[String], conj: &str) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} {conj} {b}"),
        [rest @ .., last] => format!("{}, {conj} {last}", rest.join(", ")),
    }
}

/// "20 ms", "4 PMUs", "0.5".
fn qty(v: f64, unit: &str) -> String {
    if unit.is_empty() { format!("{v}") } else { format!("{v} {unit}") }
}

/// "5 to 20 ms", "at least 5 ms", "at most 20 ms".
fn range(min: Option<f64>, max: Option<f64>, unit: &str) -> Option<String> {
    match (min, max) {
        (Some(a), Some(b)) => Some(format!("{a} to {}", qty(b, unit))),
        (Some(a), None) => Some(format!("at least {}", qty(a, unit))),
        (None, Some(b)) => Some(format!("at most {}", qty(b, unit))),
        (None, None) => None,
    }
}

/// "takes 5 to 20 ms, 10 ms expected".
pub fn duration_text(d: &crate::Duration) -> Option<String> {
    match (range(d.min, d.max, &d.unit), d.expected) {
        (Some(r), Some(e)) => Some(format!("{r}, {} expected", qty(e, &d.unit))),
        (Some(r), None) => Some(r),
        (None, Some(e)) => Some(qty(e, &d.unit)),
        (None, None) => None,
    }
}

fn article(word: &str) -> &'static str {
    match word.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    }
}

/// "Wheel" -> "Wheels", "Battery" -> "Batteries", "Box" -> "Boxes" (last word only).
fn plural(name: &str) -> String {
    let lower = name.to_lowercase();
    let consonant_y = lower.ends_with('y') && !lower.ends_with("ay") && !lower.ends_with("ey") && !lower.ends_with("oy") && !lower.ends_with("uy");
    if consonant_y {
        format!("{}ies", &name[..name.len() - 1])
    } else if ["s", "x", "z", "ch", "sh"].iter().any(|e| lower.ends_with(e)) {
        format!("{name}es")
    } else {
        format!("{name}s")
    }
}

/// "4 Wheels", "an optional Spoiler", "at least one hot Cup", "2 to 5 Seats".
fn counted(multiplicity: &str, state: Option<String>, name: &str) -> String {
    let (prefix, many) = match multiplicity.split_once("..") {
        _ if multiplicity.is_empty() || multiplicity == "1" => (String::new(), false),
        None if multiplicity == "*" => ("any number of".into(), true),
        None => (multiplicity.to_string(), true),
        Some(("0", "1")) => ("an optional".into(), false),
        Some(("1", "*")) => ("at least one".into(), false),
        Some(("0", "*")) => ("any number of".into(), true),
        Some((a, "*")) => (format!("at least {a}"), true),
        Some((a, b)) => (format!("{a} to {b}"), true),
    };
    let noun = if many { plural(name) } else { name.to_string() };
    [prefix, state.unwrap_or_default(), noun].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Sentences for one diagram view (see `Model::view`), or for a whole model given directly.
pub fn render(m: &Model) -> Vec<Sentence> {
    let mut out = vec![];
    let name = |id: Id| m.thing(id).map(|t| t.name.clone()).unwrap_or_default();
    let state = |id: Option<Id>| id.and_then(|s| m.state(s)).map(|(_, s)| s.name.clone());

    // In-zoom: "Producing zooms into Measuring, Buffering, and Publishing, in that sequence."
    if let Some(f) = m.things.iter().find(|t| t.frame.is_some()) {
        let steps = m.timeline();
        let objs: Vec<String> = m.things.iter().filter(|t| t.inside && t.kind == Kind::Object).map(|t| t.name.clone()).collect();
        let names: Vec<String> = steps
            .iter()
            .map(|step| match step.as_slice() {
                [one] => name(*one),
                _ => format!("parallel {}", list(&step.iter().map(|&id| name(id)).collect::<Vec<_>>(), "and")),
            })
            .collect();
        let mut text = match names.len() {
            0 => String::new(),
            1 => format!("{} zooms into {}", f.name, names[0]),
            // A comma before the last step keeps "parallel A and B, and C" unambiguous.
            2 if steps.iter().any(|s| s.len() > 1) => format!("{} zooms into {}, and {}, in that sequence", f.name, names[0], names[1]),
            _ => format!("{} zooms into {}, in that sequence", f.name, list(&names, "and")),
        };
        if !objs.is_empty() {
            text = if text.is_empty() { format!("{} zooms into {}", f.name, list(&objs, "and")) } else { format!("{text}, as well as {}", list(&objs, "and")) };
        }
        if !text.is_empty() {
            let refs = std::iter::once(f.id).chain(m.things.iter().filter(|t| t.inside).map(|t| t.id)).collect();
            out.push(Sentence { text: format!("{text}."), refs });
        }
    }

    for t in &m.things {
        let mut traits = vec![];
        if t.essence == Essence::Physical {
            traits.push("physical".to_string());
        }
        if t.affiliation == Affiliation::Environmental {
            traits.push("environmental".to_string());
        }
        if !traits.is_empty() {
            out.push(Sentence { text: format!("{} is {}.", t.name, list(&traits, "and")), refs: vec![t.id] });
        }
        // An attribute with one state is a value: "Weight of Car is heavy."
        let owner = m.links.iter().find(|l| l.to == t.id && l.kind == LinkKind::Exhibition).map(|l| name(l.from));
        // Quantities: "Latency of Message is 20 ms (allowed: at most 50 ms)."
        if let Some(q) = &t.measure {
            let subject = match &owner {
                Some(o) => format!("{} of {o}", t.name),
                None => t.name.clone(),
            };
            let text = match (q.value, range(q.min, q.max, &q.unit)) {
                (Some(v), Some(r)) => format!("{subject} is {} (allowed: {r}).", qty(v, &q.unit)),
                (Some(v), None) => format!("{subject} is {}.", qty(v, &q.unit)),
                (None, Some(r)) => format!("{subject} ranges {}.", if r.starts_with("at ") { r } else { format!("from {r}") }),
                (None, None) => String::new(),
            };
            if !text.is_empty() {
                out.push(Sentence { text, refs: vec![t.id] });
            }
        }
        if let Some(d) = t.duration.as_ref().and_then(duration_text) {
            out.push(Sentence { text: format!("{} takes {d}.", t.name), refs: vec![t.id] });
        }
        // Inherited states belong to the general's sentence.
        let own: Vec<crate::State> = t.states.iter().filter(|s| !s.inherited).cloned().collect();
        match (own.as_slice(), owner) {
            ([], _) => {}
            (ss, _) if t.more_states => {
                let names: Vec<_> = ss.iter().map(|s| s.name.clone()).chain(["other states".to_string()]).collect();
                out.push(Sentence { text: format!("{} can be {}.", t.name, list(&names, "or")), refs: vec![t.id] });
            }
            ([s], Some(o)) => out.push(Sentence { text: format!("{} of {o} is {}.", t.name, s.name), refs: vec![t.id] }),
            ([s], None) => out.push(Sentence { text: format!("{} is {}.", t.name, s.name), refs: vec![t.id] }),
            (ss, _) => {
                let names: Vec<_> = ss.iter().map(|s| s.name.clone()).collect();
                out.push(Sentence { text: format!("{} can be {}.", t.name, list(&names, "or")), refs: vec![t.id] });
            }
        }
        for s in t.states.iter().filter(|s| !s.inherited) {
            let what = match (s.initial, s.final_, s.default_) {
                (true, true, _) => "initial and final",
                (true, false, true) => "initial and default",
                (true, false, false) => "initial",
                (false, true, true) => "final and default",
                (false, true, false) => "final",
                (false, false, true) => "default",
                _ => continue,
            };
            out.push(Sentence { text: format!("State {} of {} is {what}.", s.name, t.name), refs: vec![t.id] });
        }
    }

    // XOR/OR fans: "Brewing yields either Espresso or Lungo."
    for f in &m.fans {
        let Some(end) = m.fan_end(f) else { continue };
        let ls: Vec<&Link> = f.links.iter().filter_map(|id| m.links.iter().find(|l| l.id == *id)).collect();
        let Some(kind) = ls.first().map(|l| l.kind.clone()) else { continue };
        let members: Vec<String> = ls
            .iter()
            .map(|l| {
                let other = if l.from == end { l.to } else { l.from };
                let st = if kind == LinkKind::Result { l.to_state } else { l.from_state };
                let is_obj = m.thing(other).is_some_and(|t| t.kind == Kind::Object);
                let who = if is_obj { counted(&l.multiplicity, state(st), &name(other)) } else { name(other) };
                match l.probability {
                    Some(p) => format!("{who} ({}%)", (p * 1000.0).round() / 10.0),
                    None => who,
                }
            })
            .collect();
        let alts = match f.kind {
            FanKind::Xor => format!("either {}", list(&members, "or")),
            FanKind::Or => format!("at least one of {}", list(&members, "or")),
        };
        let c = name(end);
        let end_is_process = m.thing(end).is_some_and(|t| t.kind == Kind::Process);
        let text = match (&kind, end_is_process) {
            (LinkKind::Invocation, _) if ls[0].from == end => format!("{c} invokes {alts}."),
            (LinkKind::Invocation, _) => format!("{} invokes {c}.", capitalize(&alts)),
            (LinkKind::Agent, true) => format!("{} handles {c}.", capitalize(&alts)),
            (LinkKind::Agent, false) => format!("{c} handles {alts}."),
            (k, true) => format!("{c} {} {alts}.", verb(k)),
            (k, false) => format!("{} {} {c}.", capitalize(&alts), verb(k)),
        };
        let mut refs = vec![end];
        refs.extend(ls.iter().flat_map(|l| [l.id, l.from, l.to]));
        out.push(Sentence { text, refs });
    }

    // Aggregation and exhibition are grouped per whole: "Car consists of Engine and Wheels." Parts
    // listed inside a semi-folded whole count too.
    let folded = |whole: Id, is_agg: bool| -> Vec<String> {
        m.thing(whole).map_or(vec![], |t| t.parts.iter().filter(|p| p.attribute != is_agg).map(|p| counted(&p.multiplicity, None, &p.name)).collect())
    };
    let mut grouped: Vec<(Id, bool)> = vec![];
    for l in m.links.iter().filter(|l| m.fan_of(l.id).is_none()) {
        let group = match l.kind {
            LinkKind::Aggregation => Some(true),
            LinkKind::Exhibition => Some(false),
            _ => None,
        };
        if let Some(is_agg) = group {
            if grouped.contains(&(l.from, is_agg)) {
                continue;
            }
            grouped.push((l.from, is_agg));
            let same: Vec<_> = m
                .links
                .iter()
                .filter(|o| o.from == l.from && o.kind == l.kind)
                .collect();
            let parts: Vec<_> = same.iter().map(|o| counted(&o.multiplicity, None, &name(o.to))).chain(folded(l.from, is_agg)).collect();
            let verb = if is_agg { "consists of" } else { "exhibits" };
            let mut refs = vec![l.from];
            refs.extend(same.iter().flat_map(|o| [o.id, o.to]));
            out.push(Sentence { text: format!("{} {verb} {}.", name(l.from), list(&parts, "and")), refs });
            continue;
        }

        let (f, t) = (name(l.from), name(l.to));
        let process_first = m.thing(l.from).is_some_and(|x| x.kind == Kind::Process);
        // For procedural links between an object and a process, the process is the subject.
        let (p, o) = if process_first { (f.clone(), t.clone()) } else { (t.clone(), f.clone()) };
        let input = state(l.from_state);
        let obj = |s: Option<String>| counted(&l.multiplicity, s, &o);
        // What the process does to the object: "consumes 2 cold Eggs".
        let pred = match &l.kind {
            LinkKind::Instrument => format!("requires {}", obj(input.clone())),
            LinkKind::Consumption => format!("consumes {}", obj(input.clone())),
            LinkKind::Result => format!("yields {}", obj(state(l.to_state))),
            LinkKind::Effect => match (input.clone(), state(l.to_state)) {
                (None, None) => format!("affects {}", obj(None)),
                (Some(a), Some(b)) => format!("changes {o} from {a} to {b}"),
                (Some(a), None) => format!("changes {o} from {a}"),
                (None, Some(b)) => format!("changes {o} to {b}"),
            },
            _ => String::new(),
        };
        // Condition: "Boiling occurs if Water is cold, otherwise Boiling is skipped."
        let cond = match &input {
            Some(s) => format!("{o} is {s}"),
            None => format!("{o} exists"),
        };
        let not_cond = match &input {
            Some(s) => format!("{o} is not {s}"),
            None => format!("{o} does not exist"),
        };
        let text = match (&l.kind, l.modifier) {
            (LinkKind::Generalization, _) => format!("{t} is {} {f}.", article(&f)),
            (LinkKind::Classification, _) => format!("{t} is an instance of {f}."),
            (LinkKind::Tagged(tag), _) if tag.is_empty() => format!("{f} relates to {}.", counted(&l.multiplicity, None, &t)),
            (LinkKind::Tagged(tag), _) => format!("{f} {tag} {}.", counted(&l.multiplicity, None, &t)),
            (LinkKind::Invocation, _) if l.from == l.to => format!("{f} invokes itself."),
            (LinkKind::Invocation, _) => format!("{f} invokes {t}."),
            (LinkKind::Overtime, _) => match m.thing(l.from).and_then(|p| Some((p.duration.as_ref()?.max?, p.duration.as_ref()?.unit.clone()))) {
                Some((max, u)) => format!("If {f} lasts more than {}, {t} occurs.", qty(max, &u)),
                None => format!("If {f} lasts too long, {t} occurs."),
            },
            (LinkKind::Undertime, _) => match m.thing(l.from).and_then(|p| Some((p.duration.as_ref()?.min?, p.duration.as_ref()?.unit.clone()))) {
                Some((min, u)) => format!("If {f} lasts less than {}, {t} occurs.", qty(min, &u)),
                None => format!("If {f} ends too soon, {t} occurs."),
            },
            (LinkKind::Agent, Modifier::None) => format!("{} handles {p}.", obj(None)),
            (LinkKind::Agent, Modifier::Event) => format!("{} initiates and handles {p}.", obj(None)),
            (LinkKind::Agent, Modifier::Condition) => format!("{p} occurs if {cond}, in which case {o} handles {p}, otherwise {p} is skipped."),
            (LinkKind::Instrument, Modifier::Condition) => format!("{p} occurs if {cond}, otherwise {p} is skipped."),
            (_, Modifier::Condition) => format!("{p} occurs if {cond}, in which case {p} {pred}, otherwise {p} is skipped."),
            (_, Modifier::NotCondition) => format!("{p} occurs if {not_cond}, otherwise {p} is skipped."),
            (_, Modifier::Event) => format!("{} initiates {p}, which {pred}.", capitalize(&obj(input))),
            (_, Modifier::None) => format!("{p} {pred}."),
        };
        // Path label: "Following path a, Sorting yields Good Part."
        let text = if l.path.is_empty() { text } else { format!("Following path {}, {text}", l.path) };
        out.push(Sentence { text, refs: vec![l.from, l.id, l.to] });
    }
    // Semi-folded wholes whose parts are all folded (no structural link drawn here).
    for t in m.things.iter().filter(|t| !t.parts.is_empty()) {
        for (is_agg, verb) in [(true, "consists of"), (false, "exhibits")] {
            let parts = folded(t.id, is_agg);
            if !parts.is_empty() && !grouped.contains(&(t.id, is_agg)) {
                out.push(Sentence { text: format!("{} {verb} {}.", t.name, list(&parts, "and")), refs: vec![t.id] });
            }
        }
    }
    out
}

fn verb(k: &LinkKind) -> &'static str {
    match k {
        LinkKind::Instrument => "requires",
        LinkKind::Consumption => "consumes",
        LinkKind::Result => "yields",
        _ => "affects",
    }
}

/// Every diagram's sentences, in tree order, under its title.
pub fn render_all(m: &Model) -> Vec<(String, Vec<Sentence>)> {
    m.tree().into_iter().map(|(opd, _)| (m.title(opd), render(&m.view(opd)))).collect()
}
