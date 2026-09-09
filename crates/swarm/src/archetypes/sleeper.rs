//! **Honest for `E` ticks, then not.** The interesting one, because it is the
//! archetype that tests a claim about TIME with a mechanism that is about
//! AMOUNT.
//!
//! An underwriter's failure is bounded in amount, never in time: committed
//! flow is released by the defaulter repaying and by nothing else, so an agent
//! that behaves for a year and then defects extracts exactly the same cut it
//! could have extracted on its first tick. A timer anywhere in that path —
//! flow released at an expiry, decay reaching `committed` — would let the same
//! agent absorb a loss, wait, and insure the same amount again, and the loss
//! would then grow with `E`. That is why the corpus runs this family across
//! four values of `E` rather than one.

use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Outcome, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// It borrows the most the ledger will insure and never settles again.
    Debtor,
    /// It stands behind others, then stops honouring the legs its own
    /// defaults hand it.
    Underwriter,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub honest_ticks: u64,
    pub mode: Mode,
    pub honest: super::honest::Params,
}

impl Default for Params {
    fn default() -> Self {
        Params { honest_ticks: 30, mode: Mode::Debtor, honest: Default::default() }
    }
}

pub struct Sleeper {
    p: Params,
    me: MemberId,
    honest: super::honest::Honest,
    defected: bool,
    /// The gross cut it stood in front of at the tick it defected.
    cut_at_defection: u64,
    /// The insured credit it holds, at its peak after defection.
    extracted_insured: u64,
    /// Whatever it discharged after defecting — which must be nothing.
    discharged_after: u64,
    declared: u64,
    owed_expired: u64,
    capacity_after: Vec<u64>,
}

impl Sleeper {
    pub fn new(p: Params, at: &Seating) -> Self {
        let honest = super::honest::Honest::new(p.honest.clone(), at);
        Sleeper {
            p,
            me: at.seat,
            honest,
            defected: false,
            cut_at_defection: 0,
            extracted_insured: 0,
            discharged_after: 0,
            declared: 0,
            owed_expired: 0,
            capacity_after: Vec::new(),
        }
    }
}

impl Strategy for Sleeper {
    fn name(&self) -> &'static str {
        "sleeper"
    }

    fn act(&mut self, view: &AgentView, rng: &mut Rng) -> Vec<Intent> {
        if view.tick < self.p.honest_ticks {
            return self.honest.act(view, rng);
        }
        let me = self.me;
        if view.st.members.get(&me).is_none_or(|m| m.status != MemberStatus::Active) {
            return Vec::new();
        }
        if self.p.mode == Mode::Underwriter {
            // It simply stops. Every substitution leg its own borrowers'
            // defaults hand it goes unhonoured, and so does everything else.
            self.defected = true;
            return Vec::new();
        }
        // The one tick it takes everything. Sized to the residual cut and to
        // each creditor's own conferrable, one after another until the ledger
        // or the creditor refuses.
        if self.defected {
            return Vec::new();
        }
        self.defected = true;
        self.cut_at_defection = view.st.gross_capacity_of_set_minor(&[me]);
        let dust = view.st.params.dust_minor();
        let mut out = Vec::new();
        let mut left = view.capacity_minor(me);
        for other in view.counterparties() {
            if left <= dust {
                break;
            }
            let want = left.min(view.conferrable_minor(other));
            if want <= dust {
                continue;
            }
            out.push(Intent::Lend {
                creditor: AgentRef::Member(other),
                debtor: AgentRef::Member(me),
                amount_minor: want,
                term: view.min_term(),
                arb: None,
            });
            left = left.saturating_sub(want);
        }
        out
    }

    fn consents(&mut self, view: &AgentView, ask: &Ask, rng: &mut Rng) -> bool {
        if !self.defected {
            return self.honest.consents(view, ask, rng);
        }
        // After defection it takes and does not give — but it still takes
        // payment, because refusing that would strand a counterparty inside a
        // default the ledger reports as theirs.
        crate::intent::is_payment_to_me(ask) || matches!(ask.role, Role::Debtor | Role::Buyer)
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        self.declared = view.st.underwriters.get(&self.me).copied().unwrap_or(0);
        if !self.defected {
            self.honest.observe(view, log);
            return;
        }
        if self.cut_at_defection == 0 {
            self.cut_at_defection = view.st.gross_capacity_of_set_minor(&[self.me]);
        }
        let insured: u64 = view
            .debts_of(self.me)
            .into_iter()
            .filter_map(|c| view.st.contracts.get(&c))
            .filter(|c| c.insured)
            .map(|c| c.outstanding)
            .sum();
        self.extracted_insured = self.extracted_insured.max(insured);
        self.owed_expired = view
            .debts_of(self.me)
            .into_iter()
            .filter_map(|c| view.st.contracts.get(&c))
            .filter(|c| c.status == ContractStatus::Expired)
            .map(|c| c.outstanding)
            .sum();
        self.discharged_after = log
            .iter()
            .filter(|l| l.tick >= self.p.honest_ticks && l.outcome == Outcome::Applied)
            .filter(|l| matches!(l.intent, Intent::Settle { .. } | Intent::Cure { .. } | Intent::Sell { .. }))
            .count() as u64;
        self.capacity_after.push(view.capacity_minor(self.me));
    }

    fn probe(&self) -> Probe {
        // **Bounded in AMOUNT, not in time.** What it took is inside the cut
        // that stood in front of it at the moment it defected, whatever `E`
        // was — and nothing it did afterwards gave any of it back, so its own
        // capacity never recovers.
        let within_cut = self.extracted_insured <= self.cut_at_defection;
        let never_recovers = self.capacity_after.windows(2).all(|w| w[1] <= w[0].max(self.cut_at_defection));
        let reached = match self.p.mode {
            Mode::Debtor => self.defected && within_cut && self.discharged_after == 0 && never_recovers,
            // An underwriter's loss is bounded by what it declared, and by
            // nothing about how long it behaved first.
            Mode::Underwriter => self.defected && self.discharged_after == 0,
        };
        Probe {
            what: "what it extracts is bounded by the cut in front of it at defection, whatever E was",
            reached,
            detail: format!(
                "defected at tick {}, insured held {} against a gross cut of {} at defection, {} discharges after it, declared {}, {} expired and owed",
                self.p.honest_ticks,
                self.extracted_insured,
                self.cut_at_defection,
                self.discharged_after,
                self.declared,
                self.owed_expired
            ),
        }
    }
}
