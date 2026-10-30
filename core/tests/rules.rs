use opm_core::{opl, Essence, Kind, LinkKind, Model, Op};

fn add(m: &mut Model, name: &str, kind: Kind) -> u32 {
    m.apply(Op::AddThing { name: name.into(), kind, essence: Essence::Informatical, affiliation: Default::default(), opd: None, x: None, y: None, inside: None })
        .unwrap()
}

fn link(m: &mut Model, from: u32, to: u32, kind: LinkKind) -> Result<u32, String> {
    m.apply(Op::AddLink { from, to, kind, from_state: None, to_state: None })
}

#[test]
fn coffee_making() {
    let mut m = Model::new("Coffee Making");
    let barista = add(&mut m, "Barista", Kind::Object);
    let water = add(&mut m, "Water", Kind::Object);
    let machine = add(&mut m, "Coffee Machine", Kind::Object);
    let coffee = add(&mut m, "Coffee", Kind::Object);
    let brewing = add(&mut m, "Brewing", Kind::Process);
    let heating = add(&mut m, "Heating", Kind::Process);
    let cold = m.apply(Op::AddState { thing: water, name: "cold".into() }).unwrap();
    let hot = m.apply(Op::AddState { thing: water, name: "hot".into() }).unwrap();
    m.apply(Op::Update { id: machine, essence: Some(Essence::Physical), affiliation: None }).unwrap();

    link(&mut m, barista, brewing, LinkKind::Agent).unwrap();
    link(&mut m, machine, brewing, LinkKind::Instrument).unwrap();
    link(&mut m, water, brewing, LinkKind::Consumption).unwrap();
    link(&mut m, brewing, coffee, LinkKind::Result).unwrap();
    m.apply(Op::AddLink { from: water, to: heating, kind: LinkKind::Effect, from_state: Some(cold), to_state: Some(hot) })
        .unwrap();
    link(&mut m, machine, water, LinkKind::Aggregation).ok(); // "Coffee Machine consists of Water" — silly but valid

    // Invalid: result must go process -> object.
    assert!(link(&mut m, coffee, brewing, LinkKind::Result).is_err());
    // Invalid: a state from another object.
    assert!(m
        .apply(Op::AddLink { from: coffee, to: brewing, kind: LinkKind::Instrument, from_state: Some(hot), to_state: None })
        .is_err());
    // Duplicate names rejected.
    assert!(m.apply(Op::Rename { id: coffee, name: "water".into() }).is_err());

    let text: Vec<_> = opl::render(&m).into_iter().map(|s| s.text).collect();
    assert_eq!(
        text,
        [
            "Water can be cold or hot.",
            "Coffee Machine is physical.",
            "Barista handles Brewing.",
            "Brewing requires Coffee Machine.",
            "Brewing consumes Water.",
            "Brewing yields Coffee.",
            "Heating changes Water from cold to hot.",
            "Coffee Machine consists of Water.",
        ]
    );

    // Removing a thing cascades its links; removing a state clears refs.
    m.apply(Op::Remove { id: cold }).unwrap();
    assert!(opl::render(&m).iter().any(|s| s.text == "Heating changes Water to hot."));
    m.apply(Op::Remove { id: water }).unwrap();
    assert!(m.links.iter().all(|l| l.from != water && l.to != water));

    // Round-trips through JSON.
    let json = serde_json::to_string(&m).unwrap();
    assert_eq!(serde_json::from_str::<Model>(&json).unwrap(), m);
}

fn no_overlap(m: &Model, pos: &[(u32, f64, f64)]) {
    for (i, a) in pos.iter().enumerate() {
        for b in &pos[i + 1..] {
            let (sa, sb) = (opm_core::layout::size(m.thing(a.0).unwrap()), opm_core::layout::size(m.thing(b.0).unwrap()));
            let apart = a.1 + sa.0 <= b.1 || b.1 + sb.0 <= a.1 || a.2 + sa.1 <= b.2 || b.2 + sb.1 <= a.2;
            assert!(apart, "{} overlaps {}", a.0, b.0);
        }
    }
}

#[test]
fn tidy_flows_top_to_bottom() {
    let mut m = Model::new("t");
    let water = add(&mut m, "Water", Kind::Object);
    let coffee = add(&mut m, "Coffee", Kind::Object);
    let serving = add(&mut m, "Serving", Kind::Process); // added before Brewing, must still land after it
    let brewing = add(&mut m, "Brewing", Kind::Process);
    let machine = add(&mut m, "Machine", Kind::Object);
    let part = add(&mut m, "Boiler", Kind::Object);
    let heating = add(&mut m, "Heating", Kind::Process);
    let hot = m.apply(Op::AddState { thing: water, name: "hot".into() }).unwrap();
    m.apply(Op::AddLink { from: water, to: heating, kind: LinkKind::Effect, from_state: None, to_state: Some(hot) }).unwrap();
    link(&mut m, water, brewing, LinkKind::Consumption).unwrap();
    link(&mut m, brewing, coffee, LinkKind::Result).unwrap();
    link(&mut m, coffee, serving, LinkKind::Effect).unwrap();
    link(&mut m, machine, brewing, LinkKind::Instrument).unwrap();
    link(&mut m, machine, part, LinkKind::Aggregation).unwrap();
    link(&mut m, serving, coffee, LinkKind::Result).ok(); // cycle Coffee <-> Serving must not hang or panic

    let pos = opm_core::layout::tidy(&m);
    assert_eq!(pos.len(), m.things.len());
    let y = |id| pos.iter().find(|p| p.0 == id).unwrap().2;
    assert!(y(water) < y(heating), "input above its process");
    assert!(y(heating) < y(brewing), "state change feeds the consumer");
    assert!(y(brewing) < y(coffee) && y(coffee) < y(serving), "result below, then its user");
    assert!(y(machine) < y(part), "part below its whole");
    no_overlap(&m, &pos);
}

