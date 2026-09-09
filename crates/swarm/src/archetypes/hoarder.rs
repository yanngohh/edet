//! **Borrows and pays, and never backs anybody.** The failure mode that looks
//! healthy in every aggregate.
//!
//! Its own figures are unremarkable: it settles, its capacity is positive, no
//! invariant notices it. What it is a probe FOR is the population-level
//! reading beside it — `capacity_share_of_non_conferrers`, the share of total
//! member capacity held by members with no out-edge — because a community
//! whose standing has concentrated in accounts that confer none of it has a
//! liquidity problem no per-member quantity reports.

use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    pub rate: f64,
    pub mean_amount_minor: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params { rate: 0.35, mean_amount_minor: 10_000 }
    }
}

pub struct Hoarder {
    p: Params,
    me: MemberId,
    settled: u64,
    out_degree: usize,
    capacity: u64,
}

impl Hoarder {
    pub fn new(p: Params, at: &Seating) -> Self {
        Hoarder { p, me: at.seat, settled: 0, out_degree: 0, capacity: 0 }
    }
}

impl Strategy for Hoarder {
    fn name(&self) -> &'static str {
        "hoarder"
    }

    fn act(&mut self, view: &AgentView, rng: &mut Rng) -> Vec<Intent> {
        let me = self.me;
        if view.st.members.get(&me).is_none_or(|m| m.status != MemberStatus::Active) {
            return Vec::new();
        }
        let dust = view.st.params.dust_minor();
        let mut out = Vec::new();
        for cid in view.debts_of(me) {
            let Some(c) = view.st.contracts.get(&cid) else { continue };
            if c.outstanding <= dust || c.maturity_epoch > view.epoch() {
                continue;
            }
            if c.status == ContractStatus::Expired {
                out.push(Intent::Cure { contract: cid, amount_minor: c.outstanding });
            } else {
                out.push(Intent::Settle { contract: cid, amount_minor: c.outstanding });
            }
        }
        if rng.chance(self.p.rate) {
            let peers = view.counterparties();
            if let Some(&other) = rng.pick(&peers) {
                out.push(Intent::Lend {
                    creditor: AgentRef::Member(other),
                    debtor: AgentRef::Member(me),
                    amount_minor: rng.amount_minor(self.p.mean_amount_minor),
                    term: view.min_term(),
                    arb: None,
                });
            }
        }
        out
    }

    /// It takes credit and it declines to extend any. A settlement it is owed
    /// never arises, because it is never a creditor.
    fn consents(&mut self, _view: &AgentView, ask: &Ask, _rng: &mut Rng) -> bool {
        crate::intent::is_payment_to_me(ask) || matches!(ask.role, Role::Debtor | Role::Buyer)
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        self.settled = log
            .iter()
            .filter(|l| l.outcome == crate::intent::Outcome::Applied && matches!(l.intent, Intent::Settle { .. }))
            .count() as u64;
        self.out_degree = view.st.edges.keys().filter(|&&(c, _)| c as MemberId == self.me).count();
        // The PRISTINE cut: what the community put behind it, gross of its own
        // borrowing. The residual reading is zero for a hoarder that has drawn
        // its whole ceiling, which is the case the probe is most about.
        self.capacity = view.st.gross_capacity_of_set_minor(&[self.me]);
    }

    fn probe(&self) -> Probe {
        Probe {
            what: "it settles, the community has put standing behind it, and it has conferred none of it",
            reached: self.settled > 0 && self.out_degree == 0 && self.capacity > 0,
            detail: format!(
                "{} settlements, out-degree {}, gross cut {} minor",
                self.settled, self.out_degree, self.capacity
            ),
        }
    }
}
