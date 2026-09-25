//! **What the player reads: a tape replayed into one snapshot per day.**
//!
//! The player never replays the ledger; it scrubs through these. Each day's
//! snapshot is the ledger as it stood when the day closed — every member with
//! their capacity, debt, supply and default, every stake, every contract —
//! beside every household's money and the published figures, and the day's
//! events with what each came to. It is the whole view, not a member's: the
//! player is for watching the community, not for being in it.
//!
//! Replaying to build it also checks the tape, because every event must come
//! to what the tape says it came to.
//!
//! ```text
//! <run>/player/run.json       the run: manifest essentials, everybody, the days
//! <run>/player/days.jsonl     one snapshot per closed day
//! <run>/player/lives/<i>.json person i's whole conversation, day by day
//! ```

use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};

use edet_state::state::State;

use crate::acts::Act;
use crate::event::Event;
use crate::run::replay_each;
use crate::tape;
use crate::world::{describe_act, World};

/// The rows a day's sweep marked expired, read at the opening against the
/// previous close. A snapshot is the day at its CLOSE, and a default the
/// sweep marks in the morning and a cure pays off by evening never appears in
/// one: pilot-6's underwriters took over seventeen insured defaults that way,
/// every one invisible to a reader of the closes, while the player's
/// milestone said the first default came on day 68.
fn expired_ids(w: &World) -> std::collections::BTreeSet<u64> {
    w.st()
        .contracts
        .values()
        .filter(|c| c.status == edet_state::types::ContractStatus::Expired)
        .map(|c| c.id)
        .collect()
}

fn snapshot(w: &World, tick: u64, events: &[Value], expired_today: &[u64], draws: &[Value]) -> Value {
    let st = w.st();
    let members: Vec<Value> = st
        .members
        .values()
        .map(|m| {
            json!({
                "id": m.id,
                "person": w.person_of_member(m.id),
                "status": edet_view::disclose::status_str(m.status),
                "capacity": State::to_minor(st.capacity_of(m.id)),
                "debt": m.debt_out,
                "supply": st.underwriters.get(&m.id).copied().unwrap_or(0),
                "open_default": m.rep.open_default,
            })
        })
        .collect();
    let edges: Vec<Value> = st.edges.iter().map(|((c, d), w)| json!([c, d, w])).collect();
    let contracts: Vec<Value> = st
        .contracts
        .values()
        .map(|c| {
            json!({
                "id": c.id,
                "debtor": c.debtor,
                "creditor": c.creditor,
                "original": c.original,
                "outstanding": c.outstanding,
                "maturity_epoch": c.maturity_epoch,
                // A row a handover or a cascade reopened inherits its
                // acceptance and restarts its creation: the two differ on a
                // successor and agree on a fresh acceptance, which is how a
                // reader tells new credit from old credit in a new row.
                "created_epoch": c.created_epoch,
                "accepted_epoch": c.accepted_epoch,
                "status": edet_view::disclose::contract_status_str(c.status),
                "insured": c.insured,
            })
        })
        .collect();
    let purses: Vec<Value> = w
        .economy
        .purses
        .iter()
        .enumerate()
        .map(|(i, p)| json!({ "person": i, "cash": p.cash, "arrears": p.arrears, "income_cut": p.cut_until.is_some() }))
        .collect();
    json!({
        "tick": tick,
        "epoch": st.epoch,
        "members": members,
        "edges": edges,
        "contracts": contracts,
        "purses": purses,
        "economy": { "index_ppm": w.economy.index_ppm, "regime": w.economy.regime, "published": w.economy.indicator() },
        "seed": st.underwriters.values().sum::<u64>(),
        "seat_committed": edet_kernel::flow::committed_total(&st.seat_committed),
        "pending_offers": w.pool.live_count(st.epoch),
        "expired_today": expired_today,
        "draws": draws,
        "square": w.square.iter().filter(|s| s.tick == tick).collect::<Vec<_>>(),
        "mail": w.mail.iter().filter(|m| m.tick == tick).collect::<Vec<_>>(),
        "events": events,
    })
}