#[test]
fn tidy_untangles_independent_chains() {
    // Six input -> process -> result chains, added in scrambled order.
    let mut m = Model::new("t");
    let mut chains = vec![];
    for i in 0..6 {
        let p = add(&mut m, &format!("P{i}"), Kind::Process);
        chains.push(p);
    }
    let order = [3, 0, 5, 1, 4, 2];
    let ins: Vec<_> = order.iter().map(|i| add(&mut m, &format!("In{i}"), Kind::Object)).collect();
    let outs: Vec<_> = order.iter().rev().map(|i| add(&mut m, &format!("Out{i}"), Kind::Object)).collect();
    for (k, &i) in order.iter().enumerate() {
        link(&mut m, ins[k], chains[i], LinkKind::Consumption).unwrap();
        link(&mut m, chains[i], outs[order.len() - 1 - k], LinkKind::Result).unwrap();
    }
    let pos = opm_core::layout::tidy(&m);
    no_overlap(&m, &pos);
    // Straight center-to-center segments must not cross.
    let c = |id: u32| {
        let p = pos.iter().find(|p| p.0 == id).unwrap();
        let (w, h) = opm_core::layout::size(m.thing(id).unwrap());
        (p.1 + w / 2.0, p.2 + h / 2.0)
    };
    let segs: Vec<_> = m.links.iter().map(|l| (c(l.from), c(l.to))).collect();
    let cross = |(a, b): ((f64, f64), (f64, f64)), (p, q): ((f64, f64), (f64, f64))| {
        let o = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        o(a, b, p) * o(a, b, q) < 0.0 && o(p, q, a) * o(p, q, b) < 0.0
    };
    for (i, s1) in segs.iter().enumerate() {
        for s2 in &segs[i + 1..] {
            assert!(!cross(*s1, *s2), "links cross");
        }
    }
}

#[test]
fn tidy_handles_a_big_model() {
    let mut m = Model::new("big");
    let mut procs = vec![];
    for i in 0..60 {
        procs.push(add(&mut m, &format!("Step {i}"), Kind::Process));
        let o = add(&mut m, &format!("Item {i}"), Kind::Object);
        link(&mut m, o, procs[i], LinkKind::Consumption).unwrap();
        if i > 0 {
            let r = add(&mut m, &format!("Part {i}"), Kind::Object);
            link(&mut m, procs[i - 1], r, LinkKind::Result).unwrap();
            link(&mut m, r, procs[i], LinkKind::Instrument).unwrap();
            if i % 7 == 0 {
                link(&mut m, procs[i / 2], procs[i], LinkKind::Invocation).unwrap();
            }
        }
    }
    let t = std::time::Instant::now();
    let pos = opm_core::layout::tidy(&m);
    assert!(t.elapsed().as_secs_f64() < 2.0, "too slow: {:?}", t.elapsed());
    no_overlap(&m, &pos);
}

#[test]
fn insert_copies_with_fresh_ids_and_names() {
    let mut m = Model::new("t");
    let water = add(&mut m, "Water", Kind::Object);
    let boil = add(&mut m, "Boiling", Kind::Process);
    let cold = m.apply(Op::AddState { thing: water, name: "cold".into() }).unwrap();
    let l = m.apply(Op::AddLink { from: water, to: boil, kind: LinkKind::Consumption, from_state: Some(cold), to_state: None }).unwrap();
    let things = m.things.clone();
    let links: Vec<_> = m.links.iter().filter(|x| x.id == l).cloned().collect();
    let at = m.view(m.root()).things.iter().map(|t| (t.id, t.x, t.y)).collect();
    let paste = |things, links, at| Op::Insert { things, links, at, dx: 30.0, dy: 30.0, opd: None };

    let first = m.apply(paste(things, links, at)).unwrap();
    assert_eq!(m.things.len(), 4);
    let v = m.view(m.root());
    let copy = v.thing(first).unwrap();
    assert_eq!(copy.name, "Water copy");
    assert_eq!(copy.x, v.thing(water).unwrap().x + 30.0);
    assert!(opl::render(&m).iter().any(|s| s.text == "Boiling copy consumes cold Water copy."));
    // Pasting again keeps names unique.
    m.apply(paste(vec![m.thing(water).unwrap().clone()], vec![], vec![])).unwrap();
    assert!(m.find("Water copy 2").is_some());
    // Copying a copy counts up instead of stacking "copy copy".
    m.apply(paste(vec![m.find("Water copy").unwrap().clone()], vec![], vec![])).unwrap();
    assert!(m.find("Water copy 3").is_some());
}

fn mv(m: &mut Model, opd: Option<u32>, id: u32, x: f64, y: f64, auto: bool) {
    m.apply(Op::Move { opd, id, x, y, auto }).unwrap();
}

fn assert_apart(v: &Model) {
    use opm_core::layout::size;
    for a in v.things.iter().filter(|t| t.frame.is_none()) {
        for b in v.things.iter().filter(|b| b.id > a.id && b.frame.is_none()) {
            let ((aw, ah), (bw, bh)) = (size(a), size(b));
            assert!(a.x + aw <= b.x || b.x + bw <= a.x || a.y + ah <= b.y || b.y + bh <= a.y, "{} overlaps {}", a.name, b.name);
        }
    }
}

