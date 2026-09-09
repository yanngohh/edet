//! **One operator the community really backs, seating rows with wash trades.**
//! The archetype that measures what a row is priced in, at the quantifier the
//! claim is made over: a SET of accounts one operator controls, across every
//! tick of a run rather than at the end of one.
//!
//! **A row is a stock, so its price is a reservation on the graph that is
//! never released.** A seat holds one bond unit of flow from the community's
//! seed to the sponsor, on the shared stake graph, so `unit x seats(S)` can
//! never exceed the all-time peak of the arcs reaching `S` from outside —
//! which is what this probe checks tick by tick, against the peaks it watched
//! accumulate. A write budget would price a RATE instead: headroom refills
//! every epoch, a seated row's own headroom reaches a bond unit after one tick
//! of decay above the wash the farm pays, and the count then compounds.
//!
//! The capacity claim was never the one at risk — the farm's cut as a SET
//! stays at the one honest edge behind it either way. What the seat bounds is
//! the STOCK: rows, permanent, held in every state root and rehashed once an
//! epoch.
//!
//! **The farm settles a seat the tick AFTER it books it.** A strategy emits
//! intents and a row's id does not exist until the adapter has applied one, so
//! a farm that predicted ids would be measuring the adapter. The mechanism is
//! unchanged: a settlement is what writes the stake, and the stake is what
//! makes the child established.

use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    /// What each seat is washed for. Below `ESTABLISHED_FRACTION x v_base` the
    /// seated row qualifies for nothing; above it, the row is established.
    pub wash_minor: u64,
    /// Stop one seat short of the refusal, so the farm is never saturated and
    /// therefore never forfeited. A farm that stops short is never sanctioned,
    /// which is why this is the default.
    pub careful: bool,
    /// Whether every seat also spends a write, so the channel is exercised and
    /// not only the row count.
    pub duds: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { wash_minor: 10_000, careful: true, duds: true }
    }
}

pub struct SybilFarm {
    p: Params,
    me: MemberId,
    agent: usize,
    next_key: u32,
    /// Rows the farm holds, per tick.
    rows_at: Vec<u64>,
    /// Of those, the ones a TRADE seated — the quantity the bound is stated
    /// over. A row the founding ceremony wrote carries no seat and is not the
    /// farm's.
    seated_at: Vec<u64>,
    /// Applied writes, per tick.
    writes_at: Vec<u64>,
    applied_before: u64,
    established_final: u64,
    gross_cut: u64,
    /// The running ALL-TIME peak of every arc reaching the farm's set from
    /// outside it, plus every supply arc a farm member holds, minor units.
    ///
    /// A peak and never a present weight: the bound has no time in it, so an
    /// arc that decayed after a seat was taken on it still counts for what it
    /// once carried, and renewal to an old peak adds nothing.
    peak: std::collections::BTreeMap<(usize, usize), u64>,
    supply_peak: std::collections::BTreeMap<MemberId, u64>,
    /// `Σ peaks / unit` at each tick — the most rows the rule can allow.
    bound_at: Vec<u64>,
    /// How many seats the ledger refused for want of reach. The ceiling has to
    /// be REACHED, or the probe does not detect the mechanism's removal.
    refused_unbacked: u64,
}

impl SybilFarm {
    pub fn new(p: Params, at: &Seating) -> Self {
        SybilFarm {
            p,
            me: at.seat,
            agent: at.agent,
            next_key: 0,
            rows_at: Vec::new(),
            seated_at: Vec::new(),
            writes_at: Vec::new(),
            applied_before: 0,
            established_final: 0,
            gross_cut: 0,
            peak: Default::default(),
            supply_peak: Default::default(),
            bound_at: Vec::new(),
            refused_unbacked: 0,
        }
    }
}

