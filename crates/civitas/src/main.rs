//! `edet-civitas` — live a community of model-driven members over the real
//! ledger, resume it, replay it, and read it.
//!
//! Run BY HAND. A gate that calls a model is a gate whose verdict is a sample,
//! so nothing here is in `just ci`, and nothing it prints is a measurement.

use std::collections::BTreeMap;
use std::path::PathBuf;

use edet_civitas::config::{Backend, RunConfig};
use edet_civitas::run::{self, Options, Runner};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let (positional, flags) = parse(&args);
    let out = match cmd {
        "config" => config(),
        "run" => live_new(&flags),
        "resume" => resume(&positional, &flags),
        "replay" => replay(&positional, &flags),
        "report" => dir_of(&positional).and_then(|d| edet_civitas::report::violations(&d)),
        "queue" => dir_of(&positional).and_then(|d| edet_civitas::report::queue(&d)),
        "index" => dir_of(&positional).and_then(|d| edet_civitas::index::build(&d)),
        "events" => dir_of(&positional).and_then(|d| events(&d)),
        "shape" => shape(&flags),
        "prompt" => prompt_sizes(&flags),
        _ => {
            usage();
            Ok(String::new())
        }
    };
    match out {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
        }
        Err(e) => {
            eprintln!("edet-civitas: {e}");
            std::process::exit(1);
        }
    }
}

fn usage() {
    eprintln!(
        "edet-civitas — a community of model-driven members over the real ledger

  config                                   print the default run configuration
  run     --config FILE --dir DIR [--control] [--scripted] [--days N] [--api-key-file F]
          [--price IN,OUT,READ,WRITE] [--cap-dollars N]
  resume  DIR [--days N] [--model M --allow-model-change] [--api-key-file F]
          [--price IN,OUT,READ,WRITE] [--cap-dollars N]
  replay  DIR [--until DAY]                replay the tape with no model call and check it
  report  DIR                              the violation report
  queue   DIR                              the compile queue
  index   DIR                              write DIR/player/ for the player
  events  DIR                              the tape's events, one JSON line each, to diff two runs
  shape   [--config FILE]                  every key `index` can write, for scripts/civitas-shape.py
  prompt  [--config FILE] [--dir DIR] [--price IN,OUT,READ,WRITE]
                                           what a person is told, what a day adds to a life, and what a tape was charged

A run needs `model.model` set in its configuration. Under the `anthropic`
backend it needs ANTHROPIC_API_KEY (or --api-key-file); under `openai` it needs
`model.base_url`, an OpenAI-compatible endpoint on this machine, and
`model.context_tokens` set to what that model can hold -- a day too long for it
is a silence, where a server left to itself would quietly truncate it.
--scripted calls no model: its tape exercises the apparatus and is not a run. --control gives every person the honest card; give it the
same world_seed as the run it is the control for. --days N ends the sitting
once N days have been lived, at the end of the day they fall in: where
`concurrency` is not zero, everybody scheduled for a day lives it together, so
a sitting ends at a day's end and may pass N by what one day holds."
    );
}