#[test]
fn arrange_keeps_hand_placed_things_and_fits_new_ones_in() {
    use opm_core::layout::arrange_opd;
    let mut m = Model::new("Mixed");
    let sd = m.root();
    let water = add(&mut m, "Water", Kind::Object);
    let brewing = add(&mut m, "Brewing", Kind::Process);
    link(&mut m, water, brewing, LinkKind::Consumption).unwrap();
    // All automatic: a full tidy.
    assert!(m.view(sd).things.iter().all(|t| t.auto));
    assert_eq!(arrange_opd(&m, sd, &[]).len(), 2);

    // The user drags Brewing; Water stays automatic. Then the AI adds a result and a second input.
    mv(&mut m, None, water, 500.0, 100.0, true);
    mv(&mut m, None, brewing, 480.0, 300.0, false);
    let coffee = add(&mut m, "Coffee", Kind::Object);
    let beans = add(&mut m, "Beans", Kind::Object);
    link(&mut m, brewing, coffee, LinkKind::Result).unwrap();
    link(&mut m, beans, brewing, LinkKind::Consumption).unwrap();

    let moves = arrange_opd(&m, sd, &[brewing, coffee, beans]);
    assert_eq!(moves.len(), 2, "only the new things move");
    for (id, x, y) in moves {
        // Results go below the process, inputs above it.
        if id == coffee { assert!(y > 300.0) } else { assert!(y < 300.0) }
        mv(&mut m, None, id, x, y, true);
    }
    assert_apart(&m.view(sd));
}

#[test]
fn old_files_migrate_to_a_root_diagram() {
    let json = r#"{"name":"Old","things":[{"id":1,"name":"Water","kind":"Object","x":40,"y":50},
        {"id":2,"name":"Boiling","kind":"Process","x":300,"y":50,"auto":true}],
        "links":[{"id":3,"from":1,"to":2,"kind":"Consumption"}],"next_id":4}"#;
    let mut m: Model = serde_json::from_str(json).unwrap();
    m.normalize();
    m.validate().unwrap();
    let v = m.view(m.root());
    assert_eq!((v.thing(1).unwrap().x, v.thing(1).unwrap().y), (40.0, 50.0));
    assert!(v.thing(2).unwrap().auto);
    assert_eq!(v.links.len(), 1);
    // Positions are stored per diagram only.
    let out = serde_json::to_string(&m).unwrap();
    assert!(!out.contains("\"x\":40.0,\"y\":50.0,\"auto\":false,\"frame\""));
    assert!(m.next_id > m.root());
}

#[test]
fn in_zooming_refines_a_process_into_its_own_diagram() {
    let mut m = Model::new("Espresso");
    let sd = m.root();
    let beans = add(&mut m, "Beans", Kind::Object);
    let espresso = add(&mut m, "Espresso", Kind::Object);
    let barista = add(&mut m, "Barista", Kind::Object);
    let making = add(&mut m, "Making", Kind::Process);
    link(&mut m, beans, making, LinkKind::Consumption).unwrap();
    link(&mut m, making, espresso, LinkKind::Result).unwrap();
    link(&mut m, barista, making, LinkKind::Agent).unwrap();

    // Objects can't be in-zoomed here; processes can, once.
    assert!(m.apply(Op::Refine { thing: beans, unfold: false, from: None }).is_err());
    let sd1 = m.apply(Op::Refine { thing: making, unfold: false, from: Some(sd) }).unwrap();
    assert_eq!(m.apply(Op::Refine { thing: making, unfold: false, from: None }).unwrap(), sd1);
    assert_eq!(m.label(sd1), "SD1");
    assert_eq!(m.title(sd1), "SD1: Making in-zoomed");
    // Its environment came along, outside.
    assert_eq!(m.view(sd1).things.len(), 4);
    assert_eq!(m.view(sd).thing(making).unwrap().detail, Some(sd1));

    // Subprocesses added in the diagram go inside the frame by default.
    let add_in = |m: &mut Model, name: &str, kind| {
        m.apply(Op::AddThing { name: name.into(), kind, essence: Default::default(), affiliation: Default::default(), opd: Some(sd1), x: None, y: None, inside: None }).unwrap()
    };
    let grinding = add_in(&mut m, "Grinding", Kind::Process);
    let brewing = add_in(&mut m, "Brewing", Kind::Process);
    let ground = add_in(&mut m, "Ground Coffee", Kind::Object);
    m.apply(Op::Show { opd: sd1, thing: ground, x: None, y: None, inside: Some(true) }).unwrap();
    // Before distributing, the outside links still sit on the frame.
    assert!(m.view(sd1).links.iter().any(|l| l.from == beans && l.to == making));
    let issues = opm_core::check::check(&m);
    assert!(issues.iter().any(|i| i.text == "Beans is linked to Making, but to none of its subprocesses."), "{issues:?}");

    link(&mut m, beans, grinding, LinkKind::Consumption).unwrap();
    link(&mut m, grinding, ground, LinkKind::Result).unwrap();
    link(&mut m, ground, brewing, LinkKind::Consumption).unwrap();
    link(&mut m, brewing, espresso, LinkKind::Result).unwrap();
    link(&mut m, barista, brewing, LinkKind::Agent).unwrap();
    let water = add_in(&mut m, "Water", Kind::Object);
    m.apply(Op::Show { opd: sd1, thing: water, x: None, y: None, inside: Some(false) }).unwrap();
    link(&mut m, water, brewing, LinkKind::Consumption).unwrap();

    // Distributed links move from the frame to the subprocesses; the SD keeps the summary.
    let v = m.view(sd1);
    assert!(!v.links.iter().any(|l| l.from == beans && l.to == making));
    assert!(m.view(sd).links.iter().any(|l| l.from == beans && l.to == making));
    assert!(!m.view(sd).things.iter().any(|t| t.id == grinding), "subprocesses live one level down");

    // Balancing: Water is used inside but not shown on Making one level up.
    let issues = opm_core::check::check(&m);
    assert!(issues.iter().any(|i| i.text.contains("Brewing links to Water, but Making doesn't")), "{issues:?}");
    assert!(!issues.iter().any(|i| i.text.contains("none of its subprocesses")), "{issues:?}");

    // Timeline layout: subprocesses top to bottom in order, all inside the frame; nothing overlaps.
    for (id, x, y) in opm_core::layout::tidy_opd(&m, sd1) {
        mv(&mut m, Some(sd1), id, x, y, true);
    }
    let v = m.view(sd1);
    let (g, b) = (v.thing(grinding).unwrap(), v.thing(brewing).unwrap());
    assert!(g.y < b.y, "Grinding yields what Brewing consumes, so it comes first");
    let f = v.thing(making).unwrap();
    let (fw, fh) = f.frame.unwrap();
    for t in v.things.iter().filter(|t| t.inside) {
        let (w, h) = opm_core::layout::size(t);
        // Every corner inside the ellipse.
        for (px, py) in [(t.x, t.y), (t.x + w, t.y), (t.x, t.y + h), (t.x + w, t.y + h)] {
            let (dx, dy) = ((px - f.x - fw / 2.0) / (fw / 2.0), (py - f.y - fh / 2.0) / (fh / 2.0));
            assert!(dx * dx + dy * dy <= 1.0, "{} pokes out of the frame", t.name);
        }
    }
    for t in v.things.iter().filter(|t| !t.inside && t.frame.is_none()) {
        let (w, h) = opm_core::layout::size(t);
        assert!(t.x + w <= f.x || t.x >= f.x + fw || t.y + h <= f.y || t.y >= f.y + fh, "{} sits on the frame", t.name);
    }
    assert_apart(&v);
    let opl: Vec<_> = opl::render(&v).into_iter().map(|s| s.text).collect();
    assert_eq!(opl[0], "Making zooms into Grinding and Brewing, in that sequence, as well as Ground Coffee.");

    // Taking a thing out of one diagram keeps it while another shows it; deleting the diagram drops
    // what only it showed.
    m.apply(Op::Hide { opd: sd1, thing: barista }).unwrap();
    assert!(m.thing(barista).is_some());
    assert!(m.apply(Op::Hide { opd: sd1, thing: making }).is_err());
    m.apply(Op::RemoveOpd { opd: sd1 }).unwrap();
    assert!(m.thing(grinding).is_none() && m.thing(ground).is_none());
    assert!(m.thing(beans).is_some() && m.opd(sd1).is_none());
    assert!(m.links.iter().all(|l| m.thing(l.from).is_some() && m.thing(l.to).is_some()));
    m.validate().unwrap();
}

