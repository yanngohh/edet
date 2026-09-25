//! **The world a community lives in, and the one way it changes.**
//!
//! The ledger (a `Driver`: `apply` runs, the audit decides), the pending pool
//! the node itself runs, the money beside the ledger, everybody's words, and
//! the standing instructions people leave. It changes only through the methods
//! [`World::apply`] calls for each event — `open_tick`, `run_act`, `close_tick`
//! — whether a run is being lived or replayed from its tape, so the two cannot
//! drift.

use std::collections::BTreeMap;

use serde::Serialize;

use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::{ContractId, ContractStatus, Key, MemberId, Party};
use edet_swarm::driver::{epochs, Audit, Driver};
use edet_swarm::intent::compose;
use edet_swarm::keys::{fresh_key, member_key};
use edet_swarm::population::World as Seats;
use edet_swarm::rng::{mix, Rng};
use edet_view::pending::{Completed, PendingPool, PendingSignReq, Scheme, SignOutcome};

use crate::acts::{Act, ActResult, Ask, Instruction, PaidWith};
use crate::config::{Card, RunConfig, Tier};
use crate::economy::Economy;
use crate::event::{Event, PersonRecord};
use crate::{fmt_minor, hex, streams, unhex};

pub const CHAIN_ID: &str = "edet-civitas";

/// The pool's signing scheme inside a world: there are no keys to sign with,
/// so a signature is the session's word that it is this person, and the
/// dispatcher is what makes that word true. The digest keys an entry and is
/// signed by nobody.
pub struct Session {
    pub signer: Key,
}

impl Scheme for Session {
    fn digest(&self, chain_id: &str, tx: &Tx, nonce: &[u8; 16], not_after_epoch: u64) -> Option<[u8; 32]> {
        let bytes = edet_state::codec::encode(&(chain_id, tx, nonce, not_after_epoch)).ok()?;
        Some(edet_state::root::value_digest(&bytes))
    }

    fn verify(&self, key: &Key, _message: &[u8], _signature: &[u8]) -> bool {
        *key == self.signer
    }
}

#[derive(Clone, Debug)]
pub struct Person {
    pub index: usize,
    pub key: Key,
    pub member: Option<MemberId>,
    pub founder: bool,
    pub card_name: String,
    pub card: Option<String>,
    pub tier: Option<Tier>,
    pub joined_tick: u64,
    pub introduced_by: Option<usize>,
    /// How many newcomers this person has introduced: the next one is named by
    /// `fresh_key(index, minted)`.
    pub minted: u32,
    pub last_day: Option<u64>,
    /// Why this person is no longer scheduled, if they are not.
    pub retired: Option<String>,
    /// The last word on the square or in the mail this person has been told of.
    pub seen_seq: u64,
    /// What happened to them since their last day, for the news they wake to.
    pub news: Vec<String>,
    pub money: crate::economy::CashNews,
}

impl Person {
    pub fn address(&self) -> String {
        edet_view::disclose::address_hex(&edet_view::disclose::key_address_bytes(&self.key))
    }

    pub fn party(&self) -> Party {
        match self.member {
            Some(id) => Party::Member(id),
            None => Party::Key(self.key),
        }
    }
}