impl Strategy for SybilFarm {
    fn name(&self) -> &'static str {
        "sybil-farm"
    }

    fn act(&mut self, view: &AgentView, _rng: &mut Rng) -> Vec<Intent> {
        let dust = view.st.params.dust_minor();
        let unit = view.bond_unit_minor().max(1);
        let mut out = Vec::new();
        let seats = view.seats().to_vec();

        // Last tick's wash rows, honoured. The settlement is what writes the
        // stake, and the stake is what makes the child established.
        for &parent in &seats {
            for cid in view.claims_of(parent) {
                let Some(c) = view.st.contracts.get(&cid) else { continue };
                if c.outstanding <= dust || !view.controls(c.debtor) {
                    continue;
                }
                out.push(Intent::As {
                    member: parent,
                    intent: Box::new(Intent::Settle { contract: cid, amount_minor: c.outstanding }),
                });
            }
        }

        // Every member of the farm seats what its HEADROOM pays for, which is
        // the ledger's own test and not `established`. The two are close and
        // not the same: the allowance turns on establishment, a seat is bonded
        // and never allowance-covered, and `bond_headroom` reads `seed_reach`
        // with no establishment gate at all. A farm that asked the wrong one
        // would be measuring its own strategy.
        for &parent in &seats {
            let headroom = view.bond_headroom_minor(parent);
            let mut affordable = headroom / unit;
            if self.p.careful {
                affordable = affordable.saturating_sub(1);
            }
            for _ in 0..affordable {
                self.next_key += 1;
                out.push(Intent::As {
                    member: parent,
                    intent: Box::new(Intent::Lend {
                        creditor: AgentRef::Member(parent),
                        debtor: AgentRef::Fresh { owner: self.agent, n: self.next_key },
                        amount_minor: self.p.wash_minor,
                        term: view.min_term(),
                        arb: None,
                    }),
                });
            }
            // The write channel, exercised. A `Dud` seats no row, so the
            // allowance may pay for it — which is exactly the budget a seated
            // row comes with.
            if self.p.duds && parent != self.me {
                out.push(Intent::As { member: parent, intent: Box::new(Intent::Dud) });
            }
        }
        out
    }

    /// It trades with itself. What it needs from outside is the one honest
    /// edge the population wrote at genesis, and it takes more if offered.
    fn consents(&mut self, _view: &AgentView, ask: &Ask, _rng: &mut Rng) -> bool {
        crate::intent::is_payment_to_me(ask) || matches!(ask.role, Role::Debtor | Role::Buyer)
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        let seats = view.seats().to_vec();
        self.rows_at.push(seats.len() as u64);
        self.seated_at.push(
            seats
                .iter()
                .filter(|&&m| view.st.members.get(&m).is_some_and(|r| r.seat.is_some()))
                .count() as u64,
        );
        let applied = log.iter().filter(|l| l.outcome == crate::intent::Outcome::Applied).count() as u64;
        self.writes_at.push(applied - self.applied_before.min(applied));
        self.applied_before = applied;
        self.established_final = seats.iter().filter(|&&m| view.established(m)).count() as u64;
        self.gross_cut = view.st.gross_capacity_of_set_minor(&seats);
        self.refused_unbacked = log
            .iter()
            .filter(|l| l.outcome == crate::intent::Outcome::Refused(edet_state::errors::ET_BOND_SEAT_UNBACKED))
            .count() as u64;

        // The peaks the bound is stated over: arcs crossing INTO the set, and
        // the supply arcs of members inside it. An edge internal to the set
        // never crosses its own boundary, which is what makes wash trading
        // arithmetically worthless here rather than merely detectable.
        let inside: std::collections::BTreeSet<usize> = seats.iter().map(|&m| m as usize).collect();
        for (&(c, d), &w) in view.st.edges.iter() {
            if inside.contains(&d) && !inside.contains(&c) {
                let e = self.peak.entry((c, d)).or_insert(0);
                *e = (*e).max(w);
            }
        }
        for (&u, &supply) in view.st.underwriters.iter() {
            if inside.contains(&(u as usize)) {
                let e = self.supply_peak.entry(u).or_insert(0);
                *e = (*e).max(supply);
            }
        }
        let unit = view.bond_unit_minor().max(1);
        let total: u128 = self.peak.values().map(|&v| v as u128).sum::<u128>()
            + self.supply_peak.values().map(|&v| v as u128).sum::<u128>();
        self.bound_at.push((total / unit as u128) as u64);
    }

    fn probe(&self) -> Probe {
        // **The bound the design claims, at every tick**: the rows the farm
        // SEATED are at most `Σ peak / unit` over the arcs reaching its set
        // from outside, supply arcs at their all-time maximum declaration.
        // Counted off the ledger's own record of what bought each row, so a
        // seat the founding ceremony wrote is not this farm's and no arithmetic
        // over a starting count has to stand in for that.
        let mut reached = true;
        let mut worst = (0u64, 0u64, 0usize);
        for (i, &seated) in self.seated_at.iter().enumerate() {
            let bound = self.bound_at.get(i).copied().unwrap_or(0);
            if seated > bound {
                reached = false;
                if seated.saturating_sub(bound) > worst.0.saturating_sub(worst.1) {
                    worst = (seated, bound, i + 1);
                }
            }
        }
        // **The ceiling must be REACHED**, or a probe that only ever watched a
        // farm below its bound would stay green with the mechanism removed.
        let bit = self.refused_unbacked > 0;
        Probe {
            what: "the rows it seats stay inside the cut behind its set, and the ceiling it stops at is that cut",
            reached: reached && bit,
            detail: format!(
                "rows per tick {:?}; of those seated by a trade {:?}; bound per tick {:?}; worst {} against {} at tick {}; {} seats refused for want of reach; {} established; the farm's gross cut as a SET is {} minor",
                self.rows_at, self.seated_at, self.bound_at, worst.0, worst.1, worst.2, self.refused_unbacked, self.established_final, self.gross_cut
            ),
        }
    }
}