#[test]
fn removing_a_refined_thing_removes_its_diagrams() {
    let mut m = Model::new("t");
    let car = add(&mut m, "Car", Kind::Object);
    let engine = add(&mut m, "Engine", Kind::Object);
    link(&mut m, car, engine, LinkKind::Aggregation).unwrap();
    let sd1 = m.apply(Op::Refine { thing: car, unfold: true, from: None }).unwrap();
    assert_eq!(m.title(sd1), "SD1: Car unfolded");
    let piston = m
        .apply(Op::AddThing { name: "Piston".into(), kind: Kind::Object, essence: Default::default(), affiliation: Default::default(), opd: Some(sd1), x: None, y: None, inside: None })
        .unwrap();
    m.apply(Op::Remove { id: car }).unwrap();
    assert!(m.opd(sd1).is_none() && m.thing(piston).is_none());
    assert!(m.thing(engine).is_some(), "still shown in the SD");
    m.validate().unwrap();
}

#[test]
fn conditions_events_multiplicity_fans_and_initial_states() {
    use opm_core::{FanKind, Modifier};
    let mut m = Model::new("Car Wash");
    let (car, wheel, water, soap) = (add(&mut m, "Car", Kind::Object), add(&mut m, "Wheel", Kind::Object), add(&mut m, "Water", Kind::Object), add(&mut m, "Soap", Kind::Object));
    let (washing, clean, dirty) = (add(&mut m, "Washing", Kind::Process), add(&mut m, "Clean Car", Kind::Object), add(&mut m, "Dirty Car", Kind::Object));
    let wet = m.apply(Op::AddState { thing: car, name: "wet".into() }).unwrap();
    let dry = m.apply(Op::AddState { thing: car, name: "dry".into() }).unwrap();
    m.apply(Op::SetState { id: dry, initial: Some(true), final_: None }).unwrap();

    let parts = link(&mut m, car, wheel, LinkKind::Aggregation).unwrap();
    m.apply(Op::SetLink { id: parts, states: None, modifier: None, multiplicity: Some("4".into()) }).unwrap();
    assert!(m.apply(Op::SetLink { id: parts, states: None, modifier: None, multiplicity: Some("lots".into()) }).is_err());
    assert!(m.apply(Op::SetLink { id: parts, states: None, modifier: Some(Modifier::Event), multiplicity: None }).is_err(), "structural links have no events");

    let uses = link(&mut m, water, washing, LinkKind::Consumption).unwrap();
    m.apply(Op::SetLink { id: uses, states: None, modifier: Some(Modifier::Condition), multiplicity: None }).unwrap();
    let soap_l = link(&mut m, soap, washing, LinkKind::Instrument).unwrap();
    m.apply(Op::SetLink { id: soap_l, states: None, modifier: Some(Modifier::Event), multiplicity: None }).unwrap();
    let fx = link(&mut m, car, washing, LinkKind::Effect).unwrap();
    m.apply(Op::SetLink { id: fx, states: Some((Some(dry), Some(wet))), modifier: None, multiplicity: None }).unwrap();

    let r1 = link(&mut m, washing, clean, LinkKind::Result).unwrap();
    let r2 = link(&mut m, washing, dirty, LinkKind::Result).unwrap();
    assert!(m.apply(Op::Fan { links: vec![r1, uses], kind: Some(FanKind::Xor) }).is_err(), "different kinds");
    m.apply(Op::Fan { links: vec![r1, r2], kind: Some(FanKind::Xor) }).unwrap();

    let text: Vec<String> = opl::render(&m.view(m.root())).into_iter().map(|s| s.text).collect();
    for want in [
        "State dry of Car is initial.",
        "Car consists of 4 Wheels.",
        "Washing occurs if Water exists, in which case Washing consumes Water, otherwise Washing is skipped.",
        "Soap initiates Washing, which requires Soap.",
        "Washing changes Car from dry to wet.",
        "Washing yields either Clean Car or Dirty Car.",
    ] {
        assert!(text.iter().any(|t| t == want), "missing '{want}' in {text:#?}");
    }
    assert!(!text.iter().any(|t| t == "Washing yields Clean Car."), "fanned links read as one sentence");

    // Deleting a fanned link dissolves the fan; the model stays valid through a JSON round trip.
    m.apply(Op::Remove { id: r2 }).unwrap();
    assert!(m.fans.is_empty());
    let back: Model = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
    back.validate().unwrap();
    assert_eq!(back, m);
    let _ = wet;
}