/// What the pool entry an offer opened was about, which the pool does not keep.
#[derive(Clone, Debug)]
pub struct OfferMeta {
    pub opener: usize,
    pub paid_with: Option<PaidWith>,
    pub what: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Said {
    pub seq: u64,
    pub tick: u64,
    pub person: usize,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Mail {
    pub seq: u64,
    pub tick: u64,
    pub from: usize,
    pub to: usize,
    pub text: String,
}

/// A violated invariant, as the run found it.
#[derive(Clone, Debug)]
pub struct Found {
    pub person: Option<usize>,
    pub act: String,
    pub tx: String,
    pub signers: Vec<MemberId>,
    pub invariant: String,
}

#[derive(Clone)]
pub struct World {
    pub cfg: RunConfig,
    pub d: DriverBox,
    pub pool: PendingPool,
    pub offers: BTreeMap<[u8; 32], OfferMeta>,
    pub seats: Seats,
    pub by_key: BTreeMap<Key, usize>,
    pub economy: Economy,
    /// Who knows whom, which the ledger cannot see and every day consults.
    pub social: crate::social::Social,
    pub persons: Vec<Person>,
    pub square: Vec<Said>,
    pub mail: Vec<Mail>,
    pub seq: u64,
    pub instructions: BTreeMap<u64, (usize, Instruction)>,
    pub next_instruction: u64,
    next_tx: u64,
    trace: Vec<u8>,
    pub tick: u64,
    pub genesis_epoch: u64,
    pub schedule: Vec<usize>,
    pub turns_used: u64,
    pub found: Option<Found>,
    pub ended: Option<String>,
}

/// A `Driver` that can be copied: a copy of the ledger with the audit off, for
/// checking what a day's acts come to before they are applied.
pub struct DriverBox(pub Driver);

impl Clone for DriverBox {
    fn clone(&self) -> Self {
        DriverBox(Driver::new(self.0.st.clone()).audit_mode(Audit::Off).report_violations())
    }
}

/// Draw who is present on the first day: which card each plays, and who holds
/// the seed.
pub fn genesis(cfg: &RunConfig, cards: &[Card]) -> Event {
    let mut rng = Rng::seeded(mix(cfg.world_seed, streams::CARDS));
    let mut order: Vec<usize> = Vec::new();
    let mut persons = Vec::with_capacity(cfg.population as usize);
    for i in 0..cfg.population as usize {
        if order.is_empty() {
            order = (0..cards.len()).collect();
            for j in (1..order.len()).rev() {
                let k = rng.below(j as u64 + 1) as usize;
                order.swap(j, k);
            }
        }
        let drawn = &cards[order.pop().expect("refilled above")];
        let (card_name, card, tier) = if cfg.control {
            ("honest trader".to_string(), cfg.honest_card.clone(), cfg.default_tier)
        } else {
            (drawn.name.clone(), drawn.card.clone(), cfg.tier_of(&drawn.name))
        };
        let key = member_key(i);
        // The people who already use edet are the first rows the ledger holds,
        // so a member number is an index into them; everybody else is present
        // with no account, and the first bonded trade that names their key is
        // what seats them.
        let uses_edet = cfg.adopters_at_genesis.is_none_or(|a| i < a as usize);
        persons.push(PersonRecord {
            index: i,
            key: hex(&key),
            address: edet_view::disclose::address_hex(&edet_view::disclose::key_address_bytes(&key)),
            founder: i < cfg.founders as usize,
            member: uses_edet.then_some(i as MemberId),
            card_name,
            card: Some(card),
            tier: Some(tier),
            joined_tick: 0,
            introduced_by: None,
        });
    }
    Event::Genesis { chain_id: CHAIN_ID.into(), persons }
}

/// What a person had read when their day was written.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct Seen {
    pub seq: u64,
    pub news: usize,
}

impl World {
    /// Found the community an `Event::Genesis` describes: founders declare the
    /// seed, everybody else opens an account holding nothing, and nobody backs
    /// anybody.
    pub fn found(cfg: &RunConfig, e: &Event) -> Result<World, String> {
        let Event::Genesis { chain_id, persons } = e else { return Err("a tape begins with its genesis".into()) };
        let mut st = State { chain_id: chain_id.clone(), ..Default::default() };
        st.params.seal_amounts = cfg.seal_amounts;
        let mut seats = Seats::default();
        let mut by_key = BTreeMap::new();
        let mut economy = Economy::new(&cfg.economy, cfg.world_seed);
        let mut people = Vec::with_capacity(persons.len());
        for p in persons {
            let key: Key = unhex(&p.key).ok_or("a genesis key is not 32 bytes of hex")?;
            match p.member {
                Some(_) => {
                    let id = if p.founder {
                        st.add_underwriter(vec![key], State::from_minor(cfg.founder_supply_minor))
                            .map_err(|e| format!("founder {}: {}", p.index, e.0))?
                    } else {
                        st.new_account(vec![key])
                    };
                    if Some(id) != p.member {
                        return Err(format!(
                            "person {} was founded as member {id}, the tape says {:?}",
                            p.index, p.member
                        ));
                    }
                    seats.claim(id, p.index);
                }
                // Present in the town, and not on the ledger: no row is
                // written until something bonds a trade that names this key.
                None if p.founder => return Err(format!("founder {} holds no account", p.index)),
                None => {}
            }
            by_key.insert(key, p.index);
            economy.open_purse(p.index, &cfg.household_of(&p.card_name));
            people.push(person_from(p, key));
        }
        let social = crate::social::Social::found(
            &cfg.social,
            cfg.world_seed,
            &people.iter().map(|p| p.card_name.clone()).collect::<Vec<_>>(),
        );
        let genesis_epoch = st.epoch;
        Ok(World {
            cfg: cfg.clone(),
            d: DriverBox(Driver::new(st).report_violations()),
            pool: PendingPool::default(),
            offers: BTreeMap::new(),
            seats,
            by_key,
            economy,
            social,
            persons: people,
            square: Vec::new(),
            mail: Vec::new(),
            seq: 0,
            instructions: BTreeMap::new(),
            next_instruction: 1,
            next_tx: 0,
            trace: Vec::new(),
            tick: 0,
            genesis_epoch,
            schedule: Vec::new(),
            turns_used: 0,
            found: None,
            ended: None,
        })
    }

    pub fn st(&self) -> &State {
        &self.d.0.st
    }

    /// Apply one event read off a tape, checking that what it records is what
    /// the world comes to.
    pub fn apply(&mut self, e: &Event) -> Result<(), String> {
        match e {
            Event::Genesis { .. } => Err("a second genesis".into()),
            Event::TickOpened { tick, epoch, schedule } => {
                let at = self.open_tick(*tick, schedule.clone());
                if at != *epoch {
                    return Err(format!("day {tick} opened at epoch {at}, the tape says {epoch}"));
                }
                Ok(())
            }
            Event::CardWritten { person, name, card, tier, .. } => {
                self.write_card(*person, name, card, *tier);
                Ok(())
            }
            Event::Day { tick, person, acts, results, seen, .. } => {
                let seen = seen.unwrap_or_else(|| self.seen_of(*person));
                let got = self.run_day(*tick, *person, acts, seen);
                same(&got, results, *tick, *person)
            }
            Event::Silent { tick, person, reason, .. } => {
                self.silent_day(*tick, *person, reason);
                Ok(())
            }
            Event::Instructions { tick, fired, results } => {
                let got: Vec<ActResult> = fired.iter().map(|(p, a)| self.run_fired(*tick, *p, a)).collect();
                same(&got, results, *tick, usize::MAX)
            }
            Event::Violation { .. } => {
                if self.found.is_none() {
                    return Err("the tape records a violation this replay did not find".into());
                }
                Ok(())
            }
            Event::TickClosed { tick, digest, state_root, turns_used, .. } => {
                let (d, r) = self.close_tick();
                if d != *digest || r != *state_root || self.turns_used != *turns_used {
                    return Err(format!(
                        "day {tick} closed on digest {d}, root {r} and {} days used; the tape says {digest}, {state_root}, {turns_used}",
                        self.turns_used
                    ));
                }
                Ok(())
            }
            Event::ModelChanged { .. } => Ok(()),
            Event::Ended { reason, .. } => {
                self.ended = Some(reason.clone());
                Ok(())
            }
            Event::Reopened { .. } => {
                self.ended = None;
                Ok(())
            }
        }
    }

