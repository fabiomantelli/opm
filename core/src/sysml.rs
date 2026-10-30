//! SysML v2 textual notation of a model, for tools that speak SysML.
//! Physical objects become part defs, informatical ones item defs, processes action defs with their
//! inputs and outputs as parameters; aggregation and exhibition become features, generalization `:>`,
//! states an enum, and an in-zoomed process its sub-actions in time order.
use crate::{Essence, Id, Kind, LinkKind, Model, Modifier, Thing};

/// `Water` stays bare; `Coffee Machine` becomes `'Coffee Machine'`.
fn name(s: &str) -> String {
    let plain = s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain { s.to_string() } else { format!("'{}'", s.replace('\'', "\\'")) }
}

/// Feature name: "Coffee Machine" -> "coffeeMachine".
fn feature(s: &str) -> String {
    let words: Vec<String> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i == 0 {
            out += w;
        } else {
            let mut c = w.chars();
            out.extend(c.next().map(|f| f.to_ascii_uppercase()));
            out.extend(c);
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) { name(s) } else { out }
}

fn multiplicity(m: &str) -> String {
    if m.is_empty() { String::new() } else { format!("[{m}]") }
}

fn keyword(t: &Thing) -> &'static str {
    match (t.kind, t.essence) {
        (Kind::Process, _) => "action",
        (Kind::Object, Essence::Physical) => "part",
        (Kind::Object, Essence::Informatical) => "item",
    }
}

pub fn export(m: &Model) -> String {
    let thing = |id: Id| m.thing(id);
    let mut out = format!("package {} {{\n", name(&m.name));
    out += "    // Exported from OPM Studio (Object-Process Methodology, ISO 19450).\n";
    for t in &m.things {
        let mut body: Vec<String> = vec![];
        if !t.states.is_empty() {
            body.push(format!("attribute state : {};", name(&format!("{} State", t.name))));
        }
        for l in m.links.iter().filter(|l| l.from == t.id) {
            let Some(o) = thing(l.to) else { continue };
            match &l.kind {
                LinkKind::Aggregation => body.push(format!("{} {} : {}{};", keyword(o), feature(&o.name), name(&o.name), multiplicity(&l.multiplicity))),
                LinkKind::Exhibition => body.push(format!("attribute {} : {}{};", feature(&o.name), name(&o.name), multiplicity(&l.multiplicity))),
                LinkKind::Tagged(tag) => body.push(format!("// {} {} {}", t.name, if tag.is_empty() { "relates to" } else { tag }, o.name)),
                LinkKind::Invocation => body.push(format!("// invokes {}", o.name)),
                _ => {}
            }
        }
        if t.kind == Kind::Process {
            for l in m.links.iter().filter(|l| (l.from == t.id || l.to == t.id) && l.kind.is_procedural() && l.kind != LinkKind::Invocation) {
                let Some(o) = thing(if l.from == t.id { l.to } else { l.from }) else { continue };
                let dir = match l.kind {
                    LinkKind::Result => "out",
                    LinkKind::Effect => "inout",
                    _ => "in",
                };
                let mut note = vec![];
                match l.kind {
                    LinkKind::Agent => note.push("agent".to_string()),
                    LinkKind::Instrument => note.push("instrument".to_string()),
                    _ => {}
                }
                match l.modifier {
                    Modifier::Condition => note.push("condition: skipped if absent".into()),
                    Modifier::Event => note.push("event: starts the action".into()),
                    Modifier::None => {}
                }
                let st = |s: Option<Id>| s.and_then(|s| m.state(s)).map(|(_, s)| s.name.clone());
                if let Some(s) = st(l.from_state) {
                    note.push(format!("in state {s}"));
                }
                if let Some(s) = st(l.to_state) {
                    note.push(format!("to state {s}"));
                }
                if let Some(f) = m.fan_of(l.id) {
                    note.push(format!("{:?} alternative", f.kind).to_uppercase());
                }
                let note = if note.is_empty() { String::new() } else { format!(" // {}", note.join(", ")) };
                body.push(format!("{dir} {} {} : {}{};{note}", keyword(o), feature(&o.name), name(&o.name), multiplicity(&l.multiplicity)));
            }
            // In-zoomed: its subprocesses, in time order.
            if let Some(o) = m.opds.iter().find(|o| o.refines == Some(t.id) && !o.unfold) {
                let mut subs: Vec<(f64, &Thing)> =
                    o.shows.iter().filter(|s| s.inside).filter_map(|s| Some((s.y, thing(s.thing)?))).filter(|(_, x)| x.kind == Kind::Process).collect();
                subs.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (_, s) in &subs {
                    body.push(format!("action {} : {};", feature(&s.name), name(&s.name)));
                }
                for w in subs.windows(2) {
                    body.push(format!("first {} then {};", feature(&w[0].1.name), feature(&w[1].1.name)));
                }
            }
        }
        let general = m.links.iter().find(|l| l.to == t.id && l.kind == LinkKind::Generalization).and_then(|l| thing(l.from));
        let head = format!("{} def {}{}", keyword(t), name(&t.name), general.map(|g| format!(" :> {}", name(&g.name))).unwrap_or_default());
        let mut doc = vec![];
        if t.affiliation == crate::Affiliation::Environmental {
            doc.push("environmental".to_string());
        }
        for r in m.satisfies(t.id) {
            doc.push(format!("satisfies {}", r.key));
        }
        if !doc.is_empty() {
            out += &format!("\n    // {}", doc.join("; "));
        }
        out += &if body.is_empty() { format!("\n    {head};\n") } else { format!("\n    {head} {{\n        {}\n    }}\n", body.join("\n        ")) };
        if !t.states.is_empty() {
            let states: Vec<String> = t.states.iter().map(|s| format!("enum {};", name(&s.name))).collect();
            out += &format!("    enum def {} {{ {} }}\n", name(&format!("{} State", t.name)), states.join(" "));
        }
    }
    for r in &m.requirements {
        let by: Vec<String> = r.satisfied_by.iter().filter_map(|&id| thing(id)).map(|t| name(&t.name)).collect();
        // ponytail: traces as comments; `satisfy` needs requirement and part usages, add them if a tool must check it.
        let traced = if by.is_empty() { "not satisfied yet".to_string() } else { format!("satisfied by {}", by.join(", ")) };
        out += &format!("\n    // {traced}\n    requirement def {} {{\n        doc /* {} */\n    }}\n", name(&r.key), r.text.replace("*/", "* /"));
    }
    out + "}\n"
}
