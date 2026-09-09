//! The pinned corpus: one entry per scene, each a `Run` and the `Summary` it
//! must produce.
//!
//! **A pin is a claim about behaviour, and a diff in it is a change of
//! behaviour.** `Summary` carries no `f64`, every map is a `BTreeMap` and
//! every ratio is a pair, so the JSON is byte-stable and a review reads a
//! moved field rather than a rounding. `just swarm-pin` rewrites the file;
//! what it writes has to be READ, because a `refused` count that went to zero
//! is a rule that stopped firing.

use crate::metrics::Summary;
use crate::run::Run;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub name: String,
    pub run: Run,
    pub expect: Summary,
}

/// Where the corpus lives, resolved against this crate rather than against
/// the working directory — a test binary's cwd is the crate root today and
/// nothing promises it stays that way.
pub fn path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus.json")
}

pub fn load() -> Result<Vec<Entry>, String> {
    let raw = std::fs::read_to_string(path()).map_err(|e| format!("{}: {e}", path().display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path().display()))
}

pub fn save(entries: &[Entry]) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    std::fs::write(path(), raw + "\n").map_err(|e| format!("{}: {e}", path().display()))
}

/// One scene: a preset, the seed it runs on, how long for, and how many seats
/// if the preset's own size is not the one being pinned.
#[derive(Clone, Debug)]
pub struct Scene {
    pub name: String,
    pub seed: u64,
    pub ticks: u64,
    pub members: Option<usize>,
}

/// The scenes the corpus covers.
///
/// Every preset the crate names appears, so an archetype that stopped
/// reaching its probe shows up as a moved pin rather than as nothing at all.
/// The sleeper family runs at four values of `E` because the claim it tests —
/// bounded in AMOUNT, not in time — is false for exactly one of them if a
/// timer ever reaches `committed`.
///
/// **Each preset is run at the size its probe needs and no larger.** A corpus
/// is a regression gate: what it has to detect is a rule that stopped firing,
/// and a rule fires or does not at ten seats as much as at two hundred. Size
/// belongs to the SEARCH, which is out of band and takes `--members`.
///
/// Measured, which is why `everything` is not grown here: at 60 seats over 200
/// ticks the sybil farm inside it seats 874 rows, and the corpus goes from
/// 26 s to **3m05s** in debug — the audit is `O(E)` per transition and every
/// one of those rows is in `E`. What that buys is a scene the search already
/// covers, at a price that turns the gate into something people skip. `members`
/// stays available for a scene whose probe needs a size its preset does not
/// seat.
pub fn scenes() -> Vec<Scene> {
    let s = |name: &str, seed: u64, ticks: u64| Scene { name: name.into(), seed, ticks, members: None };
    let mut v: Vec<Scene> = vec![
        s("honest", 1, 120),
        s("deadbeats", 2, 120),
        s("wash-ring", 3, 120),
        // **Three measurements of one claim, and each is a different way it
        // could be false.** The rows a farm seats are bounded by the cut behind
        // its set: `-x10` is the same scene at ten times the backing, where a
        // rule that merely re-scaled would look identical at one size; `-long`
        // is the same scene over 300 ticks, where a rule that bounded a rate
        // rather than a stock would look identical over five.
        s("sybil-farm", 4, 5),
        s("sybil-farm-x10", 14, 8),
        s("sybil-farm-long", 15, 300),
        // **What the ceiling costs an ordinary community.** `-open` greets at a
        // tenth of its ticks and must never meet the ceiling; `-full` greets on
        // every one and must meet it exactly where the seed runs out.
        s("honest-open", 16, 120),
        s("honest-full", 17, 120),
        s("griefer", 5, 60),
        s("coalition-third", 6, 30),
        s("coalition-half", 7, 30),
        s("coalition-two-thirds", 8, 30),
        s("exit-under-suspension", 9, 30),
        s("late-defaulter", 10, 150),
        s("hoarders", 11, 120),
        s("everything", 12, 200),
    ];
    for (i, e) in [30u64, 90, 180, 365].iter().enumerate() {
        v.push(s(&format!("sleeper-debtor-{e}"), 20 + i as u64, e + 60));
        v.push(s(&format!("sleeper-underwriter-{e}"), 30 + i as u64, e + 60));
    }
    v
}

/// Regenerate every entry from what the tree does now.
pub fn regenerate() -> Result<Vec<Entry>, String> {
    scenes()
        .into_iter()
        .map(|scene| {
            let mut run = Run::corpus(scene.seed, &scene.name, scene.ticks);
            run.members = scene.members;
            let report = crate::run::run(&run)?;
            Ok(Entry { name: scene.name, run, expect: report.summary })
        })
        .collect()
}