    // ------------------------------------------------------------ the day --

    /// Cross into the next epoch — the sweep runs, stakes decay, the audit
    /// looks — and move the weather and everybody's money.
    pub fn open_tick(&mut self, tick: u64, schedule: Vec<usize>) -> u64 {
        self.tick = tick;
        let epoch = self.genesis_epoch + tick;
        self.d.0.goto(epoch);
        self.note_violation(None, "the epoch sweep", "-", Vec::new());
        let news = self.economy.open_day(tick, &self.social, &self.cfg.social);
        for (p, n) in self.persons.iter_mut().zip(news) {
            p.money.income += n.income;
            p.money.income_cut |= n.income_cut;
            p.money.bill += n.bill;
            p.money.bill_short += n.bill_short;
            p.money.arrears_paid += n.arrears_paid;
            p.money.cut_began |= n.cut_began;
            // Money adds up over the days a person did not pick up their
            // wallet; a trade does not. A supplier who wanted paying two days
            // ago and a customer who wanted buying yesterday are gone: what a
            // person meets is the latest one drawn.
            if n.buy_from.is_some() {
                p.money.buy_from = n.buy_from;
            }
            if n.sell_to.is_some() {
                p.money.sell_to = n.sell_to;
            }
        }
        self.schedule = schedule;
        epoch
    }

    /// Apply a person's day: their news is spent, and their acts land in the
    /// order they made them.
    pub fn run_day(&mut self, tick: u64, person: usize, acts: &[Act], seen: Seen) -> Vec<ActResult> {
        self.turns_used += 1;
        self.persons[person].last_day = Some(tick);
        // What a day SPENDS is what its note showed them, and no more. Where
        // a tick's days are lived together, a neighbour's payment, offer or
        // post lands between the note and the apply: marking it seen here
        // would lose it for ever, since the note it never appeared in is the
        // only place it would have been read.
        let news = &mut self.persons[person].news;
        news.drain(..seen.news.min(news.len()));
        self.persons[person].money = Default::default();
        self.persons[person].seen_seq = seen.seq;
        let results: Vec<ActResult> = acts.iter().map(|a| self.run_act(tick, person, a)).collect();
        // An act that passed the check on a copy of the world can still be
        // refused by the world itself — another person reached a binding
        // ceiling first, and under `concurrency` they did it the same day.
        // The person is told, or their life carries a trade that never was.
        for (act, r) in acts.iter().zip(&results) {
            if matches!(r, ActResult::Refused { .. } | ActResult::Rejected { .. }) {
                let line = format!("What you did did not take: {} — {}", describe_act(act), r.describe());
                self.persons[person].news.push(line);
            }
        }
        results
    }

    /// What a person had been shown when their day was written: the square and
    /// the mail they had read up to, and how many lines of news their note
    /// carried. A day marks these spent and nothing that arrived after them.
    /// **What brings a person with no account to pick up a wallet today.**
    /// An offer waiting for them is the scheduler's own rule and is not here.
    /// These two are the person's own day: bills that have got ahead of them,
    /// and a community they can see settling what it owes. Both are read off
    /// the world as the tick opened, and the draw is seeded, so a replay comes
    /// to the same people on the same days.
    pub fn wants_in(&self, person: usize, tick: u64) -> Option<&'static str> {
        let a = &self.cfg.adoption;
        if a.arrears_minor > 0 && self.economy.purses[person].arrears >= a.arrears_minor {
            return Some("arrears");
        }
        if a.per_settled_ppm == 0 {
            return None;
        }
        // What the town has seen come good: the ledger keeps a closed contract
        // for its retention window, so this is recent by construction.
        let settled = self
            .st()
            .contracts
            .values()
            .filter(|c| matches!(c.status, ContractStatus::Settled | ContractStatus::Cured))
            .count() as u64;
        if settled == 0 {
            return None;
        }
        let chance = (a.per_settled_ppm * settled).min(a.max_ppm);
        let mut rng = Rng::seeded(mix(mix(mix(self.cfg.world_seed, streams::ADOPTION), tick), person as u64));
        (rng.below(1_000_000) < chance).then_some("saw it work")
    }