fn act_view(a: &Act) -> Value {
    let mut v = serde_json::to_value(a).unwrap_or(Value::Null);
    v["describe"] = json!(describe_act(a));
    v["parties"] = json!(parties_of(a));
    v
}

/// **Who an act names, as the ledger names them.** An outcome says a
/// reference and never a person — "offer sent, waiting for the others" is true
/// of every offer ever sent — so the row carries the people its act is about,
/// and what to call them is the reader's business, not the ledger's.
fn parties_of(a: &Act) -> Vec<Value> {
    use crate::acts::{Ask, Side};
    let side = |s: &Side| match s {
        Side::Member(id) => json!({ "member": id }),
        // `n` is which of this owner's newcomers it is, so a reader can find
        // the person the key became.
        Side::Newcomer { owner, n } => json!({ "newcomer": true, "owner": owner, "n": n }),
        // A person of the town named by address, seated by this trade.
        Side::Person(i) => json!({ "person": i }),
    };
    let of_ask = |ask: &Ask| match ask {
        Ask::Lend { creditor, debtor, .. } => vec![side(creditor), side(debtor)],
        Ask::Sell { seller, buyer, .. } => vec![side(seller), side(buyer)],
        Ask::Transfer { new_debtor, .. } => vec![json!({ "member": new_debtor })],
        Ask::ApproveSupporter { supporter, .. } => vec![json!({ "member": supporter })],
        _ => Vec::new(),
    };
    match a {
        Act::Offer { ask, .. } | Act::Solo { ask } => of_ask(ask),
        Act::PayCash { to, .. } | Act::Message { to, .. } => vec![json!({ "person": to })],
        _ => Vec::new(),
    }
}

/// How one event appears in a day's list, or `None` for the events that are
/// the day's own frame.
fn event_view(e: &Event) -> Option<Value> {
    let with = |a: &Act, r: &crate::acts::ActResult| json!({ "act": act_view(a), "result": r, "says": r.describe() });
    Some(match e {
        Event::TickOpened { schedule, .. } => json!({ "event": "opened", "schedule": schedule }),
        Event::CardWritten { person, name, card, tier, .. } => {
            json!({ "event": "card", "person": person, "name": name, "card": card, "tier": tier })
        }
        Event::Day { person, acts, results, refused, usage, exchange, .. } => json!({
            "event": "day",
            "person": person,
            "exchange": exchange,
            "acts": acts.iter().zip(results).map(|(a, r)| with(a, r)).collect::<Vec<_>>(),
            "refused": refused.iter().map(|(a, r)| with(a, r)).collect::<Vec<_>>(),
            "usage": usage,
        }),
        Event::Silent { person, reason, .. } => json!({ "event": "silent", "person": person, "reason": reason }),
        Event::Instructions { fired, results, .. } => json!({
            "event": "instructions",
            "fired": fired.iter().zip(results).map(|((p, a), r)| {
                let mut v = with(a, r);
                v["person"] = json!(p);
                v
            }).collect::<Vec<_>>(),
        }),
        Event::Violation { person, act, tx, signers, invariant, .. } => json!({
            "event": "violation", "person": person, "act": act, "tx": tx, "signers": signers, "invariant": invariant,
        }),
        Event::ModelChanged { from, to, .. } => json!({ "event": "model_changed", "from": from, "to": to }),
        Event::Ended { reason, .. } => json!({ "event": "ended", "reason": reason }),
        Event::Reopened { cap_dollars, spent, .. } => {
            json!({ "event": "reopened", "cap_dollars": cap_dollars, "spent": spent })
        }
        Event::TickClosed { .. } | Event::Genesis { .. } => return None,
    })
}