#[test]
fn requirements_simulation_and_sysml() {
    use opm_core::{check::check, sim::Sim, sysml, Modifier};
    let mut m = Model::new("Espresso");
    let beans = add(&mut m, "Beans", Kind::Object);
    let water = add(&mut m, "Water", Kind::Object);
    let espresso = add(&mut m, "Espresso", Kind::Object);
    let milk = add(&mut m, "Milk", Kind::Object);
    let making = add(&mut m, "Espresso Making", Kind::Process);
    let cold = m.apply(Op::AddState { thing: water, name: "cold".into() }).unwrap();
    let hot = m.apply(Op::AddState { thing: water, name: "hot".into() }).unwrap();
    for (o, k) in [(beans, LinkKind::Consumption), (water, LinkKind::Effect), (milk, LinkKind::Consumption)] {
        link(&mut m, o, making, k).unwrap();
    }
    link(&mut m, making, espresso, LinkKind::Result).unwrap();

    // Requirements: keys number themselves; an untraced one is reported by the checks.
    let r1 = m.apply(Op::AddRequirement { text: "Serve espresso in under 30 s".into(), key: None }).unwrap();
    let r2 = m.apply(Op::AddRequirement { text: "Use fresh beans".into(), key: None }).unwrap();
    assert_eq!(m.requirements.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["REQ-1", "REQ-2"]);
    assert!(m.apply(Op::EditRequirement { id: r2, key: Some("req-1".into()), text: None }).is_err(), "keys are unique");
    m.apply(Op::Trace { requirement: r1, thing: making, satisfies: true }).unwrap();
    assert!(check(&m).iter().any(|i| i.text.starts_with("REQ-2 isn't satisfied")));
    assert!(!check(&m).iter().any(|i| i.text.starts_with("REQ-1")));

    // In-zoom with three steps in time order (top to bottom).
    let sd1 = m.apply(Op::Refine { thing: making, unfold: false, from: None }).unwrap();
    let step = |m: &mut Model, name: &str, y: f64| {
        m.apply(Op::AddThing { name: name.into(), kind: Kind::Process, essence: Essence::Informatical, affiliation: Default::default(), opd: Some(sd1), x: Some(300.0), y: Some(y), inside: Some(true) }).unwrap()
    };
    let (grinding, heating, frothing, extracting) = (step(&mut m, "Grinding", 100.0), step(&mut m, "Heating", 200.0), step(&mut m, "Frothing", 300.0), step(&mut m, "Extracting", 400.0));
    let ground = m.apply(Op::AddThing { name: "Ground Coffee".into(), kind: Kind::Object, essence: Essence::Physical, affiliation: Default::default(), opd: Some(sd1), x: Some(500.0), y: Some(150.0), inside: Some(true) }).unwrap();
    link(&mut m, beans, grinding, LinkKind::Consumption).unwrap();
    link(&mut m, grinding, ground, LinkKind::Result).unwrap();
    m.apply(Op::AddLink { from: water, to: heating, kind: LinkKind::Effect, from_state: Some(cold), to_state: Some(hot) }).unwrap();
    let froth = link(&mut m, milk, frothing, LinkKind::Consumption).unwrap();
    m.apply(Op::SetLink { id: froth, states: None, modifier: Some(Modifier::Condition), multiplicity: None }).unwrap();
    link(&mut m, ground, extracting, LinkKind::Consumption).unwrap();
    m.apply(Op::AddLink { from: water, to: extracting, kind: LinkKind::Consumption, from_state: Some(hot), to_state: None }).unwrap();
    link(&mut m, extracting, espresso, LinkKind::Result).unwrap();
    for o in [beans, water, espresso, milk] {
        m.apply(Op::Show { opd: sd1, thing: o, x: Some(0.0), y: Some(0.0), inside: Some(false) }).unwrap();
    }
    // Not drawn in SD1 still counts: the milk link lives in the model.
    m.apply(Op::Hide { opd: sd1, thing: milk }).unwrap();
    let v = m;
    let mut sim = Sim::new(&v, sd1);
    assert_eq!(sim.order, [grinding, heating, frothing, extracting]);
    assert!(!sim.exists.contains_key(&ground) && !sim.exists.contains_key(&espresso), "made later");
    assert_eq!(sim.exists[&water], Some(cold));
    sim.exists.remove(&milk); // no milk today: frothing is skipped, the rest runs
    while sim.step(&v) {}
    assert!(!sim.blocked, "{:#?}", sim.log);
    assert!(sim.exists.contains_key(&espresso) && !sim.exists.contains_key(&beans) && !sim.exists.contains_key(&ground));
    assert!(!sim.exists.contains_key(&water), "hot water was consumed");
    assert_eq!(
        sim.log[1..],
        [
            "Grinding consumed Beans, yielded Ground Coffee.",
            "Heating changed Water from cold to hot.",
            "Frothing skipped: no Milk.",
            "Extracting consumed Ground Coffee, consumed Water, yielded Espresso.",
        ]
    );
    // Without hot water, extracting can't run.
    let mut sim = Sim::new(&v, sd1);
    sim.exists.insert(water, Some(cold));
    sim.exists.insert(ground, None);
    sim.next = 3;
    assert!(!sim.step(&v) && sim.blocked);
    assert_eq!(sim.log.last().unwrap(), "Extracting can't run: it needs Water (hot).");

    let text = sysml::export(&v);
    println!("{text}"); // see it with --nocapture
    for want in [
        "package Espresso {",
        "action def 'Espresso Making' {",
        "in item beans : Beans;",
        "inout item water : Water;",
        "out item espresso : Espresso;",
        "action grinding : Grinding;",
        "first grinding then heating;",
        "first frothing then extracting;",
        "in item milk : Milk; // condition: skipped if absent",
        "enum def 'Water State' { enum cold; enum hot; }",
        "// satisfies REQ-1",
        "requirement def 'REQ-2' {",
    ] {
        assert!(text.contains(want), "missing `{want}` in\n{text}");
    }
    let _ = r2;
}

