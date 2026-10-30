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
