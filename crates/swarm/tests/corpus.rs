//! **The gate**: every pinned entry, run in `Audit::EveryTransition`, held to
//! the `Summary` it produced when it was pinned.
//!
//! A moved pin is a change of BEHAVIOUR and has to be READ. A `refused` count
//! that went to zero is a rule that stopped firing; a `state_root` that moved
//! with every other field unchanged is a change to what the ledger COMMITS to,
//! which is a fork; an `expired_rows` that went to zero in `deadbeats` is a
//! sweep that stopped expiring anything.
//!
//! `EDET_SWARM_PIN=1 cargo test -p edet-swarm --test corpus` rewrites
//! `corpus.json` instead of asserting — `just swarm-pin`.

use edet_swarm::corpus;
use edet_swarm::metrics::Summary;

fn pinning() -> bool {
    std::env::var("EDET_SWARM_PIN").is_ok_and(|v| v != "0")
}

/// Run one entry and hold it to its pin, naming every field that moved.
fn check(name: &str) {
    if pinning() {
        return;
    }
    let entries = corpus::load().expect("crates/swarm/corpus.json");
    let entry = entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("corpus.json has no entry named {name} — run `just swarm-pin`"));
    let report = edet_swarm::run::run(&entry.run).expect("a named population");
    if let Some(v) = &report.violation {
        panic!("{name}: {v}");
    }
    let moved = diff(&entry.expect, &report.summary);
    assert!(
        moved.is_empty(),
        "{name} moved:\n  {}\n\n`just swarm-pin` rewrites it — read the diff.",
        moved.join("\n  ")
    );
}

/// Every field that differs, named. A whole-struct `assert_eq!` on a summary
/// carrying ten maps prints two pages nobody reads.
fn diff(want: &Summary, got: &Summary) -> Vec<String> {
    let mut out = Vec::new();
    let mut one = |field: &str, a: String, b: String| {
        if a != b {
            out.push(format!("{field}: pinned {a}, measured {b}"));
        }
    };
    one("format_version", want.format_version.to_string(), got.format_version.to_string());
    one("ticks", want.ticks.to_string(), got.ticks.to_string());
    one("members_final", want.members_final.to_string(), got.members_final.to_string());
    one("seated", want.seated.to_string(), got.seated.to_string());
    one("rows_accepted", want.rows_accepted.to_string(), got.rows_accepted.to_string());
    one("accepted_minor", want.accepted_minor.to_string(), got.accepted_minor.to_string());
    one("insured_minor", want.insured_minor.to_string(), got.insured_minor.to_string());
    one("committed_final", want.committed_final.to_string(), got.committed_final.to_string());
    one("external_seed", want.external_seed.to_string(), got.external_seed.to_string());
    one("refused", format!("{:?}", want.refused), format!("{:?}", got.refused));
    one("declined", want.declined.to_string(), got.declined.to_string());
    one("unbuilt", want.unbuilt.to_string(), got.unbuilt.to_string());
    one("expired_rows", want.expired_rows.to_string(), got.expired_rows.to_string());
    one("open_default_final", want.open_default_final.to_string(), got.open_default_final.to_string());
    one("forfeited_final", want.forfeited_final.to_string(), got.forfeited_final.to_string());
    one("exits", want.exits.to_string(), got.exits.to_string());
    one("enacted", want.enacted.to_string(), got.enacted.to_string());
    one("denied_members", want.denied_members.to_string(), got.denied_members.to_string());
    one("deadlocked_ticks", want.deadlocked_ticks.to_string(), got.deadlocked_ticks.to_string());
    one("ceiling_uninsured", want.ceiling_uninsured.to_string(), got.ceiling_uninsured.to_string());
    one("capacity", format!("{:?}", want.capacity), format!("{:?}", got.capacity));
    one("state_root", want.state_root.clone(), got.state_root.clone());
    for (name, a) in &want.archetypes {
        match got.archetypes.get(name) {
            None => out.push(format!("archetype {name}: pinned, and not seated in this run")),
            Some(b) if a != b => out.push(format!("archetype {name}: pinned {a:?}, measured {b:?}")),
            Some(_) => {}
        }
    }
    for name in got.archetypes.keys() {
        if !want.archetypes.contains_key(name) {
            out.push(format!("archetype {name}: seated, and not in the pin"));
        }
    }
    out
}