#[test]
fn hand_routes_belong_to_one_diagram_and_go_with_their_link() {
    let mut m = Model::new("Routes");
    let a = add(&mut m, "Water", Kind::Object);
    let p = add(&mut m, "Boiling", Kind::Process);
    let l = link(&mut m, a, p, LinkKind::Consumption).unwrap();
    let sd1 = m.apply(Op::Refine { thing: p, unfold: false, from: None }).unwrap();
    m.apply(Op::SetRoute { opd: None, link: l, points: vec![(10.0, 20.0)], from: Some((1.0, 0.5)), to: None }).unwrap();
    assert_eq!(m.view(m.root()).links[0].route.as_ref().unwrap().points, [(10.0, 20.0)]);
    assert!(m.view(sd1).links.iter().all(|x| x.route.is_none()), "only in the SD");
    assert!(m.apply(Op::SetRoute { opd: None, link: l, points: vec![(f64::NAN, 0.0)], from: None, to: None }).is_err());
    let back: Model = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
    back.validate().unwrap();
    assert_eq!(back.opds[0].routes, m.opds[0].routes);
    // Nothing left = automatic again.
    m.apply(Op::SetRoute { opd: None, link: l, points: vec![], from: None, to: None }).unwrap();
    assert!(m.opds[0].routes.is_empty());
    m.apply(Op::SetRoute { opd: None, link: l, points: vec![(1.0, 2.0)], from: None, to: None }).unwrap();
    m.apply(Op::Remove { id: l }).unwrap();
    assert!(m.opds[0].routes.is_empty(), "the route goes with its link");
}

#[test]
fn level_subprocesses_run_in_parallel_and_grouping_zooms_out() {
    use opm_core::sim::Sim;
    // SD: Ore -> Crushing -> Grit -> Washing -> Clean Grit -> Sorting -> Gravel.
    let mut m = Model::new("Quarry");
    let sd = m.root();
    let ore = add(&mut m, "Ore", Kind::Object);
    let grit = add(&mut m, "Grit", Kind::Object);
    let clean = add(&mut m, "Clean Grit", Kind::Object);
    let gravel = add(&mut m, "Gravel", Kind::Object);
    let crushing = add(&mut m, "Crushing", Kind::Process);
    let washing = add(&mut m, "Washing", Kind::Process);
    let sorting = add(&mut m, "Sorting", Kind::Process);
    link(&mut m, ore, crushing, LinkKind::Consumption).unwrap();
    link(&mut m, crushing, grit, LinkKind::Result).unwrap();
    link(&mut m, grit, washing, LinkKind::Consumption).unwrap();
    link(&mut m, washing, clean, LinkKind::Result).unwrap();
    link(&mut m, clean, sorting, LinkKind::Consumption).unwrap();
    link(&mut m, sorting, gravel, LinkKind::Result).unwrap();

    // Out-zoom: Washing and Sorting (and Clean Grit, used only by them) become Finishing.
    let fin = m.apply(Op::Group { opd: None, things: vec![washing, sorting, clean], name: "Finishing".into() }).unwrap();
    let z = m.detail_of(fin).unwrap();
    assert_eq!(m.opd(z).unwrap().parent, Some(sd));
    let top = m.view(sd);
    assert!(top.thing(washing).is_none() && top.thing(clean).is_none(), "the group lives one level down");
    let has = |v: &Model, f: u32, t: u32, k: LinkKind| v.links.iter().any(|l| l.from == f && l.to == t && l.kind == k);
    assert!(has(&top, grit, fin, LinkKind::Consumption) && has(&top, fin, gravel, LinkKind::Result), "the SD keeps the summary");
    let v = m.view(z);
    assert!(v.thing(washing).unwrap().inside && v.thing(clean).unwrap().inside && !v.thing(grit).unwrap().inside);
    assert!(has(&v, grit, washing, LinkKind::Consumption));
    assert!(!opm_core::check::check(&m).iter().any(|i| i.opd == z), "{:?}", opm_core::check::check(&m));
    let opl: Vec<_> = opl::render(&v).into_iter().map(|s| s.text).collect();
    assert_eq!(opl[0], "Finishing zooms into Washing and Sorting, in that sequence, as well as Clean Grit.");
    m.validate().unwrap();

    // Level subprocesses are parallel: in the OPL, the simulation and a re-tidy.
    let polishing = m.apply(Op::AddThing { name: "Polishing".into(), kind: Kind::Process, essence: Essence::Informatical, affiliation: Default::default(), opd: Some(z), x: None, y: None, inside: None }).unwrap();
    link(&mut m, polishing, clean, LinkKind::Effect).unwrap();
    for (id, x, y) in opm_core::layout::tidy_opd(&m, z) {
        mv(&mut m, Some(z), id, x, y, true);
    }
    assert_eq!(m.view(z).timeline().len(), 3, "a new subprocess comes after, not beside");
    let w = m.view(z).thing(washing).unwrap().clone();
    mv(&mut m, Some(z), polishing, w.x + 300.0, w.y + 5.0, true);
    let v = m.view(z);
    assert_eq!(v.timeline(), vec![vec![washing, polishing], vec![sorting]]);
    let opl: Vec<_> = opl::render(&v).into_iter().map(|s| s.text).collect();
    assert_eq!(opl[0], "Finishing zooms into parallel Washing and Polishing, and Sorting, in that sequence, as well as Clean Grit.");
    for (id, x, y) in opm_core::layout::tidy_opd(&m, z) {
        mv(&mut m, Some(z), id, x, y, true);
    }
    let v = m.view(z);
    assert_eq!(v.timeline(), vec![vec![washing, polishing], vec![sorting]], "tidy keeps parallel steps");
    assert_apart(&v);
    assert_eq!(Sim::new(&m, z).order, vec![washing, polishing, sorting]);
    assert!(opm_core::sysml::export(&m).contains("first polishing then sorting;"));
}

