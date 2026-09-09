//! `edet-swarm` — run the corpus, search fresh seeds, replay one, or take the
//! distributional reading.
//!
//! The search is deliberately OUT of band. A randomised search inside a gate
//! is a gate that goes red for a reason nobody can reproduce; what goes in the
//! gate is the pinned corpus, and what a search finds becomes a corpus entry.

use std::collections::BTreeMap;

use edet_swarm::driver::Audit;
use edet_swarm::population::Population;
use edet_swarm::run::{run, run_population, Run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let flags = parse(&args);
    let code = match cmd {
        "corpus" => corpus(&flags),
        "search" => search(&flags),
        "replay" => replay(&flags),
        "q2" => q2(&flags),
        _ => {
            usage();
            0
        }
    };
    std::process::exit(code);
}

fn usage() {
    eprintln!(
        "edet-swarm — a population of behaving agents over the real transition function

  corpus                                   run every pinned entry and report
  search  --from N --count N [--population P] [--ticks T] [--members N]
  replay  --seed S --population P --ticks T [--until T] [--members N]
  q2      --seeds N --population P --ticks T [--members N]

populations: {}",
        Population::all_names().join(", ")
    );
}

// --------------------------------------------------------------- the flags --

fn parse(args: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut i = 1;
    while i < args.len() {
        if let Some(name) = args[i].strip_prefix("--") {
            let value = args.get(i + 1).cloned().unwrap_or_else(|| "true".into());
            out.insert(name.to_string(), value);
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn num(f: &BTreeMap<String, String>, name: &str, default: u64) -> u64 {
    f.get(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn text(f: &BTreeMap<String, String>, name: &str, default: &str) -> String {
    f.get(name).cloned().unwrap_or_else(|| default.to_string())
}

fn members(f: &BTreeMap<String, String>) -> Option<usize> {
    f.get("members").and_then(|v| v.parse().ok())
}

// ------------------------------------------------------------- the commands --

fn corpus(_f: &BTreeMap<String, String>) -> i32 {
    let entries = match edet_swarm::corpus::load() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let mut bad = 0;
    for entry in &entries {
        let report = match run(&entry.run) {
            Ok(r) => r,
            Err(e) => {
                println!("{:<26} could not run: {e}", entry.name);
                bad += 1;
                continue;
            }
        };
        let same = report.summary == entry.expect;
        let mark = if same { "pinned" } else { "MOVED" };
        println!(
            "{:<26} {mark:<7} {} ticks, {} members, {} rows, root {}",
            entry.name,
            report.summary.ticks,
            report.summary.members_final,
            report.summary.accepted_minor,
            &report.summary.state_root[..8.min(report.summary.state_root.len())]
        );
        if let Some(v) = &report.violation {
            println!("  {v}");
            bad += 1;
        }
        if !same {
            bad += 1;
        }
    }
    if bad > 0 {
        eprintln!("\n{bad} entries moved. `just swarm-pin` rewrites them — read the diff.");
    }
    i32::from(bad > 0)
}

/// Fresh seeds, in parallel, one seed per thread. Runs are independent by
/// construction, so the only thing parallelism can change is wall clock.
fn search(f: &BTreeMap<String, String>) -> i32 {
    use rayon::prelude::*;
    let from = num(f, "from", 1);
    let count = num(f, "count", 256);
    let population = text(f, "population", "everything");
    let ticks = num(f, "ticks", 365);
    let members = members(f);
    let found: Vec<_> = (from..from + count)
        .into_par_iter()
        .filter_map(|seed| {
            let cfg = Run {
                seed,
                population: population.clone(),
                members,
                ticks,
                // A search wants throughput and a violation named exactly, and
                // it gets both: the tick audit finds it, and the replay in
                // `EveryTransition` names the transition.
                audit: Audit::EveryTick,
                metrics_every: 0,
                measure_ceiling: false,
            };
            run(&cfg).ok().and_then(|r| r.violation)
        })
        .collect();
    for v in &found {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
        println!("{v}");
    }
    if found.is_empty() {
        println!("{count} seeds of `{population}` over {ticks} ticks: no invariant broken.");
        0
    } else {
        1
    }
}

fn replay(f: &BTreeMap<String, String>) -> i32 {
    let ticks = num(f, "ticks", 365);
    let cfg = Run {
        seed: num(f, "seed", 1),
        population: text(f, "population", "everything"),
        members: members(f),
        ticks,
        audit: Audit::EveryTransition,
        metrics_every: num(f, "metrics-every", 30),
        measure_ceiling: false,
    };
    let until = num(f, "until", ticks);
    let mut pop = match Population::named(&cfg.population) {
        Some(p) => p,
        None => {
            eprintln!("no population named {}", cfg.population);
            return 2;
        }
    };
    if let Some(n) = cfg.members {
        pop.resize(n);
    }
    let report = run_population(&cfg, &pop, until);
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    match &report.violation {
        Some(v) => {
            eprintln!("\n{v}");
            1
        }
        None => 0,
    }
}

/// **Every figure is a ratio against a control aged identically.** An epoch
/// advance decays every stake, so a control that has not advanced the same
/// number of epochs is not a control — and an absolute reading off this box is
/// a figure about the box.
fn q2(f: &BTreeMap<String, String>) -> i32 {
    let seeds = num(f, "seeds", 50);
    let population = text(f, "population", "everything");
    let ticks = num(f, "ticks", 365);
    let members = members(f);
    let Some(pop) = Population::named(&population) else {
        eprintln!("no population named {population}");
        return 2;
    };
    let mut pop = pop;
    if let Some(n) = members {
        pop.resize(n);
    }
    if !pop.has_treatment() {
        eprintln!(
            "`{population}` has no treatment seat: the treatment IS the control, and every ratio would read \
             1.000 for want of a comparison. Q2 is for a population with something to replace."
        );
        return 2;
    }
    let control = pop.control();

    // One seed is one thread: runs are independent by construction, so the
    // only thing parallelism can change is wall clock.
    use rayon::prelude::*;
    let pairs: Vec<(Agg, Agg)> = (1..=seeds)
        .into_par_iter()
        .map(|seed| {
            let cfg = Run {
                seed,
                population: population.clone(),
                members,
                ticks,
                audit: Audit::EveryTick,
                metrics_every: ticks,
                measure_ceiling: true,
            };
            let mut t = Agg::default();
            let mut c = Agg::default();
            t.add(&run_population(&cfg, &pop, ticks));
            c.add(&run_population(&cfg, &control, ticks));
            (t, c)
        })
        .collect();
    let mut t = Agg::default();
    let mut c = Agg::default();
    for (a, b) in &pairs {
        t.merge(a);
        c.merge(b);
    }
    println!("\n`{population}`, seeds 1..={seeds}, {ticks} ticks, treatment over a control aged identically\n");
    println!("{:<32} {:^21} {:^21} {:>10}", "", "treatment", "control", "ratio");
    row("insured share", t.insured, t.accepted, c.insured, c.accepted);
    row("deadlocked ticks", t.deadlocked, t.ticks, c.deadlocked, c.ticks);
    row("top-decile capacity", t.top.0, t.top.1, c.top.0, c.top.1);
    row("capacity held by non-conferrers", t.hoarded.0, t.hoarded.1, c.hoarded.0, c.hoarded.1);
    row("rows a ceiling pushed uninsured", t.ceiling, t.rows, c.ceiling, c.rows);
    println!(
        "\nNothing here is pinned and nothing is written into the tree. Quote a figure only\nbeside `just swarm-q2`, this seed range and this population."
    );
    0
}

#[derive(Default)]
struct Agg {
    accepted: u64,
    insured: u64,
    ticks: u64,
    deadlocked: u64,
    rows: u64,
    ceiling: u64,
    top: (u64, u64),
    hoarded: (u64, u64),
}

impl Agg {
    fn merge(&mut self, o: &Agg) {
        self.accepted += o.accepted;
        self.insured += o.insured;
        self.ticks += o.ticks;
        self.deadlocked += o.deadlocked;
        self.rows += o.rows;
        self.ceiling += o.ceiling;
        self.top.0 += o.top.0;
        self.top.1 += o.top.1;
        self.hoarded.0 += o.hoarded.0;
        self.hoarded.1 += o.hoarded.1;
    }

    fn add(&mut self, r: &edet_swarm::run::Report) {
        let s = &r.summary;
        self.accepted += s.accepted_minor;
        self.insured += s.insured_minor;
        self.ticks += s.ticks;
        self.deadlocked += s.deadlocked_ticks;
        self.ceiling += s.ceiling_uninsured;
        self.rows += s.rows_accepted;
        self.top.0 += s.capacity.top_decile.0;
        self.top.1 += s.capacity.top_decile.1;
        self.hoarded.0 += s.capacity.non_conferrer_share.0;
        self.hoarded.1 += s.capacity.non_conferrer_share.1;
    }
}

/// One line of the table: both shares, exactly, and the ratio between them.
///
/// **Both numerator and denominator are printed**, because a ratio alone
/// cannot say whether it is a ratio of anything: `0 / 0` against `0 / 0` is
/// "neither population did this", which reads as 1.000 and means nothing at
/// all. That case prints a dash rather than a number.
fn row(name: &str, tn: u64, td: u64, cn: u64, cd: u64) {
    let f = |n: u64, d: u64| if d == 0 { 0.0 } else { n as f64 / d as f64 };
    let (a, b) = (f(tn, td), f(cn, cd));
    let ratio = if tn == 0 && cn == 0 {
        "        — ".to_string()
    } else if b == 0.0 {
        "        ∞ ".to_string()
    } else {
        format!("{:>10.3}", a / b)
    };
    println!("{name:<32} {tn:>10} / {td:<10} {cn:>10} / {cd:<10} {ratio}");
}