fn parse(args: &[String]) -> (Vec<String>, BTreeMap<String, String>) {
    let mut positional = Vec::new();
    let mut flags = BTreeMap::new();
    let mut i = 1;
    while i < args.len() {
        if let Some(name) = args[i].strip_prefix("--") {
            match args.get(i + 1) {
                Some(v) if !v.starts_with("--") => {
                    flags.insert(name.to_string(), v.clone());
                    i += 2;
                }
                _ => {
                    flags.insert(name.to_string(), "true".into());
                    i += 1;
                }
            }
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    (positional, flags)
}

fn dir_of(positional: &[String]) -> Result<PathBuf, String> {
    positional.first().map(PathBuf::from).ok_or("which run directory?".into())
}

fn price(flags: &BTreeMap<String, String>) -> Result<Option<edet_civitas::config::Price>, String> {
    flags
        .get("price")
        .map(|p| edet_civitas::config::Price::parse(p).map_err(|e| format!("--{e}")))
        .transpose()
}

fn cap(flags: &BTreeMap<String, String>) -> Result<Option<f64>, String> {
    flags
        .get("cap-dollars")
        .map(|c| c.parse::<f64>().map_err(|_| format!("--cap-dollars {c} is not an amount")))
        .transpose()
}

fn days(flags: &BTreeMap<String, String>) -> Result<Option<u64>, String> {
    flags
        .get("days")
        .map(|d| d.parse().map_err(|_| format!("--days {d} is not a number")))
        .transpose()
}

fn config() -> Result<String, String> {
    serde_json::to_string_pretty(&RunConfig::default()).map_err(|e| e.to_string())
}

fn logger() -> impl FnMut(&str) {
    |line: &str| eprintln!("{line}")
}

fn live_new(flags: &BTreeMap<String, String>) -> Result<String, String> {
    let mut cfg = match flags.get("config") {
        Some(path) => RunConfig::load(path)?,
        None => return Err("--config FILE: `edet-civitas config` prints one to start from".into()),
    };
    if flags.contains_key("control") {
        cfg.control = true;
    }
    if flags.contains_key("scripted") {
        cfg.model.backend = Backend::Scripted;
    }
    let dir = PathBuf::from(flags.get("dir").ok_or("--dir DIR: where the run's tape goes")?);
    // A price or a cap on the command line goes into the manifest with the
    // rest of the configuration: it is part of what this run is.
    if let Some(p) = price(flags)? {
        cfg.model.price = Some(p);
    }
    if let Some(c) = cap(flags)? {
        cfg.cap_dollars = c;
    }
    let opts = Options { api_key_file: flags.get("api-key-file").cloned(), days: days(flags)? };
    let mut model = run::model_for(&cfg, &opts)?;
    let mut runner = Runner::start(cfg, &dir, model.name())?;
    runner.live(model.as_mut(), &opts, &mut logger())?;
    Ok(summary(&runner))
}

fn resume(positional: &[String], flags: &BTreeMap<String, String>) -> Result<String, String> {
    let dir = dir_of(positional)?;
    let manifest = edet_civitas::tape::read_manifest(&dir)?;
    let mut cfg = manifest.config.clone();
    if let Some(m) = flags.get("model") {
        cfg.model.model = m.clone();
    }
    let opts = Options { api_key_file: flags.get("api-key-file").cloned(), days: days(flags)? };
    let mut model = run::model_for(&cfg, &opts)?;
    let mut runner = Runner::resume(&dir, &model.name(), flags.contains_key("allow-model-change"))?;
    runner.cfg.model.model = cfg.model.model;
    // A tape written before it had a price or a cap takes them for this
    // sitting: the bill is read off the whole tape either way.
    if let Some(p) = price(flags)? {
        runner.cfg.model.price = Some(p);
    }
    match cap(flags)? {
        Some(c) => runner.raise_cap(c)?,
        None if runner.world.ended.is_some() => {
            return Err(format!(
                "this run has ended: {}; pass --cap-dollars above what was spent to continue",
                runner.world.ended.clone().unwrap_or_default()
            ))
        }
        None => runner.cfg.check()?,
    }
    if let Some(spent) = runner.spent() {
        eprintln!(
            "this tape has cost ${spent:.2} so far{}",
            if runner.cfg.cap_dollars > 0.0 {
                format!(", of a cap of ${:.2}", runner.cfg.cap_dollars)
            } else {
                String::new()
            }
        );
    }
    runner.live(model.as_mut(), &opts, &mut logger())?;
    Ok(summary(&runner))
}

fn replay(positional: &[String], flags: &BTreeMap<String, String>) -> Result<String, String> {
    let dir = dir_of(positional)?;
    let until = flags
        .get("until")
        .map(|u| u.parse::<u64>().map_err(|_| format!("--until {u} is not a day")))
        .transpose()?;
    let (manifest, r) = run::replay(&dir, until)?;
    let w = &r.world;
    let mut out = format!(
        "replayed {} event(s) to day {} with every recorded outcome reproduced{}\n",
        r.events.len(),
        w.tick,
        if manifest.not_a_run { " (scripted: not a run)" } else { "" }
    );
    match &w.found {
        Some(f) => out.push_str(&format!("invariant failed: {} (act {}, transaction {})\n", f.invariant, f.act, f.tx)),
        None => out.push_str("no invariant failed\n"),
    }
    let (digest, root) = w.clone().close_tick();
    out.push_str(&format!("transition digest {digest}\nstate root {root}"));
    Ok(out)
}

/// The event layer as text: one event per line, in order, so two runs — a
/// run and its control, or one run before and after a resume — can be read
/// side by side with `diff`, the way a corpus re-pin is.
fn events(dir: &std::path::Path) -> Result<String, String> {
    let lines: Result<Vec<String>, String> = edet_civitas::tape::read_events(dir)?
        .iter()
        .map(|e| serde_json::to_string(e).map_err(|x| x.to_string()))
        .collect();
    Ok(lines?.join("\n"))
}

fn shape(flags: &BTreeMap<String, String>) -> Result<String, String> {
    let cfg = match flags.get("config") {
        Some(path) => RunConfig::load(path)?,
        None => RunConfig::default(),
    };
    edet_civitas::index::shape(&cfg)
}

/// What a person carries on every call, and what a day adds to that. Sizes are
/// characters and an approximation of tokens at four characters each: the
/// figure to compare against another block, never a bill. With a `--dir`, the
/// tape's own days are measured, which is the quantity a long run's cost
/// follows.
fn prompt_sizes(flags: &BTreeMap<String, String>) -> Result<String, String> {
    use edet_civitas::config::Tier;
    use edet_civitas::prompt::{self, Knowledge};
    use edet_civitas::tools;

    let cfg = match flags.get("config") {
        Some(path) => RunConfig::load(path)?,
        None => RunConfig::default(),
    };
    let k = Knowledge::load(&cfg)?;
    let tok = |n: usize| n.div_ceil(4);
    let len = |v: &serde_json::Value| serde_json::to_string(v).unwrap_or_default().len();
    let mut out = String::from("what a person carries on every call (chars, ~tokens at 4 chars each)\n");
    let tools_len: usize = tools::definitions().iter().map(len).sum();
    for tier in [Tier::Paper, Tier::Readme, Tier::Wallet] {
        let blocks = prompt::system(&k, "", tier, cfg.trust.wary);
        let system: usize = blocks.iter().map(len).sum();
        out.push_str(&format!(
            "  {:<8} system {:>8} ({:>6}) + tools {:>7} ({:>5}) = {:>8} ({:>6})\n",
            format!("{tier:?}").to_lowercase(),
            system,
            tok(system),
            tools_len,
            tok(tools_len),
            system + tools_len,
            tok(system + tools_len),
        ));
    }
    out.push_str(&format!(
        "  of which the wallet's copy {} ({}), and its {} refusal sentences {} ({}) are NOT carried\n",
        k.wallet.len(),
        tok(k.wallet.len()),
        k.errors.len(),
        k.errors.values().map(String::len).sum::<usize>(),
        tok(k.errors.values().map(String::len).sum::<usize>()),
    ));
    let Some(dir) = flags.get("dir") else {
        return Ok(out);
    };
    let (manifest, replayed) = run::replay(PathBuf::from(dir).as_path(), None)?;
    let (mut note, mut said, mut asked, mut answered, mut days) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for life in &replayed.lives {
        for m in life {
            let user = m["role"] == "user";
            let blocks = m["content"].as_array().cloned().unwrap_or_default();
            for b in &blocks {
                let n = len(b);
                match (b["type"].as_str().unwrap_or(""), user) {
                    ("text", true) => {
                        note += n;
                        days += usize::from(b["text"].as_str().is_some_and(|t| t.starts_with("Day ")));
                    }
                    ("text", false) => said += n,
                    ("tool_use", _) => asked += n,
                    ("tool_result", _) => answered += n,
                    _ => {}
                }
            }
        }
    }
    let total = note + said + asked + answered;
    let per = |n: usize| n.checked_div(days).map(tok).unwrap_or(0);
    out.push_str(&format!("\nwhat a day adds to a life, over {days} days lived in {dir}\n"));
    for (name, n) in [
        ("the day's note", note),
        ("what they said", said),
        ("what they asked", asked),
        ("what they were told", answered),
    ] {
        out.push_str(&format!(
            "  {name:<22} {:>9} ({:>6})  {:>5} tokens a day  {:>5.1}%\n",
            n,
            tok(n),
            per(n),
            if total == 0 { 0.0 } else { 100.0 * n as f64 / total as f64 }
        ));
    }
    out.push_str(&format!("  {:<22} {:>9} ({:>6})  {:>5} tokens a day\n", "altogether", total, tok(total), per(total)));

    // What the endpoint counted, which is the only figure here that is a
    // measurement: everything above is four characters a token, and a tape
    // written by the scripted backend has no counts at all.
    use edet_civitas::event::{Event, Usage};
    let mut u = Usage::default();
    // Counted over the days a person SPENT, which is not the days a person
    // KEPT: a day whose call failed partway had already paid for the calls
    // before it, and it leaves no life behind to be measured above. Divide
    // tokens by days that left a life and every figure reads high.
    let mut turns = 0u64;
    let mut ticks = 0u64;
    for e in &replayed.events {
        match e {
            Event::Day { usage, .. } | Event::Silent { usage, .. } => {
                u.add(usage);
                turns += 1;
            }
            Event::TickClosed { .. } => ticks += 1,
            _ => {}
        }
    }
    // A scripted tape counts its calls and no tokens: there is nothing to
    // report and nothing to price. Nothing else can carry a count, so past
    // this line a day was spent and `turns` is not zero.
    let counted = u.input_tokens + u.output_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
    if counted == 0 || turns == 0 {
        return Ok(out);
    }
    let each = |n: u64| n / turns;
    out.push_str(&format!("\nwhat the endpoint counted, over {} call(s) in {turns} day(s) somebody spent\n", u.calls));
    for (name, n) in [
        ("input, fresh", u.input_tokens),
        ("input, from the cache", u.cache_read_input_tokens),
        ("input, written to it", u.cache_creation_input_tokens),
        ("output", u.output_tokens),
    ] {
        out.push_str(&format!("  {name:<22} {n:>12}  {:>8} a person-day\n", each(n)));
    }
    // The command line's rates, or the manifest's own.
    let Some(price) = price(flags)?.or(manifest.config.model.price) else {
        out.push_str("  --price IN,OUT,READ,WRITE (dollars a million) prices it\n");
        return Ok(out);
    };
    let bill = price.cost(&u);
    out.push_str(&format!("  {:<22} {:>12}\n", "a bill of", format!("${bill:.2}")));
    out.push_str(&format!(
        "  {:<22} {:>12}  over {} person(s)\n",
        "a person-day cost",
        format!("${:.4}", bill / turns as f64),
        replayed.lives.len()
    ));
    if ticks > 0 {
        out.push_str(&format!(
            "  {:<22} {:>12}  over {ticks} day(s) of the community\n",
            "a day of it cost",
            format!("${:.4}", bill / ticks as f64)
        ));
    }
    Ok(out)
}

/// The run's last line. The members are counted by how they came to hold an
/// account — present on the first day, seated from the town, or a stranger
/// an offer minted — because a member count alone said "42 members" of a
/// pilot in which no townsperson without an account was ever seated.
fn summary(r: &Runner) -> String {
    let w = &r.world;
    let members: Vec<&edet_civitas::world::Person> = w.persons.iter().filter(|p| p.member.is_some()).collect();
    let at_genesis = members.iter().filter(|p| p.joined_tick == 0).count();
    let minted = members.iter().filter(|p| p.introduced_by.is_some()).count();
    let from_town = members.len() - at_genesis - minted;
    format!(
        "{}: day {}, {} of {} days used, {} people, {} members ({at_genesis} from the first day, {from_town} seated from the town, {minted} strangers minted){}",
        r.dir.display(),
        w.tick,
        w.turns_used,
        r.cfg.turn_budget,
        w.persons.len(),
        members.len(),
        w.ended.as_ref().map(|e| format!(", ended: {e}")).unwrap_or_default()
    )
}
