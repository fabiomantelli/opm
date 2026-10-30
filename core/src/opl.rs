//! Object-Process Language: the English sentences equivalent to the diagram.
use crate::{Affiliation, Essence, Id, Kind, LinkKind, Model, Thing};
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

fn article(word: &str) -> &'static str {
    match word.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    }
}

/// Sentences for one diagram view (see `Model::view`), or for a whole model given directly.
pub fn render(m: &Model) -> Vec<Sentence> {
    let mut out = vec![];
    let name = |id: Id| m.thing(id).map(|t| t.name.clone()).unwrap_or_default();
    let state = |id: Option<Id>| id.and_then(|s| m.state(s)).map(|(_, s)| s.name.clone());

    // In-zoom: "Producing zooms into Measuring, Buffering, and Publishing, in that sequence."
    if let Some(f) = m.things.iter().find(|t| t.frame.is_some()) {
        let mut procs: Vec<&Thing> = m.things.iter().filter(|t| t.inside && t.kind == Kind::Process).collect();
        procs.sort_by(|a, b| a.y.total_cmp(&b.y));
        let objs: Vec<String> = m.things.iter().filter(|t| t.inside && t.kind == Kind::Object).map(|t| t.name.clone()).collect();
        let names: Vec<String> = procs.iter().map(|t| t.name.clone()).collect();
        let mut text = match names.len() {
            0 => String::new(),
            1 => format!("{} zooms into {}", f.name, names[0]),
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
        match t.states.as_slice() {
            [] => {}
            [s] => out.push(Sentence { text: format!("{} is {}.", t.name, s.name), refs: vec![t.id] }),
            ss => {
                let names: Vec<_> = ss.iter().map(|s| s.name.clone()).collect();
                out.push(Sentence { text: format!("{} can be {}.", t.name, list(&names, "or")), refs: vec![t.id] });
            }
        }
    }

    // Aggregation and exhibition are grouped per whole: "Car consists of Engine and Wheels."
    let mut grouped: Vec<(Id, bool)> = vec![];
    for l in &m.links {
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
            let parts: Vec<_> = same.iter().map(|o| name(o.to)).collect();
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
        let qualified = |s: Option<String>| s.map(|s| format!("{s} {o}")).unwrap_or_else(|| o.clone());
        let text = match &l.kind {
            LinkKind::Generalization => format!("{t} is {} {f}.", article(&f)),
            LinkKind::Classification => format!("{t} is an instance of {f}."),
            LinkKind::Tagged(tag) if tag.is_empty() => format!("{f} relates to {t}."),
            LinkKind::Tagged(tag) => format!("{f} {tag} {t}."),
            LinkKind::Agent => format!("{o} handles {p}."),
            LinkKind::Instrument => format!("{p} requires {}.", qualified(state(l.from_state))),
            LinkKind::Consumption => format!("{p} consumes {}.", qualified(state(l.from_state))),
            LinkKind::Result => format!("{p} yields {}.", qualified(state(l.to_state))),
            LinkKind::Effect => match (state(l.from_state), state(l.to_state)) {
                (None, None) => format!("{p} affects {o}."),
                (Some(a), Some(b)) => format!("{p} changes {o} from {a} to {b}."),
                (Some(a), None) => format!("{p} changes {o} from {a}."),
                (None, Some(b)) => format!("{p} changes {o} to {b}."),
            },
            LinkKind::Invocation => format!("{f} invokes {t}."),
            LinkKind::Aggregation | LinkKind::Exhibition => unreachable!(),
        };
        out.push(Sentence { text, refs: vec![l.from, l.id, l.to] });
    }
    out
}

/// Every diagram's sentences, in tree order, under its title.
pub fn render_all(m: &Model) -> Vec<(String, Vec<Sentence>)> {
    m.tree().into_iter().map(|(opd, _)| (m.title(opd), render(&m.view(opd)))).collect()
}