    /// **Whom the wallet has notified today**: everybody with a card whose
    /// signature a request that arrived THIS day is waiting for. A request
    /// opened by the person themself is not waiting for them.
    pub fn notified_today(&self) -> Vec<usize> {
        let st = self.st();
        let now = epochs(st.epoch);
        self.persons
            .iter()
            .filter(|p| p.card.is_some() && p.retired.is_none())
            .filter(|p| self.pool.for_party(st, p.party()).0.iter().any(|(_, e)| e.created_secs == now))
            .map(|p| p.index)
            .collect()
    }

    pub fn seen_of(&self, person: usize) -> Seen {
        Seen { seq: self.seq, news: self.persons[person].news.len() }
    }

    pub fn silent_day(&mut self, tick: u64, person: usize, reason: &str) {
        self.turns_used += 1;
        self.persons[person].last_day = Some(tick);
        if reason.starts_with(CONTEXT_FULL) {
            self.persons[person].retired = Some(reason.to_string());
        }
    }

    fn run_fired(&mut self, tick: u64, person: usize, act: &Act) -> ActResult {
        let r = self.run_act(tick, person, act);
        let line = format!("A standing instruction of yours acted: {} — {}", describe_act(act), r.describe());
        self.persons[person].news.push(line);
        r
    }

    /// The fold every transaction and its answer goes into, and the state root.
    pub fn close_tick(&mut self) -> (String, String) {
        let digest = hex(&self.trace_digest());
        let root = edet_state::root::state_root(self.st())
            .map(|r| hex(&r))
            .unwrap_or_else(|e| format!("unrooted: {e:?}"));
        (digest, root)
    }

