//! **The violation report and the compile queue, read off a tape.**
//!
//! A violation is a fact about the ledger's code, found by whatever the people
//! happened to do, and it is reported in full: the day, the person and their
//! card, the act, the transaction, its signers, the invariant, and the command
//! that replays the tape to that day with no model call — the only way it can
//! be reproduced, since nobody in the run can be reseeded.
//!
//! The queue is what `scripts/persona.py` hands a hand: entries with their
//! marks and an empty `compiles to:` line, one per diary entry and one per
//! first refusal of a code by a person. No counts, no percentages, no ranking:
//! what a person wrote or tried is what the generator produced, and a hand
//! decides whether it compiles to an archetype, a corpus scene, or a probe
//! that shows it does not bite.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use crate::acts::{Act, ActResult};
use crate::event::{Event, PersonRecord};
use crate::tape;
use crate::world::describe_act;

struct People(BTreeMap<usize, (String, String)>);

impl People {
    fn from(events: &[Event]) -> People {
        let mut m = BTreeMap::new();
        let mut add = |p: &PersonRecord| {
            m.insert(p.index, (p.card_name.clone(), p.card.clone().unwrap_or_default()));
        };
        for e in events {
            if let Event::Genesis { persons, .. } = e {
                persons.iter().for_each(&mut add);
            }
        }
        for e in events {
            if let Event::CardWritten { person, name, card, .. } = e {
                m.insert(*person, (name.clone(), card.clone()));
            }
        }
        People(m)
    }

    fn name(&self, i: usize) -> String {
        self.0
            .get(&i)
            .map(|(n, _)| n.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "a newcomer".into())
    }
}

pub fn violations(dir: &Path) -> Result<String, String> {
    let manifest = tape::read_manifest(dir)?;
    let events = tape::read_events(dir)?;
    let people = People::from(&events);
    let mut out = String::new();
    let _ = writeln!(out, "# Violations\n");
    if manifest.not_a_run {
        let _ = writeln!(out, "**This tape was written by the scripted backend. It is not a run.**\n");
    }
    let mut any = false;
    for e in &events {
        if let Event::Violation { tick, person, act, tx, signers, invariant } = e {
            any = true;
            let who = person
                .map(|p| format!("person {p} ({})", people.name(p)))
                .unwrap_or_else(|| "the epoch sweep".into());
            let _ = writeln!(out, "## Day {tick}: {invariant}\n");
            let _ = writeln!(out, "- by: {who}");
            let _ = writeln!(out, "- act: `{act}`");
            let _ = writeln!(out, "- transaction: `{tx}`");
            let _ = writeln!(out, "- signers: {signers:?}");
            let _ = writeln!(out, "- replay: `edet-civitas replay {} --until {tick}`\n", dir.display());
        }
    }
    if !any {
        let _ = writeln!(out, "No invariant failed on this tape.");
    }
    Ok(out)
}

pub fn queue(dir: &Path) -> Result<String, String> {
    let manifest = tape::read_manifest(dir)?;
    let events = tape::read_events(dir)?;
    let people = People::from(&events);
    let mut out = String::new();
    let _ = writeln!(out, "# Compile queue\n");
    let _ = writeln!(
        out,
        "Every entry is what one generator produced. None is a measurement. `compiles to:` is for the hand that turns \
         an entry into an `Archetype`, a corpus scene, or a probe showing it does not bite.\n"
    );
    if manifest.not_a_run {
        let _ = writeln!(out, "**This tape was written by the scripted backend. It is not a run.**\n");
    }
    let mut refused_seen: BTreeSet<(usize, String)> = BTreeSet::new();
    for e in &events {
        let Event::Day { tick, person, acts, results, refused, .. } = e else { continue };
        let marks = |extra: &str| {
            let mut m = vec![format!("person {person}"), people.name(*person), format!("day {tick}")];
            if !extra.is_empty() {
                m.push(extra.to_string());
            }
            m.join(" · ")
        };
        let others: Vec<String> = acts
            .iter()
            .zip(results)
            .filter(|(a, _)| !matches!(a, Act::Diary { .. }))
            .map(|(a, r)| format!("{} → {}", describe_act(a), r.describe()))
            .collect();
        for a in acts {
            if let Act::Diary { text } = a {
                let _ = writeln!(out, "## Diary\n");
                let _ = writeln!(out, "marks: {}\n", marks(""));
                for line in text.lines() {
                    let _ = writeln!(out, "> {line}");
                }
                if !others.is_empty() {
                    let _ = writeln!(out, "\nthe same day: {}", others.join("; "));
                }
                let _ = writeln!(out, "\ncompiles to:\n");
            }
        }
        let committed = acts.iter().zip(results.iter());
        for (a, r) in refused.iter().map(|(a, r)| (a, r)).chain(committed) {
            let code = match r {
                ActResult::Refused { code } => code.clone(),
                _ => continue,
            };
            if refused_seen.insert((*person, code.clone())) {
                let _ = writeln!(out, "## Refused: {code}\n");
                let _ = writeln!(out, "marks: {}\n", marks("first time this person met this refusal"));
                let _ = writeln!(out, "tried: {}\n", describe_act(a));
                let _ = writeln!(out, "compiles to:\n");
            }
        }
    }
    Ok(out)
}
