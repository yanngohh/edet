//! **Defaults, then wins the arbitration.** The probe for when an award is
//! minted, and for the panel's median being a statement about a SET rather
//! than about a race.
//!
//! An award is the median over the whole WINDOW, minted by the sweep at its
//! close. Minting at quorum makes the panel a race: colluders attesting the
//! ceiling first take the median of the first `q` attestations and everybody
//! else is refused as late. So the debtor here controls a minority of the
//! panel, attests the ceiling on the first tick it can, and the honest
//! majority attests inside the window and later — and nothing mints, because
//! the median of the whole panel is what the parties consented to.
//!
//! The window has to outlive the term, or the remedy closes before the
//! default it is a remedy for. And the row has to be UNINSURED, which is why
//! this archetype sizes it past its own capacity: an insured default is
//! SUBSTITUTED, and substitution drops the consented panel — the panel D and C
//! agreed on must not be able to mint an award against an underwriter who was
//! never asked. So the arbitration remedy survives exactly the defaults the
//! community did not stand behind, which is also the tier where a creditor has
//! nothing else.
//!
//! **The median is taken over the ATTESTATIONS, not over the panel**, and a
//! quorum below a majority of the panel is what turns that into a lever: an
//! arbiter who does not attest hands the median to whoever did. Two colluders
//! and one honest arbiter on a panel of five with a quorum of three make the
//! median the colluders'. What both parties consent to at acceptance is
//! therefore the quorum as much as the panel — a panel of five with a quorum
//! of three is a panel of three, chosen by whoever answers first.

use std::collections::BTreeSet;

use edet_state::state::State;
use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Outcome, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    /// Strictly greater than `term`, so the window outlives the default.
    pub window_epochs: u64,
    pub term: u64,
    /// How many honest members join the panel beside the colluders.
    pub honest_panel: usize,
    pub quorum: u32,
    pub award_cap_minor: u64,
    pub amount_minor: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            window_epochs: 90,
            term: 30,
            honest_panel: 3,
            quorum: 3,
            award_cap_minor: 50_000,
            amount_minor: 20_000,
        }
    }
}

pub struct LateDefaulter {
    p: Params,
    me: MemberId,
    cohort: Vec<MemberId>,
    rank: usize,
    /// The row the defaulter booked, read off the log once the ledger has
    /// actually booked it — never predicted from `next_contract`, because a
    /// creditor that DECLINES leaves the prediction pointing at somebody
    /// else's row and the probe then measures a stranger.
    row: Option<ContractId>,
    creditor: Option<MemberId>,
    sent: bool,
    booked: bool,
    /// The window closed and the sweep resolved it.
    awarded_early: bool,
    awarded_at_close: bool,
    minted: bool,
    attestations: usize,
    panel: usize,
    /// The median of what the window actually received, capped by the terms
    /// and by the original — the figure the sweep mints against.
    expected_award: u64,
    awarded_amount: u64,
    attested: bool,
}

impl LateDefaulter {
    pub fn new(p: Params, at: &Seating) -> Self {
        LateDefaulter {
            p,
            me: at.seat,
            cohort: at.cohort.clone(),
            rank: at.rank,
            row: None,
            creditor: None,
            sent: false,
            booked: false,
            awarded_early: false,
            awarded_at_close: false,
            minted: false,
            attestations: 0,
            panel: 0,
            expected_award: 0,
            awarded_amount: 0,
            attested: false,
        }
    }

    /// The colluders: every member of this cohort but the debtor.
    fn colluders(&self) -> Vec<MemberId> {
        self.cohort.iter().copied().filter(|&m| m != self.cohort[0]).collect()
    }
}