/// **Adoption is what a member did with a trade the note brought them
/// against somebody with no account**, never the member count. Pilot-6 read
/// "42 members": eight from the first day and thirty-four strangers that
/// `new` had minted, while 655 trade lines named one of the town's own
/// account-less people and not one of them was ever seated. A line is counted
/// on the first day a member lives in a tick, from the draws their note
/// showed; what became of it is read off the same tick's acts.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Adoption {
    /// Trade lines a member's note carried naming a person with no account.
    pub outsider_lines: u64,
    /// Of those, offered to that person on the ledger the same day.
    pub offered_to_them: u64,
    /// Of those, paid in cash the same day, by either side.
    pub settled_in_cash: u64,
    pub unanswered: u64,
    /// Offers that minted a stranger instead of naming anybody the town had.
    pub strangers_minted: u64,
    pub seated_from_the_town: u64,
    pub seated_minted: u64,
}

impl Adoption {
    pub fn line(&self) -> String {
        format!(
            "adoption: {} trade line(s) named somebody with no account, {} offered to them on the ledger, {} settled in cash, {} unanswered; {} stranger(s) minted; seated {} from the town and {} minted",
            self.outsider_lines,
            self.offered_to_them,
            self.settled_in_cash,
            self.unanswered,
            self.strangers_minted,
            self.seated_from_the_town,
            self.seated_minted
        )
    }
}

/// What a tick's opening dealt everybody: the draws their notes will carry.
fn draws_at(w: &World) -> Vec<Value> {
    w.persons
        .iter()
        .filter(|p| p.money.buy_from.is_some() || p.money.sell_to.is_some())
        .map(|p| json!({ "person": p.index, "buy_from": p.money.buy_from, "sell_to": p.money.sell_to }))
        .collect()
}

/// Somebody to trade with and what it comes to, or nobody today.
type Draw = Option<(usize, u64)>;

/// The tick's part of the adoption count, folded into the total at its close.
#[derive(Default)]
struct TickLines {
    money: Vec<(Draw, Draw)>,
    member: Vec<bool>,
    lived: std::collections::BTreeSet<usize>,
    lines: Vec<(usize, usize)>,
    offered: std::collections::BTreeSet<(usize, usize)>,
    cash: std::collections::BTreeSet<(usize, usize)>,
}

impl TickLines {
    fn open(&mut self, w: &World) {
        self.money = w.persons.iter().map(|p| (p.money.buy_from, p.money.sell_to)).collect();
        self.member = w.persons.iter().map(|p| p.member.is_some()).collect();
        self.lived.clear();
        self.lines.clear();
        self.offered.clear();
        self.cash.clear();
    }

    fn is_member(&self, i: usize) -> bool {
        self.member.get(i).copied().unwrap_or(false)
    }

    fn day(&mut self, w: &World, person: usize, acts: &[Act], results: &[crate::acts::ActResult], a: &mut Adoption) {
        use crate::acts::ActResult;
        if self.lived.insert(person) && self.is_member(person) {
            if let Some((buy, sell)) = self.money.get(person).copied() {
                for (j, _) in buy.into_iter().chain(sell) {
                    if !self.is_member(j) {
                        self.lines.push((person, j));
                    }
                }
            }
        }
        for (act, r) in acts.iter().zip(results) {
            match (act, r) {
                (Act::Offer { ask, .. }, ActResult::Opened { .. } | ActResult::Applied { .. }) => {
                    if let Some(j) = ask.person() {
                        self.offered.insert((person, j));
                    }
                    if ask.newcomer().is_some() {
                        a.strangers_minted += 1;
                    }
                }
                (Act::PayCash { to, .. }, ActResult::Paid) => {
                    self.cash.insert((person.min(*to), person.max(*to)));
                }
                _ => {}
            }
            Self::seated(w, r, a);
        }
    }

    fn seated(w: &World, r: &crate::acts::ActResult, a: &mut Adoption) {
        if let crate::acts::ActResult::Applied { seated, .. } = r {
            for m in seated {
                match w.person_of_member(*m).and_then(|p| w.persons.get(p)) {
                    Some(p) if p.introduced_by.is_some() => a.seated_minted += 1,
                    Some(_) => a.seated_from_the_town += 1,
                    None => {}
                }
            }
        }
    }