#[test]
fn suppressed_states_and_semi_folded_parts_belong_to_one_diagram() {
    let mut m = Model::new("Car");
    let sd = m.root();
    let car = add(&mut m, "Car", Kind::Object);
    let engine = add(&mut m, "Engine", Kind::Object);
    let wheel = add(&mut m, "Wheel", Kind::Object);
    let color = add(&mut m, "Color", Kind::Object);
    let driving = add(&mut m, "Driving", Kind::Process);
    let parked = m.apply(Op::AddState { thing: car, name: "parked".into() }).unwrap();
    let moving = m.apply(Op::AddState { thing: car, name: "moving".into() }).unwrap();
    let broken = m.apply(Op::AddState { thing: car, name: "broken".into() }).unwrap();
    link(&mut m, car, engine, LinkKind::Aggregation).unwrap();
    let w = link(&mut m, car, wheel, LinkKind::Aggregation).unwrap();
    m.apply(Op::SetLink { id: w, states: None, modifier: None, multiplicity: Some("4".into()) }).unwrap();
    link(&mut m, car, color, LinkKind::Exhibition).unwrap();
    m.apply(Op::AddLink { from: car, to: driving, kind: LinkKind::Effect, from_state: Some(parked), to_state: Some(moving) }).unwrap();

    // Suppressing: hidden unless a link here uses it.
    for s in [broken, moving] {
        m.apply(Op::HideState { opd: None, state: s, hidden: true }).unwrap();
    }
    let v = m.view(sd);
    let c = v.thing(car).unwrap();
    assert_eq!(c.states.iter().map(|s| s.id).collect::<Vec<_>>(), vec![parked, moving], "moving is used by a link here");
    assert!(c.more_states);
    let opl: Vec<_> = opl::render(&v).into_iter().map(|s| s.text).collect();
    assert!(opl.contains(&"Car can be parked, moving, or other states.".to_string()), "{opl:?}");

    // Semi-folding lists the parts this diagram doesn't draw, and the OPL keeps them.
    let sd1 = m.apply(Op::Refine { thing: car, unfold: true, from: None }).unwrap();
    m.apply(Op::Hide { opd: sd, thing: engine }).unwrap();
    m.apply(Op::Hide { opd: sd, thing: color }).unwrap();
    m.apply(Op::SemiFold { opd: None, thing: car, on: true }).unwrap();
    let v = m.view(sd);
    let c = v.thing(car).unwrap();
    let names: Vec<_> = c.parts.iter().map(opm_core::layout::part_label).collect();
    assert_eq!(names, ["Engine", "Color"], "Wheel is drawn here, so it isn't listed");
    let (_, h) = opm_core::layout::size(c);
    assert!(h > 96.0, "rows for the parts");
    let opl: Vec<_> = opl::render(&v).into_iter().map(|s| s.text).collect();
    assert!(opl.contains(&"Car consists of 4 Wheels and Engine.".to_string()), "{opl:?}");
    assert!(opl.contains(&"Car exhibits Color.".to_string()), "{opl:?}");
    // Only in that diagram.
    assert!(m.view(sd1).thing(car).unwrap().parts.is_empty() && !m.view(sd1).thing(car).unwrap().more_states);
    assert!(m.apply(Op::SemiFold { opd: None, thing: driving, on: true }).is_err());
    m.validate().unwrap();
    // Survives a save.
    let back: Model = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
    assert_eq!(back.opd(sd).unwrap().semi, vec![car]);
}