impl Strategy for LateDefaulter {
    fn name(&self) -> &'static str {
        "late-defaulter"
    }

    fn act(&mut self, view: &AgentView, _rng: &mut Rng) -> Vec<Intent> {
        let me = self.me;
        let _ = self.sent;
        if view.st.members.get(&me).is_none_or(|m| m.status != MemberStatus::Active) {
            return Vec::new();
        }
        // The colluders' only job: attest the ceiling as early as the ledger
        // will take it.
        if self.rank > 0 {
            let mut out = Vec::new();
            for cid in view.panels_of(me) {
                let Some(c) = view.st.contracts.get(&cid) else { continue };
                if c.arb_attestations.contains_key(&me) {
                    continue;
                }
                out.push(Intent::ArbAttest { contract: cid, amount_minor: self.p.award_cap_minor });
            }
            return out;
        }
        // The debtor books once, with a panel it has a minority of, and then
        // never settles. It retries while the creditor keeps declining: a
        // declined ask is the pending-signature pool, not a refusal.
        if self.booked {
            return Vec::new();
        }
        let colluders = self.colluders();
        let honest: Vec<MemberId> = view
            .active()
            .into_iter()
            .filter(|id| !self.cohort.contains(id) && view.established(*id))
            .take(self.p.honest_panel)
            .collect();
        if honest.len() < self.p.honest_panel || colluders.is_empty() {
            return Vec::new();
        }
        let creditor = view
            .active()
            .into_iter()
            .find(|id| !self.cohort.contains(id) && !honest.contains(id) && view.conferrable_minor(*id) > 0);
        let Some(creditor) = creditor else { return Vec::new() };
        let arbiters: BTreeSet<MemberId> = colluders.iter().chain(honest.iter()).copied().collect();
        self.sent = true;
        self.creditor = Some(creditor);
        // Past its own capacity, so the community does not insure it and the
        // panel survives the default. See the module note.
        let amount_minor = view.capacity_raw_minor(me).saturating_add(self.p.amount_minor);
        vec![Intent::Lend {
            creditor: AgentRef::Member(creditor),
            debtor: AgentRef::Member(me),
            amount_minor,
            term: self.p.term.max(view.min_term()),
            arb: Some(ArbTermsWire {
                arbiters,
                quorum: self.p.quorum,
                window_epochs: self.p.window_epochs,
                award_cap: State::from_minor(self.p.award_cap_minor),
            }),
        }]
    }

    fn consents(&mut self, _view: &AgentView, ask: &Ask, _rng: &mut Rng) -> bool {
        if self.cohort.contains(&ask.from) {
            return true;
        }
        crate::intent::is_payment_to_me(ask) || matches!(ask.role, Role::Debtor | Role::Buyer)
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        if self.rank > 0 {
            self.attested |= log
                .iter()
                .any(|l| matches!(l.intent, Intent::ArbAttest { .. }) && l.outcome == Outcome::Applied);
            return;
        }
        if !self.booked {
            if let Some((cid, cr)) = log.iter().rev().find_map(|l| match (&l.intent, &l.outcome) {
                (Intent::Lend { creditor, arb: Some(_), .. }, Outcome::Applied) => l.contract.map(|c| (c, *creditor)),
                _ => None,
            }) {
                self.booked = true;
                self.row = Some(cid);
                if let AgentRef::Member(id) = cr {
                    self.creditor = Some(id);
                }
            }
        }
        let Some(row) = self.row else { return };
        let Some(c) = view.st.contracts.get(&row) else { return };
        let Some(terms) = c.arb.as_ref() else { return };
        self.attestations = c.arb_attestations.len();
        self.panel = terms.arbiters.len();
        // What the sweep will mint against: the median of the attestations the
        // window RECEIVED, capped by the terms and by the original. An even
        // count takes the mean of the middle pair, rounded down — down,
        // because an award is minted against a party who consented to the
        // panel and not to the figure.
        let mut vals: Vec<u64> = c.arb_attestations.values().copied().collect();
        vals.sort_unstable();
        let n = vals.len();
        self.expected_award = if (n as u32) < terms.quorum {
            0
        } else {
            let median =
                if n % 2 == 1 { vals[n / 2] } else { ((vals[n / 2 - 1] as u128 + vals[n / 2] as u128) / 2) as u64 };
            median.min(terms.award_cap).min(c.original)
        };
        let closes = c.created_epoch.saturating_add(terms.window_epochs);
        if view.epoch() <= closes {
            if c.arb_awarded {
                self.awarded_early = true;
            }
        } else if c.arb_awarded {
            self.awarded_at_close = true;
        }
        // **An award runs from the original CREDITOR to the original
        // DEBTOR** — the buyer's remedy for non-delivery, not the seller's
        // remedy for non-payment.
        if let Some(cr) = self.creditor {
            self.awarded_amount = view
                .st
                .contracts
                .values()
                .filter(|x| x.creditor == self.me && x.debtor == cr && x.id != row)
                .map(|x| x.original)
                .sum();
            self.minted = self.awarded_amount > 0;
        }
    }

    fn probe(&self) -> Probe {
        if self.rank > 0 {
            return Probe {
                what: "a colluding panel member attests the ceiling and the ledger takes it",
                reached: self.attested,
                detail: format!("attested: {}", self.attested),
            };
        }
        Probe {
            what: "the award waits for the window to close, and then mints the median of what the window received",
            reached: self.booked
                && !self.awarded_early
                && self.awarded_at_close
                && self.awarded_amount == self.expected_award,
            detail: format!(
                "booked: {}, awarded before the close: {}, awarded at the close: {}, {} of a panel of {} attested, the median of them capped is {}, and {} was minted",
                self.booked,
                self.awarded_early,
                self.awarded_at_close,
                self.attestations,
                self.panel,
                self.expected_award,
                self.awarded_amount
            ),
        }
    }
}
