//! **Borrows from whoever consents and never pays.** The probe for what a
//! default costs the community and what it costs the defaulter.
//!
//! The three things it has to show are the three the design claims about a
//! default: the row expires through the SWEEP rather than because somebody
//! called it, the flow it committed is not released by the expiry, and the
//! creditor of an insured row is made whole by a substitution leg an
//! underwriter now owes.

use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    pub rate: f64,
    pub mean_amount_minor: u64,
    pub term: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params { rate: 0.6, mean_amount_minor: 20_000, term: 30 }
    }
}

pub struct Deadbeat {
    p: Params,
    me: MemberId,
    /// Rows of this member's that the sweep expired.
    expired: u64,
    /// Whether the hold on a row ever FELL as that row expired. A default
    /// releases nothing — not the flow, not the reservation — so this must
    /// stay false.
    ///
    /// Read off the ROW rather than off `committed_total`, which every other
    /// member's settlement moves in the same tick: a global figure cannot
    /// answer a question about one obligation.
    hold_released: bool,
    /// The hold, the creditor and the insurance each row carried the tick
    /// BEFORE it fell due. Substitution rewrites the creditor inside
    /// `mark_expired`, so a reading taken after the sweep is about the
    /// underwriter that inherited the claim rather than about the member who
    /// lost it.
    snap: std::collections::BTreeMap<ContractId, (u64, MemberId, bool)>,
    seen: std::collections::BTreeSet<ContractId>,
    open_default: u64,
    allowance_after: u32,
    substitution_leg: bool,
}

impl Deadbeat {
    pub fn new(p: Params, at: &Seating) -> Self {
        Deadbeat {
            p,
            me: at.seat,
            expired: 0,
            hold_released: false,
            snap: Default::default(),
            seen: Default::default(),
            open_default: 0,
            allowance_after: 0,
            substitution_leg: false,
        }
    }
}

impl Strategy for Deadbeat {
    fn name(&self) -> &'static str {
        "deadbeat"
    }

    fn act(&mut self, view: &AgentView, rng: &mut Rng) -> Vec<Intent> {
        let me = self.me;
        if view.st.members.get(&me).is_none_or(|m| m.status != MemberStatus::Active) {
            return Vec::new();
        }
        // It never emits `MarkExpired`: the probe is that the SWEEP expires
        // the row, so calling the crank would measure the crank.
        if !rng.chance(self.p.rate) {
            return Vec::new();
        }
        let peers = view.counterparties();
        let Some(&other) = rng.pick(&peers) else { return Vec::new() };
        vec![Intent::Lend {
            creditor: AgentRef::Member(other),
            debtor: AgentRef::Member(me),
            amount_minor: rng.amount_minor(self.p.mean_amount_minor),
            term: self.p.term.max(view.min_term()),
            arb: None,
        }]
    }

    /// It lends to nobody and it settles nothing, so the only thing anybody
    /// can ask it for is to take on more debt.
    fn consents(&mut self, _view: &AgentView, ask: &Ask, _rng: &mut Rng) -> bool {
        crate::intent::is_payment_to_me(ask) || matches!(ask.role, Role::Debtor | Role::Buyer)
    }

    fn observe(&mut self, view: &AgentView, _log: &[crate::intent::AgentLog]) {
        for cid in view.debts_of(self.me) {
            let Some(c) = view.st.contracts.get(&cid) else { continue };
            let held = c.held.amount();
            if c.status == ContractStatus::Expired && self.seen.insert(cid) {
                self.expired += 1;
                if let Some(&(before, creditor, insured)) = self.snap.get(&cid) {
                    // **A default releases nothing**, so the hold this row
                    // carried the tick before it fell due is the hold it
                    // carries now.
                    if held < before {
                        self.hold_released = true;
                    }
                    // **The creditor of an insured row is made whole by a leg
                    // an underwriter now owes them**: uninsured, live, running
                    // from an underwriter to the member who lost it.
                    if insured {
                        self.substitution_leg |= view.st.contracts.values().any(|x| {
                            !x.insured
                                && x.creditor == creditor
                                && x.debtor != self.me
                                && view.st.underwriters.contains_key(&x.debtor)
                                && matches!(x.status, ContractStatus::Active | ContractStatus::Expired)
                        });
                    }
                }
            }
            self.snap.insert(cid, (held, c.creditor, c.insured));
        }
        if let Some(m) = view.st.members.get(&self.me) {
            self.open_default = m.rep.open_default;
        }
        self.allowance_after = edet_state::bond::free_remaining(view.st, self.me);
    }

    fn probe(&self) -> Probe {
        Probe {
            what: "a row it owes reaches Expired through the sweep, the hold is kept, and its own budget closes",
            reached: self.expired > 0 && !self.hold_released && self.open_default > 0 && self.allowance_after == 0,
            detail: format!(
                "{} of its rows expired, committed flow released at an expiry: {}, open default {}, allowance {}, substitution leg: {}",
                self.expired, self.hold_released, self.open_default, self.allowance_after, self.substitution_leg
            ),
        }
    }
}