    fn close(&mut self, a: &mut Adoption) {
        for &(i, j) in &self.lines {
            a.outsider_lines += 1;
            if self.offered.contains(&(i, j)) {
                a.offered_to_them += 1;
            } else if self.cash.contains(&(i.min(j), i.max(j))) {
                a.settled_in_cash += 1;
            } else {
                a.unanswered += 1;
            }
        }
        self.lines.clear();
    }
}

fn run_view(manifest: &tape::Manifest, w: &World, ticks: &[Value], adoption: &Adoption) -> Value {
    let persons: Vec<Value> = w
        .persons
        .iter()
        .map(|p| {
            let mut v = serde_json::to_value(w.record_of(p)).unwrap_or(Value::Null);
            v["retired"] = json!(p.retired);
            v
        })
        .collect();
    json!({
        "not_a_run": manifest.not_a_run,
        "backend": manifest.backend,
        "model": manifest.model,
        "world_seed": manifest.config.world_seed,
        "control": manifest.config.control,
        "turn_budget": manifest.config.turn_budget,
        "turns_used": w.turns_used,
        "ended": w.ended,
        "genesis_epoch": w.genesis_epoch,
        "persons": persons,
        "ticks": ticks,
        "adoption": adoption,
    })
}

fn life_entry(x: &tape::Exchange) -> Value {
    json!({ "tick": x.tick, "what": x.what, "exchange": x.id, "messages": x.messages })
}

