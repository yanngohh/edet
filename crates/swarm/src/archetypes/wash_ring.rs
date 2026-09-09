//! **A ring of accounts backing each other, reached by exactly one honest
//! edge.** The probe for the two rules that keep manufactured standing from
//! becoming manufactured insurance.
//!
//! The ring writes real stakes — every one of them earned by a real
//! settlement, because there is no other way — and then tries to turn them
//! into a declared supply. It cannot: `DeclareSupply` may only ever LOWER a
//! declaration, so the ring's insured credit stays bounded by the cut through
//! the one honest edge that reaches it, however much the members stake on each
//! other. A ring nobody reaches at all is scenery: no signer has headroom, so
//! it writes nothing.

use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Outcome, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    /// What each member stakes on the next, per round.
    pub amount_minor: u64,
    /// The term the wash rows run for.
    pub term: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params { amount_minor: 20_000, term: 30 }
    }
}

pub struct WashRing {
    p: Params,
    me: MemberId,
    peers: Vec<MemberId>,
    rank: usize,
    /// Every `Declare` above the current declaration must come back
    /// `ET-UWR-001`. One that did not is the hollow-insurance construction.
    declares_refused: u64,
    declares_admitted: u64,
    /// The set bound, checked by the ring's first member so it costs one cut a
    /// tick rather than one per member.
    set_bound_held: bool,
    worst: (u64, u64),
    headroom_within_reach: bool,
}

impl WashRing {
    pub fn new(p: Params, at: &Seating) -> Self {
        WashRing {
            p,
            me: at.seat,
            peers: at.cohort.clone(),
            rank: at.rank,
            declares_refused: 0,
            declares_admitted: 0,
            set_bound_held: true,
            worst: (0, 0),
            headroom_within_reach: true,
        }
    }

    fn next_peer(&self) -> Option<MemberId> {
        if self.peers.len() < 2 {
            return None;
        }
        Some(self.peers[(self.rank + 1) % self.peers.len()])
    }
}

impl Strategy for WashRing {
    fn name(&self) -> &'static str {
        "wash-ring"
    }

    fn act(&mut self, view: &AgentView, _rng: &mut Rng) -> Vec<Intent> {
        let me = self.me;
        if view.st.members.get(&me).is_none_or(|m| m.status != MemberStatus::Active) {
            return Vec::new();
        }
        let dust = view.st.params.dust_minor();
        let mut out = Vec::new();

        // **It pays everything, and that is the point.** The ring's wash rows
        // are honoured at once — a settlement between two ring members is a
        // real discharge, and what it writes is a real stake — and what it
        // borrowed from outside is honoured on its maturity, so nothing here
        // is a default. A ring that stopped paying would shut its own write
        // budget inside an epoch and the probe would be measuring that.
        for cid in view.debts_of(me) {
            let Some(c) = view.st.contracts.get(&cid) else { continue };
            if c.outstanding <= dust {
                continue;
            }
            let inside = self.peers.contains(&c.creditor);
            if !inside && c.maturity_epoch > view.epoch() {
                continue;
            }
            out.push(if c.status == ContractStatus::Expired {
                Intent::Cure { contract: cid, amount_minor: c.outstanding }
            } else {
                Intent::Settle { contract: cid, amount_minor: c.outstanding }
            });
        }

        // Back the next member of the ring.
        if let Some(next) = self.next_peer() {
            out.push(Intent::Lend {
                creditor: AgentRef::Member(me),
                debtor: AgentRef::Member(next),
                amount_minor: self.p.amount_minor,
                term: self.p.term.max(view.min_term()),
                arb: None,
            });
        }

        // **Turn the manufactured standing into a supply.** The one thing the
        // ring is for, and the thing that is refused: a declaration rises
        // through a ceremony and through nothing else.
        //
        // Measured against the PRISTINE cut rather than the residual one,
        // because that is the quantity the construction is about — what the
        // community conferred, not what is left after this member's own
        // borrowing. A ring reading the residual declares once and stops the
        // moment it draws anything, which measures its own borrowing.
        let gross = view.st.gross_capacity_of_set_minor(&[me]);
        let declared = view.st.underwriters.get(&me).copied().unwrap_or(0);
        if gross > declared {
            out.push(Intent::Declare { supply_minor: gross });
        }

        // Borrow insured from outside the ring, up to what the community will
        // carry — the whole point of the standing it just manufactured.
        let want = view.capacity_minor(me);
        if want > dust {
            let outsiders: Vec<MemberId> = view
                .active()
                .into_iter()
                .filter(|id| !self.peers.contains(id) && *id != me)
                .collect();
            if let Some(&other) = outsiders.first() {
                out.push(Intent::Lend {
                    creditor: AgentRef::Member(other),
                    debtor: AgentRef::Member(me),
                    amount_minor: want,
                    term: self.p.term.max(view.min_term()),
                    arb: None,
                });
            }
        }
        out
    }

    /// Inside the ring, everybody signs everything. Outside it, it takes
    /// credit and extends none.
    fn consents(&mut self, _view: &AgentView, ask: &Ask, _rng: &mut Rng) -> bool {
        if self.peers.contains(&ask.from) {
            return true;
        }
        crate::intent::is_payment_to_me(ask) || matches!(ask.role, Role::Debtor | Role::Buyer)
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        self.declares_refused = 0;
        self.declares_admitted = 0;
        for l in log {
            if !matches!(l.intent, Intent::Declare { .. }) {
                continue;
            }
            match l.outcome {
                Outcome::Refused(edet_state::errors::ET_UWR_ABOVE_CAPACITY) => self.declares_refused += 1,
                Outcome::Applied => self.declares_admitted += 1,
                _ => {}
            }
        }
        // **`bond_headroom` reads `seed_reach`, not `conferrable`.** A
        // declared supply is a promise on promises, and a chain of them would
        // double the write channel per accomplice.
        for &m in &self.peers {
            if view.bond_headroom_minor(m) > view.seed_reach_minor(m) {
                self.headroom_within_reach = false;
            }
        }
        // The set quantifier, once per tick. Insured credit outstanding to the
        // ring AS A SET against the gross cut of that set — which is bounded
        // by the one honest edge, whatever the members staked on each other.
        if self.rank == 0 {
            let drawn: u64 = view
                .st
                .contracts
                .values()
                .filter(|c| c.insured && matches!(c.status, ContractStatus::Active | ContractStatus::Expired))
                .filter(|c| self.peers.contains(&c.debtor))
                .map(|c| c.outstanding)
                .sum();
            let cut = view.st.gross_capacity_of_set_minor(&self.peers);
            if drawn > cut {
                self.set_bound_held = false;
            }
            if drawn > self.worst.0 {
                self.worst = (drawn, cut);
            }
        }
    }

    fn probe(&self) -> Probe {
        let reached = self.declares_admitted == 0
            && self.declares_refused > 0
            && self.set_bound_held
            && self.headroom_within_reach;
        Probe {
            what: "a raise is refused ET-UWR-001, and the ring's insured credit as a SET stays inside its own cut",
            reached,
            detail: format!(
                "{} raises refused and {} admitted; peak insured to the set {} against a gross cut of {}; headroom within seed reach: {}",
                self.declares_refused, self.declares_admitted, self.worst.0, self.worst.1, self.headroom_within_reach
            ),
        }
    }
}
