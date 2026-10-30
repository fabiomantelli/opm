//! MCP tools so ChatGPT (or any MCP client) can build OPM models by name.
use crate::App;
use opm_core::{check::check, opl, Affiliation, Essence, Id, Kind, LinkKind, Model, Op};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ServerHandler,
};
use serde::Deserialize;
use std::sync::Arc;

const GUIDE: &str = "OPM Studio builds Object-Process Methodology (ISO 19450) models that appear live in the user's editor.
Workflow: create_model -> add_things (all of them in one call) -> add_links (all of them in one call) -> get_model, read the OPL
sentences and the checks back, and fix anything that reads wrong. The layout is automatic after every change and keeps whatever
the user placed by hand; call arrange only when the user asks to reorganize a whole diagram.
Naming: objects are singular nouns in Title Case ('Coffee Machine'); processes are gerunds ('Brewing', 'Water Heating');
states are short lowercase adjectives ('cold', 'hot'). Keep names short (1-3 words). Names are unique in a model: the same
name always means the same thing, in every diagram.
Completeness first: never leave out a thing, state or link the system needs just to keep a diagram small.
Diagrams: one model holds the whole system; diagrams are views of it, in a tree. SD (the system diagram) shows the main
process(es), their inputs, outputs, agents and instruments. A diagram reads best with up to ~20-25 things; refine instead of
cutting:
- zoom_in(process) opens a detail diagram (e.g. SD1) of a process with 3+ steps, already showing what the process touches.
  Then add_things(diagram='SD1') adds its subprocesses (inside the process, in time order: add them in execution order),
  objects used only inside it (inside=true), and other outside things (inside=false). Link outside objects to the specific
  subprocesses that use or yield them (they show up in the diagram automatically). Keep the SD links as the summary: every
  object a subprocess touches is also linked to the parent process; get_model's checks report what's unbalanced.
- unfold(thing) opens a diagram for an object's (or process's) parts, attributes and specializations.
- Refine as deep as useful (SD1.1...), usually 2-3 levels.
Link kinds (from -> to):
- agent: human/actor object -> process ('Barista handles Brewing'); only for humans
- instrument: object -> process, needed but not changed ('Brewing requires Coffee Machine')
- consumption: object -> process, used up ('Brewing consumes Water')
- result: process -> object, created ('Brewing yields Coffee')
- effect: object -> process, changed; give from_state/to_state for a state change ('Heating changes Water from cold to hot')
- invocation: process -> process ('Brewing invokes Serving')
- aggregation: whole -> part ('Car consists of Engine')
- exhibition: thing -> attribute/operation ('Car exhibits Color')
- generalization: general -> specialization ('Sedan is a Car')
- classification: class -> instance
- tagged: any -> any with a custom 'tag' phrase
Mark physical things physical=true and external ones environmental=true. Every process should transform at least one object
(consumption, result or effect); avoid things with no links.";

/// Lays out every diagram where something in `touched` (just added or linked) shows (see `layout::arrange_opd`).
fn auto_arrange(m: &mut Model, touched: &[Id]) -> Result<(), String> {
    for (opd, _) in m.tree() {
        let here: Vec<Id> = m.opd(opd).map(|o| o.shows.iter().map(|s| s.thing).filter(|t| touched.contains(t)).collect()).unwrap_or_default();
        if here.is_empty() {
            continue;
        }
        for (id, x, y) in opm_core::layout::arrange_opd(m, opd, &here) {
            m.apply(Op::Move { opd: Some(opd), id, x, y, auto: true })?;
        }
    }
    Ok(())
}

/// A diagram by label ("SD", "SD1.2") or by the name of the thing it refines; none = the SD.
fn diagram(m: &Model, name: &Option<String>) -> Result<Id, String> {
    let Some(name) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) else { return Ok(m.root()) };
    m.tree()
        .into_iter()
        .map(|(id, _)| id)
        .find(|&id| {
            m.label(id).eq_ignore_ascii_case(name)
                || m.title(id).eq_ignore_ascii_case(name)
                || m.opd(id).and_then(|o| m.thing(o.refines?)).is_some_and(|t| t.name.eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| format!("no diagram '{name}'; diagrams: {}", m.tree().iter().map(|(id, _)| m.label(*id)).collect::<Vec<_>>().join(", ")))
}

#[derive(Clone)]
pub struct Mcp {
    app: Arc<App>,
    #[allow(dead_code)] // used by #[tool_handler]
    tool_router: ToolRouter<Self>,
}

pub fn service(app: Arc<App>) -> StreamableHttpService<Mcp, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(Mcp { app: app.clone(), tool_router: Mcp::tool_router() }),
        Arc::new(LocalSessionManager::default()),
        // Reached through a tunnel with an unknown host; the secret path is the guard.
        StreamableHttpServerConfig::default().disable_allowed_hosts(),
    )
}