pub fn build(dir: &Path) -> Result<String, String> {
    let out_dir = dir.join("player");
    std::fs::create_dir_all(out_dir.join("lives")).map_err(|e| e.to_string())?;
    let days_path = out_dir.join("days.jsonl");
    let mut days = std::io::BufWriter::new(std::fs::File::create(&days_path).map_err(|e| e.to_string())?);
    let mut today: Vec<Value> = Vec::new();
    let mut ticks: Vec<Value> = Vec::new();
    let mut failure: Option<String> = None;
    // The last closed day is held back until another day closes, so events
    // that arrive after it without opening a new day — a run ending on its
    // budget — join that day rather than becoming a second frame of it.
    let mut held: Option<Value> = None;
    let mut opened_since = false;
    // What the sweep expired since the last close, read at each opening.
    let mut expired_at_close = std::collections::BTreeSet::new();
    let mut expired_today: Vec<u64> = Vec::new();
    // What the opening dealt, and what members did with the lines that named
    // somebody with no account.
    let mut draws_today: Vec<Value> = Vec::new();
    let mut adoption = Adoption::default();
    let mut lines = TickLines::default();
    let write = |days: &mut std::io::BufWriter<std::fs::File>, snap: &Value| {
        serde_json::to_writer(&mut *days, snap)
            .map_err(|e| e.to_string())
            .and_then(|_| days.write_all(b"\n").map_err(|e| e.to_string()))
    };
    let mut each = |w: &World, e: &Event| {
        match e {
            Event::Day { person, acts, results, .. } => lines.day(w, *person, acts, results, &mut adoption),
            Event::Instructions { results, .. } => results.iter().for_each(|r| TickLines::seated(w, r, &mut adoption)),
            _ => {}
        }
        if let Event::TickClosed { tick, epoch, digest, .. } = e {
            if let Some(prev) = held.take() {
                if let Err(err) = write(&mut days, &prev) {
                    failure.get_or_insert(err);
                }
            }
            lines.close(&mut adoption);
            held = Some(snapshot(
                w,
                *tick,
                &std::mem::take(&mut today),
                &std::mem::take(&mut expired_today),
                &std::mem::take(&mut draws_today),
            ));
            expired_at_close = expired_ids(w);
            opened_since = false;
            ticks.push(json!({ "tick": tick, "epoch": epoch, "digest": digest }));
        } else if let Some(v) = event_view(e) {
            if matches!(e, Event::TickOpened { .. }) {
                expired_today = expired_ids(w).difference(&expired_at_close).copied().collect();
                draws_today = draws_at(w);
                lines.open(w);
                // What happened between the last close and this opening — a
                // resume's `ModelChanged` — belongs to the day it followed.
                if let (Some(last), false) = (held.as_mut(), opened_since) {
                    if let Some(events) = last["events"].as_array_mut() {
                        events.extend(std::mem::take(&mut today));
                    }
                }
                opened_since = true;
            }
            today.push(v);
        }
    };
    let (manifest, r) = replay_each(dir, None, &mut each)?;
    if let Some(f) = failure {
        return Err(f);
    }
    // **A day that never closed is still a day.** A run ends on a violated
    // invariant without closing the day it failed on: that day is written, and
    // marked, or the violation would vanish from the one thing people watch.
    // Events after the last close that opened no day — a run ending on its
    // budget — are the last closed day's.
    match (held.take(), today.is_empty(), opened_since) {
        (Some(mut last), false, false) => {
            if let Some(events) = last["events"].as_array_mut() {
                events.extend(std::mem::take(&mut today));
            }
            write(&mut days, &last)?;
        }
        (held, _, _) => {
            if let Some(last) = held {
                write(&mut days, &last)?;
            }
            if !today.is_empty() {
                lines.close(&mut adoption);
                let mut snap = snapshot(&r.world, r.world.tick, &today, &expired_today, &draws_today);
                snap["unclosed"] = json!(true);
                write(&mut days, &snap)?;
                ticks.push(json!({ "tick": r.world.tick, "epoch": r.world.st().epoch, "digest": Value::Null }));
            }
        }
    }
    days.flush().map_err(|e| e.to_string())?;
    drop(days);

    std::fs::write(
        out_dir.join("run.json"),
        serde_json::to_vec(&run_view(&manifest, &r.world, &ticks, &adoption)).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let exchanges = tape::read_exchanges(dir)?;
    let named: std::collections::BTreeSet<u64> = r
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Day { exchange, .. } => Some(*exchange),
            Event::CardWritten { exchange, .. } => *exchange,
            _ => None,
        })
        .collect();
    let mut lives: Vec<Vec<Value>> = vec![Vec::new(); r.world.persons.len()];
    for x in exchanges.iter().filter(|x| named.contains(&x.id)) {
        if let Some(life) = lives.get_mut(x.person) {
            life.push(life_entry(x));
        }
    }
    for (i, life) in lives.iter().enumerate() {
        std::fs::write(
            out_dir.join("lives").join(format!("{i}.json")),
            serde_json::to_vec(life).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(format!(
        "{} day(s), {} person(s) indexed into {}\n{}",
        ticks.len(),
        r.world.persons.len(),
        out_dir.display(),
        adoption.line()
    ))
}

/// **Every key `index` can write**, for `scripts/civitas-shape.py`: a world at
/// genesis with a word said on the square and in private, one event of every
/// kind with an act of every result, a run view and a life entry — built from
/// the functions `build` uses, so the answer is what the index writes and not a
/// list somebody keeps.
pub fn shape(cfg: &crate::config::RunConfig) -> Result<String, String> {
    use crate::acts::{ActResult, Ask, Instruction, PaidWith, Side};
    use crate::event::Usage;
    let cards = crate::config::load_cards(cfg)?;
    let genesis = crate::world::genesis(cfg, &cards);
    let mut w = World::found(cfg, &genesis)?;
    // A contract and a stake, earned the only way one is: a founder lends, the
    // debtor settles.
    let (creditor, debtor) = (w.persons[0].key, w.persons[1].key);
    let lend = edet_state::tx::Tx::Accept {
        debtor: edet_state::types::Party::Member(1),
        creditor: edet_state::types::Party::Member(0),
        amount: 10.0,
        maturity_epochs: w.st().params.min_maturity_epochs,
        arb: None,
    };
    w.d.0
        .apply(lend, &[creditor, debtor])
        .map_err(|e| format!("shape: the lend was refused: {}", e.0))?;
    let settle = edet_state::tx::Tx::Settle { contract: 0, amount: 10.0 };
    w.d.0
        .apply(settle, &[creditor, debtor])
        .map_err(|e| format!("shape: the settlement was refused: {}", e.0))?;
    w.square
        .push(crate::world::Said { seq: 1, tick: 0, person: 0, text: String::new() });
    w.mail
        .push(crate::world::Mail { seq: 2, tick: 0, from: 0, to: 1, text: String::new() });
    let acts = [
        Act::Offer {
            ask: Ask::Lend {
                creditor: Side::Member(0),
                debtor: Side::Newcomer { owner: 0, n: 0 },
                amount_minor: 1,
                term: 30,
            },
            required: Vec::new(),
            nonce: String::new(),
            not_after_epoch: 0,
            newcomer: Some(0),
            paid_with: Some(PaidWith::Cash),
        },
        Act::Solo { ask: Ask::Exit },
        Act::Sign { digest: String::new() },
        Act::Decline { digest: String::new() },
        Act::PayCash { to: 1, amount_minor: 1, note: String::new() },
        Act::Post { text: String::new() },
        Act::Message { to: 1, text: String::new() },
        Act::Diary { text: String::new() },
        Act::SetInstruction { instruction: Instruction::AcceptFrom { member: 1, max_amount_minor: 1 } },
        Act::RevokeInstruction { id: 1 },
    ];
    // Every ask a member can make, not only the lend and the exit the acts
    // above happen to carry. An ask's own fields are written into a day's acts
    // — a sale's `seller` and `buyer`, a proposal's `key` — so a variant this
    // sample never takes is a field the player may read and this answer never
    // names, which is the one thing `civitas-shape.py` exists to catch. Each
    // is paired with a result below, because `zip` drops whatever the shorter
    // side does not reach.
    let asks = [
        Ask::Sell { seller: Side::Member(0), buyer: Side::Member(1), amount_minor: 1, term: 30 },
        Ask::Sell { seller: Side::Member(0), buyer: Side::Person(1), amount_minor: 1, term: 30 },
        Ask::Settle { contract: 0, amount_minor: 1 },
        Ask::Cure { contract: 0, amount_minor: 1 },
        Ask::Extend { contract: 0, new_maturity_epoch: 1 },
        Ask::Transfer { contract: 0, new_debtor: 1 },
        Ask::Declare { supply_minor: 1 },
        Ask::Propose {
            proposal: edet_state::types::ProposalKind::ParamChange {
                key: edet_state::types::ParamKey::RiskK,
                value: 1.0,
            },
        },
        Ask::Assent { proposal: 0 },
        Ask::ListBeneficiaries { entries: vec![(1, 1.0)] },
        Ask::ApproveSupporter { supporter: 1, approved: true },
        Ask::RegisterGuardians { guardians: vec![1], threshold: 1, veto_window_epochs: 1 },
    ];
    // A proposal is an ask with an enum inside it, externally tagged: a
    // `Redenominate` writes `num` and `den` where a `ParamChange` writes `key`
    // and `value`, and sampling one variant named neither of the others. The
    // same holds for an instruction, which is three policies with three
    // different payloads. Under-sampling one level down is the same defect as
    // under-sampling the asks, one level in.
    use edet_state::types::{ParamKey, ProposalKind};
    let proposals = [
        ProposalKind::ParamChange { key: ParamKey::RiskK, value: 1.0 },
        ProposalKind::Redenominate { num: 1, den: 1 },
        ProposalKind::Suspend { member: 1 },
        ProposalKind::Unsuspend { member: 1 },
        ProposalKind::ValidatorPower { member: 1, power: 1 },
        ProposalKind::SeedAmendment { amount: 1.0 },
    ];
    let instructions = [
        Instruction::PayAtMaturity { contract: 0, paid_with: PaidWith::Cash },
        Instruction::AcceptPayments,
        Instruction::AcceptFrom { member: 1, max_amount_minor: 1 },
    ];
    let acts: Vec<Act> = acts
        .into_iter()
        .chain(asks.into_iter().map(|ask| Act::Solo { ask }))
        .chain(
            proposals
                .into_iter()
                .map(|proposal| Act::Solo { ask: Ask::Propose { proposal } }),
        )
        .chain(instructions.into_iter().map(|instruction| Act::SetInstruction { instruction }))
        .collect();
    let digest = "0".repeat(64);
    let mut results = vec![
        ActResult::Opened { digest: digest.clone() },
        ActResult::Signed { digest: digest.clone() },
        ActResult::Applied { tx_id: String::new(), contract: Some(0), seated: vec![0], cash_moved: 1 },
        ActResult::Refused { code: String::new() },
        ActResult::Rejected { reason: String::new() },
        ActResult::Declined { digest },
        ActResult::Paid,
        ActResult::Said,
        ActResult::InstructionSet { id: 1 },
        ActResult::InstructionRevoked { id: 1 },
    ];
    results.extend(std::iter::repeat_n(
        ActResult::Applied { tx_id: String::new(), contract: Some(0), seated: vec![0], cash_moved: 1 },
        acts.len() - results.len(),
    ));
    let paired: Vec<(Act, ActResult)> = acts.iter().cloned().zip(results.iter().cloned()).collect();
    let events = [
        Event::TickOpened { tick: 1, epoch: 1, schedule: vec![0] },
        Event::CardWritten {
            tick: 1,
            person: 0,
            name: String::new(),
            card: String::new(),
            tier: cfg.default_tier,
            exchange: Some(0),
        },
        Event::Day {
            tick: 1,
            person: 0,
            exchange: 0,
            acts: acts.to_vec(),
            results: results.clone(),
            refused: paired.clone(),
            seen: None,
            usage: Usage::default(),
        },
        Event::Silent { tick: 1, person: 0, reason: String::new(), usage: Usage::default() },
        Event::Instructions { tick: 1, fired: acts.iter().cloned().map(|a| (0, a)).collect(), results },
        Event::Violation {
            tick: 1,
            person: Some(0),
            act: String::new(),
            tx: String::new(),
            signers: vec![0],
            invariant: String::new(),
        },
        Event::ModelChanged { tick: 1, from: String::new(), to: String::new() },
        Event::Ended { tick: 1, reason: String::new() },
        Event::Reopened { tick: 1, cap_dollars: 0.0, spent: 0.0 },
    ];
    let views: Vec<Value> = events.iter().filter_map(event_view).collect();
    let draws = vec![json!({ "person": 0, "buy_from": [1, 1], "sell_to": [1, 1] })];
    let mut snap = snapshot(&w, 0, &views, &[0], &draws);
    snap["unclosed"] = json!(true);
    let manifest = tape::Manifest {
        tape_format: tape::TAPE_FORMAT,
        config: cfg.clone(),
        backend: String::new(),
        model: String::new(),
        knowledge: Vec::new(),
        not_a_run: true,
    };
    w.ended = Some(String::new());
    for p in &mut w.persons {
        p.retired = Some(String::new());
    }
    let run = run_view(&manifest, &w, &[json!({ "tick": 0, "epoch": 0, "digest": Value::Null })], &Adoption::default());
    let life = life_entry(&tape::Exchange {
        id: 0,
        person: 0,
        tick: 0,
        what: tape::ExchangeKind::Day,
        messages: vec![json!({ "role": "user", "content": [
            { "type": "text", "text": "" },
            { "type": "tool_use", "id": "", "name": "", "input": {} },
            { "type": "tool_result", "tool_use_id": "", "content": "", "is_error": false },
        ] })],
    });
    serde_json::to_string_pretty(&json!({ "run": run, "day": snap, "life": life })).map_err(|e| e.to_string())
}