    fn trace_digest(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, b) in self.trace.iter().take(32).enumerate() {
            out[i] = *b;
        }
        out
    }

    fn fold(&mut self, bytes: &[u8]) {
        let mut buf = std::mem::take(&mut self.trace);
        buf.extend_from_slice(bytes);
        self.trace = edet_state::root::value_digest(&buf).to_vec();
    }

    pub fn write_card(&mut self, person: usize, name: &str, card: &str, tier: Tier) {
        let p = &mut self.persons[person];
        p.card_name = name.to_string();
        p.card = Some(card.to_string());
        p.tier = Some(tier);
    }

    // ----------------------------------------------------------- the acts --

    /// Apply one act, by one person. Every refusal is a result, never an error:
    /// a person may try anything, and what the world says is the finding.
    pub fn run_act(&mut self, tick: u64, person: usize, act: &Act) -> ActResult {
        match act {
            Act::Offer { ask, required, nonce, not_after_epoch, newcomer, paid_with } => {
                self.offer(tick, person, ask, required, nonce, *not_after_epoch, *newcomer, *paid_with)
            }
            Act::Solo { ask } => self.solo(person, ask),
            Act::Sign { digest } => self.sign(tick, person, digest),
            Act::Decline { digest } => self.decline(person, digest),
            Act::PayCash { to, amount_minor, note } => match self.economy.pay(person, *to, *amount_minor) {
                Ok(()) => {
                    let line = format!(
                        "{} paid you {} in cash{}",
                        self.name_of(person),
                        fmt_minor(*amount_minor),
                        if note.is_empty() { String::new() } else { format!(": \"{note}\"") }
                    );
                    self.persons[*to].news.push(line);
                    ActResult::Paid
                }
                Err(r) => ActResult::Rejected { reason: r.into() },
            },
            Act::Post { text } => {
                self.seq += 1;
                self.square.push(Said { seq: self.seq, tick, person, text: text.clone() });
                ActResult::Said
            }
            Act::Message { to, text } => {
                if *to >= self.persons.len() || *to == person {
                    return ActResult::Rejected { reason: "no such recipient".into() };
                }
                self.seq += 1;
                self.mail
                    .push(Mail { seq: self.seq, tick, from: person, to: *to, text: text.clone() });
                ActResult::Said
            }
            Act::Diary { .. } => ActResult::Said,
            Act::SetInstruction { instruction } => match self.check_instruction(person, instruction) {
                Ok(()) => {
                    let id = self.next_instruction;
                    self.next_instruction += 1;
                    self.instructions.insert(id, (person, instruction.clone()));
                    ActResult::InstructionSet { id }
                }
                Err(reason) => ActResult::Rejected { reason },
            },
            Act::RevokeInstruction { id } => match self.instructions.get(id) {
                Some((owner, _)) if *owner == person => {
                    self.instructions.remove(id);
                    ActResult::InstructionRevoked { id: *id }
                }
                _ => ActResult::Rejected { reason: "you have no instruction with that number".into() },
            },
        }
    }

    fn check_instruction(&self, person: usize, i: &Instruction) -> Result<(), String> {
        let me = self.persons[person].member.ok_or("you have no account yet")?;
        match i {
            Instruction::PayAtMaturity { contract, .. } => match self.st().contracts.get(contract) {
                Some(c) if c.debtor == me => Ok(()),
                Some(_) => Err(format!("you are not the debtor on contract {contract}")),
                None => Err(format!("there is no contract {contract}")),
            },
            Instruction::AcceptPayments => Ok(()),
            Instruction::AcceptFrom { member, .. } => {
                if self.st().members.contains_key(member) && *member != me {
                    Ok(())
                } else {
                    Err(format!("there is no other member {member}"))
                }
            }
        }
    }

    /// The transaction an ask composes to from this member: the one door, with
    /// the keys it already holds and the members it must ask.
    pub fn compose(&self, member: MemberId, ask: &Ask) -> edet_swarm::intent::Composed {
        // A person named by index is named to the ledger by their key. An
        // index nobody has names nobody: `offer` refuses it before this runs.
        let key_of = |i: usize| self.persons.get(i).map_or([0u8; 32], |p| p.key);
        compose(self.st(), &self.seats, member, &ask.intent(&key_of))
    }

    /// An ask as this world names its parties: a person with no account by
    /// their address, since that is the one name the town has for them.
    pub fn describe(&self, ask: &Ask) -> String {
        match ask.person() {
            Some(i) => describe_ask(ask).replace(&person_placeholder(i), &self.name_of(i)),
            None => describe_ask(ask),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn offer(
        &mut self,
        tick: u64,
        person: usize,
        ask: &Ask,
        required: &[Party],
        nonce: &str,
        not_after_epoch: u64,
        newcomer: Option<usize>,
        paid_with: Option<PaidWith>,
    ) -> ActResult {
        let Some(member) = self.persons[person].member else {
            return ActResult::Rejected { reason: "you have no account yet".into() };
        };
        let Some(nonce) = unhex::<16>(nonce) else { return ActResult::Rejected { reason: "a malformed offer".into() } };
        if let Some((owner, n)) = ask.newcomer() {
            if owner != person || n != self.persons[person].minted || newcomer != Some(self.persons.len()) {
                return ActResult::Rejected { reason: "that newcomer is not yours to introduce".into() };
            }
        }
        // A neighbour named by address is seated by this trade, so they must
        // be somebody, not the offerer, and still without a row when the day
        // applies: another member may have seated them since the morning.
        if let Some(i) = ask.person() {
            match self.persons.get(i) {
                Some(p) if i != person && p.member.is_none() => {}
                Some(p) if p.member.is_some() => {
                    return ActResult::Rejected {
                        reason: format!("{} has an account now: name them by member number", self.name_of(i)),
                    }
                }
                _ => return ActResult::Rejected { reason: "no such neighbour".into() },
            }
        }
        let key = self.persons[person].key;
        let composed = self.compose(member, ask);
        let req = PendingSignReq {
            tx: composed.tx,
            nonce,
            not_after_epoch,
            required: required.to_vec(),
            min_sigs: required.len(),
            signer: key,
            signature: Vec::new(),
            invite: None,
        };
        let scheme = Session { signer: key };
        let Some(digest) = edet_view::pending::request_digest(&scheme, self.st(), &req) else {
            return ActResult::Rejected { reason: "the transaction does not encode".into() };
        };
        let now = epochs(self.st().epoch);
        match self.pool.sign(&scheme, &self.d.0.st, req, now) {
            SignOutcome::Recorded { .. } => {
                if let (Some(idx), Some((owner, n))) = (newcomer, ask.newcomer()) {
                    self.introduce(tick, idx, owner, n);
                }
                let what = self.describe(ask);
                self.offers
                    .insert(digest, OfferMeta { opener: person, paid_with, what: what.clone() });
                let from = self.name_of(person);
                for p in required {
                    if let Some(i) = self.person_of_party(p) {
                        if i != person {
                            let line = format!("{from} sent you an offer: {what} (ref {})", &hex(&digest)[..12]);
                            self.persons[i].news.push(line);
                        }
                    }
                }
                ActResult::Opened { digest: hex(&digest) }
            }
            SignOutcome::Complete(done) => self.complete(person, digest, *done),
            SignOutcome::Rejected(reason) => ActResult::Rejected { reason: reason.into() },
        }
    }

    /// A newcomer is a person from the moment somebody offers them a first
    /// trade: a key, a household, and a card to be written before their first
    /// day.
    fn introduce(&mut self, tick: u64, index: usize, owner: usize, n: u32) {
        let key = fresh_key(owner, n);
        self.persons[owner].minted += 1;
        self.social.grow();
        self.social.meet(owner, index);
        self.by_key.insert(key, index);
        // A newcomer's card is written during the run and names nothing the
        // table knows, so their household is the town's own base.
        self.economy.open_purse(index, &crate::config::Household::default());
        self.persons.push(Person {
            index,
            key,
            member: None,
            founder: false,
            card_name: String::new(),
            card: None,
            tier: None,
            joined_tick: tick,
            introduced_by: Some(owner),
            minted: 0,
            last_day: None,
            retired: None,
            seen_seq: self.seq,
            news: Vec::new(),
            money: Default::default(),
        });
    }

    fn solo(&mut self, person: usize, ask: &Ask) -> ActResult {
        let Some(member) = self.persons[person].member else {
            return ActResult::Rejected { reason: "you have no account yet".into() };
        };
        let composed = self.compose(member, ask);
        if !composed.asks.is_empty() {
            return ActResult::Rejected { reason: "this needs somebody else's signature: send it as an offer".into() };
        }
        let what = ask.name().to_string();
        self.submit(Some(person), &what, composed.tx, composed.signers, 0, None)
    }

    fn sign(&mut self, tick: u64, person: usize, digest: &str) -> ActResult {
        let Some(d) = unhex::<32>(digest) else { return ActResult::Rejected { reason: "no such offer".into() } };
        let party = self.persons[person].party();
        let key = self.persons[person].key;
        let (awaiting, _) = self.pool.for_party(&self.d.0.st, party);
        let Some((_, entry)) = awaiting.into_iter().find(|(x, _)| *x == d) else {
            return ActResult::Rejected { reason: "no offer with that reference is waiting for you".into() };
        };
        let req = PendingSignReq {
            tx: entry.tx.clone(),
            nonce: entry.nonce,
            not_after_epoch: entry.not_after_epoch,
            required: entry.required.clone(),
            min_sigs: entry.min_sigs,
            signer: key,
            signature: Vec::new(),
            invite: None,
        };
        let now = epochs(self.st().epoch);
        let _ = tick;
        match self.pool.sign(&Session { signer: key }, &self.d.0.st, req, now) {
            SignOutcome::Recorded { .. } => ActResult::Signed { digest: digest.to_string() },
            SignOutcome::Complete(done) => self.complete(person, d, *done),
            SignOutcome::Rejected(reason) => ActResult::Rejected { reason: reason.into() },
        }
    }

    /// Every signature is in: apply the transaction, and move the cash if the
    /// discharge was paid in cash.
    fn complete(&mut self, person: usize, digest: [u8; 32], done: Completed) -> ActResult {
        let meta = self.offers.remove(&digest);
        let what = meta.as_ref().map(|m| m.what.clone()).unwrap_or_else(|| "an offer".into());
        let mut cash = None;
        if meta.as_ref().and_then(|m| m.paid_with) == Some(PaidWith::Cash) {
            if let Tx::Settle { contract, amount } | Tx::Cure { contract, amount } = &done.tx {
                if let Some(c) = self.st().contracts.get(contract) {
                    let (from, to) = (self.seats.agent_of(c.debtor), self.seats.agent_of(c.creditor));
                    let amount = State::to_minor(*amount);
                    if from == usize::MAX || to == usize::MAX || self.economy.purses[from].cash < amount {
                        let r = ActResult::Rejected { reason: "the payer's cash does not cover the payment".into() };
                        self.tell_parties(
                            &done,
                            &format!("{what} (ref {}): {}", &hex(&digest)[..12], r.describe()),
                            person,
                        );
                        return r;
                    }
                    cash = Some((from, to, amount));
                }
            }
        }
        let mut signers = done.signers.clone();
        signers.sort_by_key(|k| self.st().member_of_key(k).unwrap_or(MemberId::MAX));
        signers.dedup();
        let label = format!("offer:{}", what);
        let r = self.submit(Some(person), &label, done.tx.clone(), signers, done.not_after_epoch, cash);
        if matches!(r, ActResult::Applied { .. }) {
            // They have dealt with each other now, whoever they were this morning.
            let dealt: Vec<usize> = done.signers.iter().filter_map(|k| self.by_key.get(k).copied()).collect();
            for (n, a) in dealt.iter().enumerate() {
                for b in &dealt[n + 1..] {
                    self.social.meet(*a, *b);
                }
            }
        }
        self.tell_parties(&done, &format!("{what} (ref {}): {}", &hex(&digest)[..12], r.describe()), person);
        r
    }

    fn tell_parties(&mut self, done: &Completed, line: &str, except: usize) {
        let mut told = Vec::new();
        for k in &done.signers {
            if let Some(&i) = self.by_key.get(k) {
                if i != except && !told.contains(&i) {
                    told.push(i);
                    self.persons[i].news.push(line.to_string());
                }
            }
        }
    }

    /// Hand a transaction to the ledger under the next id. `not_after` of zero
    /// is the widest window the ledger admits from today.
    fn submit(
        &mut self,
        person: Option<usize>,
        what: &str,
        tx: Tx,
        signers: Vec<Key>,
        not_after: u64,
        cash: Option<(usize, usize, u64)>,
    ) -> ActResult {
        self.next_tx += 1;
        let mut id = [0u8; 32];
        id[0] = 0xA6;
        id[1..9].copy_from_slice(&self.next_tx.to_be_bytes());
        let epoch = self.st().epoch;
        let not_after = if not_after == 0 { epoch + edet_kernel::constants::MAX_TX_LIFETIME_EPOCHS } else { not_after };
        let contract_before = self.st().next_contract;
        let member_before = self.st().next_member;
        self.fold(&edet_state::codec::encode(&tx).unwrap_or_default());
        let tx_debug = format!("{tx:?}");
        let out = self.d.0.apply_raw(tx, id, not_after, &signers);
        let signer_ids: Vec<MemberId> = signers.iter().filter_map(|k| self.st().member_of_key(k)).collect();
        match out {
            Ok(()) => {
                self.fold(b"ok");
                let mut moved = 0;
                if let Some((from, to, amount)) = cash {
                    if self.economy.pay(from, to, amount).is_ok() {
                        moved = amount;
                    }
                }
                let mut seated = Vec::new();
                for m in member_before..self.st().next_member {
                    if let Some(k) = self.st().members.get(&m).and_then(|x| x.keys.first()).copied() {
                        if let Some(&i) = self.by_key.get(&k) {
                            self.persons[i].member = Some(m);
                            self.seats.claim(m, i);
                        }
                    }
                    seated.push(m);
                }
                let contract = (self.st().next_contract > contract_before).then_some(contract_before);
                self.note_violation(person, what, &tx_debug, signer_ids);
                ActResult::Applied { tx_id: hex(&id), contract, seated, cash_moved: moved }
            }
            Err(e) => {
                self.fold(format!("refused:{}", e.0).as_bytes());
                self.note_violation(person, what, &tx_debug, signer_ids);
                ActResult::Refused { code: e.0.to_string() }
            }
        }
    }

    fn note_violation(&mut self, person: Option<usize>, act: &str, tx: &str, signers: Vec<MemberId>) {
        if self.found.is_some() {
            return;
        }
        if let Some(v) = self.d.0.violation() {
            self.found = Some(Found { person, act: act.into(), tx: tx.into(), signers, invariant: v.into() });
        }
    }

    fn decline(&mut self, person: usize, digest: &str) -> ActResult {
        let Some(d) = unhex::<32>(digest) else { return ActResult::Rejected { reason: "no such offer".into() } };
        let key = self.persons[person].key;
        if !self.pool.decline(&Session { signer: key }, &self.d.0.st, d, &key, &[]) {
            return ActResult::Rejected { reason: "no offer with that reference is yours to decline".into() };
        }
        if let Some(meta) = self.offers.remove(&d) {
            if meta.opener != person {
                let line =
                    format!("{} declined your offer: {} (ref {})", self.name_of(person), meta.what, &digest[..12]);
                self.persons[meta.opener].news.push(line);
            }
        }
        ActResult::Declined { digest: digest.to_string() }
    }

    // ----------------------------------------------- standing instructions --

    /// What standing instructions do at the end of a day, in order: every
    /// payment due tomorrow is offered, then every waiting entry somebody's
    /// instruction accepts is signed. Each act is applied as it is found, so
    /// an acceptance sees the payment offered before it.
    pub fn fire_instructions(&mut self, tick: u64) -> (Vec<(usize, Act)>, Vec<ActResult>) {
        let mut fired = Vec::new();
        let mut results = Vec::new();
        let epoch = self.st().epoch;
        let pays: Vec<(u64, usize, ContractId, PaidWith)> = self
            .instructions
            .iter()
            .filter_map(|(id, (owner, i))| match i {
                Instruction::PayAtMaturity { contract, paid_with } => Some((*id, *owner, *contract, *paid_with)),
                _ => None,
            })
            .collect();
        for (id, owner, contract, paid_with) in pays {
            let Some(c) = self.st().contracts.get(&contract) else { continue };
            if c.status != ContractStatus::Active || c.maturity_epoch != epoch + 1 {
                continue;
            }
            if Some(c.debtor) != self.persons[owner].member {
                continue;
            }
            let act = Act::Offer {
                ask: Ask::Settle { contract, amount_minor: c.outstanding },
                required: vec![Party::Member(c.debtor), Party::Member(c.creditor)],
                nonce: nonce_for(tick, owner, 1_000_000 + id),
                not_after_epoch: epoch + 1,
                newcomer: None,
                paid_with: Some(paid_with),
            };
            results.push(self.run_fired(tick, owner, &act));
            fired.push((owner, act));
        }
        for (owner, act) in self.acceptances(tick) {
            results.push(self.run_fired(tick, owner, &act));
            fired.push((owner, act));
        }
        (fired, results)
    }

    fn acceptances(&self, _tick: u64) -> Vec<(usize, Act)> {
        let mut out = Vec::new();
        let st = self.st();
        for (owner, i) in self.instructions.values() {
            let person = &self.persons[*owner];
            let Some(me) = person.member else { continue };
            let (awaiting, _) = self.pool.for_party(st, Party::Member(me));
            for (digest, entry) in awaiting {
                let accept = match i {
                    Instruction::AcceptPayments => match &entry.tx {
                        Tx::Settle { contract, .. } | Tx::Cure { contract, .. } => {
                            st.contracts.get(contract).is_some_and(|c| c.creditor == me)
                        }
                        _ => false,
                    },
                    Instruction::AcceptFrom { member, max_amount_minor } => {
                        entry.opener == Party::Member(*member)
                            && tx_amount_minor(&entry.tx).is_some_and(|a| a <= *max_amount_minor)
                    }
                    Instruction::PayAtMaturity { .. } => false,
                };
                let act = Act::Sign { digest: hex(&digest) };
                if accept && !out.iter().any(|(o, a): &(usize, Act)| *o == *owner && same_act(a, &act)) {
                    out.push((*owner, act));
                }
            }
        }
        out
    }

    // -------------------------------------------------------------- lookup --

    pub fn person_of_party(&self, p: &Party) -> Option<usize> {
        match p {
            Party::Member(id) => {
                let i = self.seats.agent_of(*id);
                (i != usize::MAX).then_some(i)
            }
            Party::Key(k) => self.by_key.get(k).copied(),
        }
    }

    pub fn person_of_member(&self, id: MemberId) -> Option<usize> {
        self.person_of_party(&Party::Member(id))
    }

    /// How a person appears to others: their member number if they have one,
    /// and their address.
    pub fn name_of(&self, person: usize) -> String {
        let p = &self.persons[person];
        match p.member {
            Some(id) => format!("member {id} ({})", p.address()),
            None => format!("{} (no account yet)", p.address()),
        }
    }

    /// Who a person means by a member number or an address.
    pub fn resolve(&self, who: &str) -> Option<usize> {
        let who = who.trim();
        if let Some(rest) = who.strip_prefix("0x") {
            let _ = rest;
            return self.persons.iter().position(|p| p.address().eq_ignore_ascii_case(who));
        }
        let id: MemberId = who.trim_start_matches("member").trim().parse().ok()?;
        self.person_of_member(id)
    }

    pub fn record_of(&self, p: &Person) -> PersonRecord {
        PersonRecord {
            index: p.index,
            key: hex(&p.key),
            address: p.address(),
            founder: p.founder,
            member: p.member,
            card_name: p.card_name.clone(),
            card: p.card.clone(),
            tier: p.tier,
            joined_tick: p.joined_tick,
            introduced_by: p.introduced_by,
        }
    }
}

/// The reason a day fails when a person's life no longer fits the model's
/// context: they are not scheduled again.
pub const CONTEXT_FULL: &str = "context full";

fn person_from(p: &PersonRecord, key: Key) -> Person {
    Person {
        index: p.index,
        key,
        member: p.member,
        founder: p.founder,
        card_name: p.card_name.clone(),
        card: p.card.clone(),
        tier: p.tier,
        joined_tick: p.joined_tick,
        introduced_by: p.introduced_by,
        minted: 0,
        last_day: None,
        retired: None,
        seen_seq: 0,
        news: Vec::new(),
        money: Default::default(),
    }
}

fn same(got: &[ActResult], want: &[ActResult], tick: u64, person: usize) -> Result<(), String> {
    if got == want {
        return Ok(());
    }
    Err(format!("day {tick}, person {person}: replay came to {got:?}, the tape says {want:?}"))
}

fn same_act(a: &Act, b: &Act) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// A nonce no other act in the run shares: the day, the person, and which of
/// their acts it is.
pub fn nonce_for(tick: u64, person: usize, n: u64) -> String {
    let bytes = edet_state::root::value_digest(format!("civitas-nonce\n{tick}\n{person}\n{n}").as_bytes());
    hex(&bytes[..16])
}

/// The amount a trade or a payment names. Every other transaction names none,
/// and an instruction bounded by an amount accepts none of them: a transfer
/// that makes its signer the debtor carries the whole outstanding balance and
/// names no amount at all.
fn tx_amount_minor(tx: &Tx) -> Option<u64> {
    match tx {
        Tx::Accept { amount, .. } | Tx::Sale { amount, .. } | Tx::Settle { amount, .. } | Tx::Cure { amount, .. } => {
            Some(State::to_minor(*amount))
        }
        _ => None,
    }
}

/// What a description calls a person named by index, where no world is at
/// hand to give their address: the tape's own name for them.
fn person_placeholder(i: usize) -> String {
    format!("person {i} (no account yet)")
}

pub fn describe_ask(ask: &Ask) -> String {
    use crate::acts::Side;
    let side = |s: &Side| match s {
        Side::Member(id) => format!("member {id}"),
        Side::Newcomer { .. } => "a newcomer".to_string(),
        Side::Person(i) => person_placeholder(*i),
    };
    match ask {
        Ask::Lend { creditor, debtor, amount_minor, term } => {
            format!("{} lends {} to {}, due in {term} days", side(creditor), fmt_minor(*amount_minor), side(debtor))
        }
        Ask::Sell { seller, buyer, amount_minor, term } => format!(
            "{} sells to {} for {} on credit, due in {term} days",
            side(seller),
            side(buyer),
            fmt_minor(*amount_minor)
        ),
        Ask::Settle { contract, amount_minor } => format!("settle {} on contract {contract}", fmt_minor(*amount_minor)),
        Ask::Cure { contract, amount_minor } => format!("cure {} on contract {contract}", fmt_minor(*amount_minor)),
        Ask::Extend { contract, new_maturity_epoch } => {
            format!("extend contract {contract} to day {new_maturity_epoch}")
        }
        Ask::Transfer { contract, new_debtor } => format!("hand contract {contract} to member {new_debtor}"),
        Ask::Declare { supply_minor } => format!("declare supply {}", fmt_minor(*supply_minor)),
        Ask::Exit => "leave the ledger".into(),
        Ask::Propose { proposal } => format!("propose {proposal:?}"),
        Ask::Assent { proposal } => format!("assent to proposal {proposal}"),
        Ask::ListBeneficiaries { entries } => format!("list beneficiaries {entries:?}"),
        Ask::ApproveSupporter { supporter, approved } => {
            format!("{} supporter {supporter}", if *approved { "approve" } else { "withdraw approval of" })
        }
        Ask::RegisterGuardians { guardians, threshold, .. } => {
            format!("register guardians {guardians:?}, {threshold} needed")
        }
    }
}

pub fn describe_act(act: &Act) -> String {
    match act {
        Act::Offer { ask, .. } => format!("offer: {}", describe_ask(ask)),
        Act::Solo { ask } => describe_ask(ask),
        Act::Sign { digest } => format!("sign offer {}", &digest[..12.min(digest.len())]),
        Act::Decline { digest } => format!("decline offer {}", &digest[..12.min(digest.len())]),
        Act::PayCash { to, amount_minor, .. } => format!("pay person {to} {} in cash", fmt_minor(*amount_minor)),
        Act::Post { .. } => "post on the square".into(),
        Act::Message { to, .. } => format!("message person {to}"),
        Act::Diary { .. } => "write in the diary".into(),
        Act::SetInstruction { instruction } => format!("set instruction {instruction:?}"),
        Act::RevokeInstruction { id } => format!("revoke instruction {id}"),
    }
}