#[derive(Deserialize, JsonSchema)]
pub struct ModelRef {
    /// Model id returned by create_model or list_models.
    model: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiagramRef {
    model: String,
    /// Diagram label ("SD", "SD1") or the name of the thing it refines. Default: SD.
    #[serde(default)]
    diagram: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ThingRef {
    model: String,
    /// Name of the thing to refine.
    thing: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateModel {
    /// Human name of the system, e.g. "Coffee Making".
    name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ThingSpec {
    name: String,
    /// "object" or "process".
    kind: String,
    #[serde(default)]
    physical: bool,
    #[serde(default)]
    environmental: bool,
    /// State names, objects only (e.g. ["cold", "hot"]).
    #[serde(default)]
    states: Vec<String>,
    /// In an in-zoom diagram: inside the zoomed process (default: processes inside, objects outside).
    #[serde(default)]
    inside: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AddThings {
    model: String,
    /// Diagram to show them in: label ("SD1") or the name of the refined thing. Default: SD.
    /// Things that already exist are shown there too (same thing, another view).
    #[serde(default)]
    diagram: Option<String>,
    things: Vec<ThingSpec>,
}

#[derive(Deserialize, JsonSchema)]
pub struct LinkSpec {
    /// Name of the source thing.
    from: String,
    /// Name of the target thing.
    to: String,
    /// agent | instrument | consumption | result | effect | invocation | aggregation | exhibition | generalization | classification | tagged
    kind: String,
    /// Input state of the object end (consumption/instrument/agent/effect).
    #[serde(default)]
    from_state: Option<String>,
    /// Output state of the object end (result/effect).
    #[serde(default)]
    to_state: Option<String>,
    /// Phrase for tagged links, e.g. "is connected to".
    #[serde(default)]
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AddLinks {
    model: String,
    links: Vec<LinkSpec>,
}

#[derive(Deserialize, JsonSchema)]
pub struct Remove {
    model: String,
    /// Things to delete from the whole model (their links and detail diagrams go too).
    #[serde(default)]
    things: Vec<String>,
    /// Links to delete; only from/to/kind are matched.
    #[serde(default)]
    links: Vec<LinkSpec>,
    /// If set, `things` are only taken out of this diagram and stay in the model.
    #[serde(default)]
    from_diagram: Option<String>,
}

fn link_kind(s: &str, tag: Option<String>) -> Result<LinkKind, String> {
    use LinkKind::*;
    Ok(match s.trim().to_lowercase().as_str() {
        "agent" => Agent,
        "instrument" => Instrument,
        "consumption" => Consumption,
        "result" => Result,
        "effect" => Effect,
        "invocation" => Invocation,
        "aggregation" => Aggregation,
        "exhibition" => Exhibition,
        "generalization" => Generalization,
        "classification" => Classification,
        "tagged" => Tagged(tag.unwrap_or_default()),
        other => return Err(format!("unknown link kind '{other}'")),
    })
}

fn thing_id(m: &Model, name: &str) -> Result<Id, String> {
    m.find(name).map(|t| t.id).ok_or_else(|| format!("no thing named '{name}'; add it with add_things first"))
}

fn state_id(m: &Model, a: Id, b: Id, state: &Option<String>) -> Result<Option<Id>, String> {
    let Some(name) = state else { return Ok(None) };
    [a, b]
        .iter()
        .filter_map(|&id| m.thing(id))
        .flat_map(|t| &t.states)
        .find(|s| s.name.eq_ignore_ascii_case(name.trim()))
        .map(|s| Some(s.id))
        .ok_or_else(|| format!("no state '{name}' on the linked object"))
}

/// Diagram tree, OPL per diagram, and the checks: everything needed to verify the model.
fn summary(id: &str, m: &Model) -> String {
    let tree: Vec<String> = m.tree().into_iter().map(|(d, depth)| format!("{}{}", "  ".repeat(depth), m.title(d))).collect();
    let mut out = format!("Model '{}' (id: {id})\nDiagrams:\n{}\n", m.name, tree.join("\n"));
    for (title, sentences) in opl::render_all(m) {
        out += &format!("\nOPL of {title}:\n");
        for s in sentences {
            out += &format!("{}\n", s.text);
        }
    }
    let issues = check(m);
    if issues.is_empty() {
        out += "\nChecks: all good.";
    } else {
        out += "\nChecks to fix:\n";
        for i in issues {
            out += &format!("- [{}] {}\n", m.label(i.opd), i.text);
        }
    }
    out
}

#[tool_router]
impl Mcp {
    #[tool(description = "List existing OPM models as id: name.")]
    async fn list_models(&self) -> String {
        let v: Vec<_> = self.app.list().into_iter().map(|(id, n)| format!("{id}: {n}")).collect();
        if v.is_empty() { "no models yet".into() } else { v.join("\n") }
    }

    #[tool(description = "Create an OPM model (or reuse the one with that name). Returns its id. Read the server instructions for the modeling guide.")]
    async fn create_model(&self, Parameters(p): Parameters<CreateModel>) -> Result<String, String> {
        let id = self.app.create(&p.name)?;
        Ok(format!("model id: {id}\n\n{GUIDE}"))
    }

    #[tool(description = "Add objects and processes (with optional states) to a diagram (default SD). Existing names are shown in that diagram too and only get the missing states. All-or-nothing.")]
    async fn add_things(&self, Parameters(p): Parameters<AddThings>) -> Result<String, String> {
        self.app.edit(&p.model, "AI", |m| {
            let opd = diagram(m, &p.diagram)?;
            let mut touched = vec![];
            for t in p.things {
                let kind = match t.kind.trim().to_lowercase().as_str() {
                    "object" => Kind::Object,
                    "process" => Kind::Process,
                    other => return Err(format!("'{}': kind must be object or process, got '{other}'", t.name)),
                };
                let id = match m.find(&t.name) {
                    Some(existing) => {
                        let id = existing.id;
                        m.apply(Op::Show { opd, thing: id, x: None, y: None, inside: t.inside })?;
                        id
                    }
                    None => m.apply(Op::AddThing {
                        name: t.name.clone(),
                        kind,
                        essence: if t.physical { Essence::Physical } else { Essence::Informatical },
                        affiliation: if t.environmental { Affiliation::Environmental } else { Affiliation::Systemic },
                        opd: Some(opd),
                        x: None,
                        y: None,
                        inside: t.inside,
                    })?,
                };
                touched.push(id);
                for s in t.states {
                    if !m.thing(id).unwrap().states.iter().any(|x| x.name.eq_ignore_ascii_case(&s)) {
                        m.apply(Op::AddState { thing: id, name: s }).map_err(|e| format!("'{}': {e}", t.name))?;
                    }
                }
            }
            auto_arrange(m, &touched)?;
            Ok(summary(&p.model, m))
        })
    }

    #[tool(description = "Add links between things by name. Links belong to the model and show in every diagram that shows both ends; linking an outside thing to a subprocess shows it in that in-zoom diagram. Invalid kinds are rejected with the allowed list. All-or-nothing.")]
    async fn add_links(&self, Parameters(p): Parameters<AddLinks>) -> Result<String, String> {
        self.app.edit(&p.model, "AI", |m| {
            let mut touched = vec![];
            for l in p.links {
                let (from, to) = (thing_id(m, &l.from)?, thing_id(m, &l.to)?);
                touched.extend([from, to]);
                let op = Op::AddLink {
                    from,
                    to,
                    kind: link_kind(&l.kind, l.tag)?,
                    from_state: state_id(m, from, to, &l.from_state)?,
                    to_state: state_id(m, from, to, &l.to_state)?,
                };
                m.apply(op).map_err(|e| format!("{} -> {}: {e}", l.from, l.to))?;
                // A thing linked to what's inside an in-zoomed process joins that diagram (outside it).
                for (opd, inner) in m.opds.iter().filter(|o| !o.unfold && o.refines.is_some()).map(|o| (o.id, m.inner(o.id))).collect::<Vec<_>>() {
                    for (a, b) in [(from, to), (to, from)] {
                        if inner.contains(&a) && !m.shown_in(b).contains(&opd) {
                            m.apply(Op::Show { opd, thing: b, x: None, y: None, inside: Some(false) })?;
                        }
                    }
                }
            }
            auto_arrange(m, &touched)?;
            Ok(summary(&p.model, m))
        })
    }

    #[tool(description = "Open a detail diagram for a process (in-zooming): its subprocesses go inside it, in time order. Returns the diagram label to pass as `diagram` to add_things. Reuses the existing one if any.")]
    async fn zoom_in(&self, Parameters(p): Parameters<ThingRef>) -> Result<String, String> {
        self.app.edit(&p.model, "AI", |m| {
            let id = thing_id(m, &p.thing)?;
            let opd = m.apply(Op::Refine { thing: id, unfold: false, from: None })?;
            Ok(format!("{} — add subprocesses with add_things(diagram='{}')\n\n{}", m.title(opd), m.label(opd), summary(&p.model, m)))
        })
    }

    #[tool(description = "Open a detail diagram showing a thing's parts, attributes and specializations (unfolding). Returns the diagram label.")]
    async fn unfold(&self, Parameters(p): Parameters<ThingRef>) -> Result<String, String> {
        self.app.edit(&p.model, "AI", |m| {
            let id = thing_id(m, &p.thing)?;
            let opd = m.apply(Op::Refine { thing: id, unfold: true, from: None })?;
            Ok(format!("{} — add parts with add_things(diagram='{}') and aggregation/exhibition links\n\n{}", m.title(opd), m.label(opd), summary(&p.model, m)))
        })
    }

    #[tool(description = "Remove things (from the whole model, or only from one diagram with from_diagram) and/or specific links, by name.")]
    async fn remove(&self, Parameters(p): Parameters<Remove>) -> Result<String, String> {
        self.app.edit(&p.model, "AI", |m| {
            for l in p.links {
                let (from, to, kind) = (thing_id(m, &l.from)?, thing_id(m, &l.to)?, link_kind(&l.kind, l.tag)?);
                let id = m
                    .links
                    .iter()
                    .find(|x| x.from == from && x.to == to && std::mem::discriminant(&x.kind) == std::mem::discriminant(&kind))
                    .ok_or_else(|| format!("no {} link {} -> {}", l.kind, l.from, l.to))?
                    .id;
                m.apply(Op::Remove { id })?;
            }
            let from = p.from_diagram.as_ref().map(|_| diagram(m, &p.from_diagram)).transpose()?;
            for name in p.things {
                let id = thing_id(m, &name)?;
                m.apply(match from {
                    Some(opd) => Op::Hide { opd, thing: id },
                    None => Op::Remove { id },
                })?;
            }
            auto_arrange(m, &[])?;
            Ok(summary(&p.model, m))
        })
    }

    #[tool(description = "Re-lay a WHOLE diagram (default SD), discarding the user's manual positions there. Layout is already automatic after every change: only call this when the user asks to reorganize.")]
    async fn arrange(&self, Parameters(p): Parameters<DiagramRef>) -> Result<String, String> {
        self.app.edit(&p.model, "AI", |m| {
            let opd = diagram(m, &p.diagram)?;
            for (id, x, y) in opm_core::layout::tidy_opd(m, opd) {
                m.apply(Op::Move { opd: Some(opd), id, x, y, auto: true })?;
            }
            Ok(format!("arranged {}", m.title(opd)))
        })
    }

    #[tool(description = "Read a model back: its diagram tree, the OPL sentences of every diagram, and the checks (well-formedness and in-zoom balancing) still to fix.")]
    async fn get_model(&self, Parameters(p): Parameters<ModelRef>) -> Result<String, String> {
        let m = self.app.get(&p.model).ok_or_else(|| format!("no model '{}'", p.model))?;
        Ok(summary(&p.model, &m))
    }
}

#[tool_handler]
impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(GUIDE)
    }
}