#[test]
fn time_values_loops_and_chances() {
    use opm_core::{sim::Sim, Duration, FanKind, Measure, Modifier};
    let mut m = Model::new("PMU");
    let at = |m: &mut Model, name: &str, kind: Kind, x: f64, y: f64| {
        m.apply(Op::AddThing { name: name.into(), kind, essence: Essence::Informatical, affiliation: Default::default(), opd: None, x: Some(x), y: Some(y), inside: None }).unwrap()
    };
    let dur = |m: &mut Model, id: u32, min: Option<f64>, expected: Option<f64>, max: Option<f64>| {
        m.apply(Op::SetDuration { id, duration: Some(Duration { min, expected, max, unit: "ms".into() }) }).unwrap();
    };
    let measuring = at(&mut m, "Measuring", Kind::Process, 0.0, 0.0);
    let filtering = at(&mut m, "Filtering", Kind::Process, 300.0, 0.0);
    let publishing = at(&mut m, "Publishing", Kind::Process, 0.0, 200.0);
    let polling = at(&mut m, "Polling", Kind::Process, 0.0, 300.0);
    let alarming = at(&mut m, "Alarming", Kind::Process, 300.0, 400.0);
    let classifying = at(&mut m, "Classifying", Kind::Process, 0.0, 500.0);
    let queue = at(&mut m, "Queue", Kind::Object, 600.0, 300.0);
    let good = at(&mut m, "Good Frame", Kind::Object, 600.0, 500.0);
    let bad = at(&mut m, "Bad Frame", Kind::Object, 800.0, 500.0);
    let message = at(&mut m, "Message", Kind::Object, 600.0, 0.0);
    let latency = at(&mut m, "Latency", Kind::Object, 800.0, 0.0);
    dur(&mut m, measuring, None, Some(8.0), None);
    dur(&mut m, filtering, None, Some(12.0), None);
    dur(&mut m, publishing, Some(1.0), Some(30.0), Some(20.0));
    dur(&mut m, alarming, None, Some(5.0), None);
    dur(&mut m, polling, None, Some(1.0), None);
    assert!(m.apply(Op::SetDuration { id: queue, duration: Some(Duration { expected: Some(1.0), unit: "ms".into(), ..Default::default() }) }).is_err());
    assert!(m.apply(Op::SetDuration { id: polling, duration: Some(Duration { expected: Some(1.0), unit: "parsecs".into(), ..Default::default() }) }).is_err());

    // Exception: Publishing takes 30 ms but may take 20.
    link(&mut m, publishing, alarming, LinkKind::Overtime).unwrap();
    assert!(link(&mut m, queue, alarming, LinkKind::Overtime).is_err(), "exceptions go between processes");
    // Loop: Polling runs while Queue is pending, and invokes itself.
    let pending = m.apply(Op::AddState { thing: queue, name: "pending".into() }).unwrap();
    let empty = m.apply(Op::AddState { thing: queue, name: "empty".into() }).unwrap();
    let e = m.apply(Op::AddLink { from: queue, to: polling, kind: LinkKind::Effect, from_state: Some(pending), to_state: Some(empty) }).unwrap();
    m.apply(Op::SetLink { id: e, states: None, modifier: Some(Modifier::Condition), multiplicity: None }).unwrap();
    link(&mut m, polling, polling, LinkKind::Invocation).unwrap();
    assert!(link(&mut m, queue, queue, LinkKind::Aggregation).is_err());
    // Alternatives with chances.
    let g = link(&mut m, classifying, good, LinkKind::Result).unwrap();
    let b = link(&mut m, classifying, bad, LinkKind::Result).unwrap();
    m.apply(Op::Fan { links: vec![g, b], kind: Some(FanKind::Xor) }).unwrap();
    m.apply(Op::SetProbability { id: g, probability: Some(1.0) }).unwrap();
    m.apply(Op::SetProbability { id: b, probability: Some(0.0) }).unwrap();
    assert!(m.apply(Op::SetProbability { id: b, probability: Some(1.5) }).is_err());
    // A value outside its range.
    link(&mut m, message, latency, LinkKind::Exhibition).unwrap();
    m.apply(Op::SetMeasure { id: latency, measure: Some(Measure { value: Some(60.0), min: None, max: Some(50.0), unit: "ms".into() }) }).unwrap();
    assert!(m.apply(Op::SetMeasure { id: alarming, measure: Some(Measure { value: Some(1.0), ..Default::default() }) }).is_err());

    let opl: Vec<_> = opl::render(&m.view(m.root())).into_iter().map(|s| s.text).collect();
    for s in [
        "Publishing takes 1 to 20 ms, 30 ms expected.",
        "Latency of Message is 60 ms (allowed: at most 50 ms).",
        "If Publishing lasts more than 20 ms, Alarming occurs.",
        "Polling invokes itself.",
        "Classifying yields either Good Frame (100%) or Bad Frame (0%).",
    ] {
        assert!(opl.contains(&s.to_string()), "missing '{s}' in {opl:?}");
    }
    let issues = opm_core::check::check(&m);
    assert!(issues.iter().any(|i| i.text.contains("Latency is 60 ms, outside")), "{issues:?}");

    let mut sim = Sim::new(&m, m.root());
    assert!(!sim.order.contains(&alarming), "a handler waits for its exception");
    while sim.step(&m) {}
    assert!(!sim.blocked, "{:?}", sim.log);
    let ran: Vec<&str> = sim.log.iter().map(|s| s.as_str()).collect();
    let pos = |needle: &str| ran.iter().position(|s| s.starts_with(needle)).unwrap_or_else(|| panic!("no '{needle}' in {ran:?}"));
    assert!(pos("Publishing took longer") < pos("Alarming ran"));
    assert!(pos("Polling runs again") < pos("Polling skipped"), "the loop ends when its condition fails");
    assert!(ran.iter().any(|s| s.contains("yielded Good Frame")) && !ran.iter().any(|s| s.contains("Bad Frame")));
    // 12 ms (Measuring and Filtering in parallel) + 30 + 5 + 1 = 48 ms.
    assert!((sim.now - 0.048).abs() < 1e-9, "{} {:?}", sim.now, sim.log);

    // An endless loop stops.
    m.apply(Op::Remove { id: e }).unwrap();
    let mut sim = Sim::new(&m, m.root());
    while sim.step(&m) {}
    assert!(sim.blocked && sim.log.last().unwrap().contains("loop never ends"));
    m.validate().unwrap();
}