macro_rules! entries {
    ($($fn_name:ident => $name:literal,)*) => {
        $(
            #[test]
            fn $fn_name() {
                check($name);
            }
        )*
        /// Every scene the tests above assert, so the meta-test below can hold
        /// this list to `corpus::scenes()`: a scene with a pin and no test is
        /// a pin nothing asserts.
        const ENTRY_NAMES: &[&str] = &[$($name,)*];
    };
}

entries! {
    honest => "honest",
    deadbeats => "deadbeats",
    wash_ring => "wash-ring",
    // **The farm is bounded by the seat reservation** — a row is a stock
    // priced on the cut — and these pins hold the count; `tests/archetypes.rs`
    // carries the ratio at two sizes and the equality over time.
    sybil_farm => "sybil-farm",
    sybil_farm_x10 => "sybil-farm-x10",
    sybil_farm_long => "sybil-farm-long",
    honest_open => "honest-open",
    honest_full => "honest-full",
    griefer => "griefer",
    coalition_third => "coalition-third",
    coalition_half => "coalition-half",
    coalition_two_thirds => "coalition-two-thirds",
    exit_under_suspension => "exit-under-suspension",
    late_defaulter => "late-defaulter",
    hoarders => "hoarders",
    everything => "everything",
    sleeper_debtor_30 => "sleeper-debtor-30",
    sleeper_debtor_90 => "sleeper-debtor-90",
    sleeper_debtor_180 => "sleeper-debtor-180",
    sleeper_debtor_365 => "sleeper-debtor-365",
    sleeper_underwriter_30 => "sleeper-underwriter-30",
    sleeper_underwriter_90 => "sleeper-underwriter-90",
    sleeper_underwriter_180 => "sleeper-underwriter-180",
    sleeper_underwriter_365 => "sleeper-underwriter-365",
}

/// **A scene with no entry is a scene nothing gates.** The list of tests above,
/// the list of scenes and `corpus.json` are three places one fact is written,
/// so this is what keeps them one fact.
#[test]
fn every_scene_has_an_entry_and_every_entry_a_scene() {
    if pinning() {
        return;
    }
    let entries = corpus::load().expect("crates/swarm/corpus.json");
    let pinned: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    for scene in corpus::scenes() {
        let name = &scene.name;
        let e = entries
            .iter()
            .find(|e| &e.name == name)
            .unwrap_or_else(|| panic!("no entry for the scene {name} — run `just swarm-pin`"));
        assert_eq!(e.run.seed, scene.seed, "{name}: the entry's seed is not the scene's");
        assert_eq!(e.run.ticks, scene.ticks, "{name}: the entry's tick count is not the scene's");
        assert_eq!(e.run.members, scene.members, "{name}: the entry's size is not the scene's");
        assert_eq!(
            e.run.audit,
            edet_swarm::driver::Audit::EveryTransition,
            "{name}: the corpus audits every transition"
        );
    }
    assert_eq!(pinned.len(), corpus::scenes().len(), "corpus.json carries an entry no scene names");
    // The third place the list is written: the `entries!` tests above. A scene
    // missing there is regenerated by `swarm-pin` and asserted by nothing.
    let mut tested: Vec<String> = ENTRY_NAMES.iter().map(|s| s.to_string()).collect();
    tested.sort_unstable();
    let mut scenes: Vec<String> = corpus::scenes().iter().map(|s| s.name.clone()).collect();
    scenes.sort_unstable();
    assert_eq!(tested, scenes, "every scene needs an `entries!` test, and every test a scene");
    assert!(
        entries
            .iter()
            .all(|e| e.expect.format_version == edet_swarm::metrics::FORMAT_VERSION),
        "the pinned shape is not this one — a deliberate re-pin, or a stale file"
    );
}

/// `just swarm-pin`. Does nothing unless asked, so a green run of this file is
/// always an ASSERTION and never a rewrite.
#[test]
fn rewriting_the_pin_when_asked() {
    if !pinning() {
        return;
    }
    let entries = corpus::regenerate().expect("every scene runs");
    corpus::save(&entries).expect("crates/swarm/corpus.json");
    eprintln!("rewrote {} entries in {} — READ THE DIFF", entries.len(), corpus::path().display());
}
