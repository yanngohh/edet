//! **A block of underwriters voting together, with accomplices who hold no
//! supply.** The probe for what governance weight IS and for the two classes
//! of proposal.
//!
//! Weight is a share of the EXTERNAL seed, every proposal kind — a cut says
//! who the seed reached, not who put it up — so an accomplice with no declared
//! supply is refused outright rather than silently discounted: a recorded
//! assent that did not count would be a vote the ledger shows and does not
//! use.
//!
//! And the higher bar is keyed on the TARGET's voting power rather than on the
//! proposal's kind. A change to who orders the ledger takes two thirds; a
//! parameter is recoverable through the door it moved through and takes a
//! half. Suspension is in the higher class exactly when its target holds
//! power — read off state, because a rule keyed on the kind leaves the same
//! door open one name over.

use edet_state::types::*;

use super::Seating;
use crate::intent::{Ask, Intent, Outcome, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    pub target: ParamKey,
    pub value: f64,
    /// Whether the coalition also goes for the order: a consensus key, voting
    /// power for the leader, and the suspension of a sitting validator.
    pub also_the_order: bool,
    /// Whether the member it suspends at the LOWER bar is somebody outside the
    /// coalition.
    ///
    /// Off by default, so the probe for "the threshold reads the target's
    /// voting power" costs the population nothing: a suspended honest trader
    /// stops originating, and every figure the run reports about honest
    /// traders is then about a member under sanction.
    pub suspend_outsider: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { target: ParamKey::RiskK, value: 1.5, also_the_order: true, suspend_outsider: false }
    }
}

/// One proposal the coalition put up, and the class it belongs to.
#[derive(Clone, Debug)]
struct Filed {
    id: ProposalId,
    what: &'static str,
    /// Whether the ledger should carry it at the coalition's weight.
    expected: bool,
}

pub struct Coalition {
    p: Params,
    me: MemberId,
    peers: Vec<MemberId>,
    rank: usize,
    filed: Vec<Filed>,
    /// Set once the leader has put its slate up.
    proposed: bool,
    assented: Vec<ProposalId>,
    /// An accomplice with no external supply is refused `ET-GOV-007`.
    no_mandate: bool,
    mandate_expected: bool,
    verdicts: Vec<(&'static str, bool, bool)>,
}

impl Coalition {
    pub fn new(p: Params, at: &Seating) -> Self {
        Coalition {
            p,
            me: at.seat,
            peers: at.cohort.clone(),
            rank: at.rank,
            filed: Vec::new(),
            proposed: false,
            assented: Vec::new(),
            no_mandate: false,
            mandate_expected: false,
            verdicts: Vec::new(),
        }
    }

    /// The coalition's share of the external seed, which is what every
    /// threshold is measured against.
    fn share(&self, view: &AgentView) -> f64 {
        let seed = view.st.external_seed();
        if seed <= 0.0 {
            return 0.0;
        }
        let mine: f64 = self
            .peers
            .iter()
            .filter_map(|m| view.st.underwriters.get(m))
            .map(|&s| edet_state::state::State::from_minor(s))
            .sum();
        mine / seed
    }

    /// The member it suspends at the LOWER bar: one of its own accomplices,
    /// or somebody outside when the preset asks for that. Never a validator,
    /// which is the other proposal's job.
    fn low_bar_target(&self, view: &AgentView) -> Option<MemberId> {
        let free_of_power = |id: &MemberId| !view.st.validators.contains_key(id) && *id != self.me;
        if self.p.suspend_outsider {
            return view
                .active()
                .into_iter()
                .find(|id| !self.peers.contains(id) && free_of_power(id));
        }
        view.active()
            .into_iter()
            .find(|id| self.peers.contains(id) && free_of_power(id))
    }

    /// A sitting validator that is not the leader — the suspension whose
    /// threshold is the higher one.
    fn a_validator(&self, view: &AgentView) -> Option<MemberId> {
        view.st.validators.keys().copied().find(|id| *id != self.me)
    }
}

impl Strategy for Coalition {
    fn name(&self) -> &'static str {
        "coalition"
    }

    fn act(&mut self, view: &AgentView, _rng: &mut Rng) -> Vec<Intent> {
        let mut out = Vec::new();
        if view.st.members.get(&self.me).is_none_or(|m| m.status != MemberStatus::Active) {
            return out;
        }
        // The leader files the slate once, on the second tick, so the
        // population has settled a genesis edge behind it first.
        if self.rank == 0 && !self.proposed && view.tick >= 2 {
            self.proposed = true;
            let share = self.share(view);
            let half = view.st.params.theta_adopt;
            let two_thirds = view.st.params.theta_adopt_validator;
            let mut next = view.st.next_proposal;

            out.push(Intent::Propose(ProposalKind::ParamChange { key: self.p.target, value: self.p.value }));
            self.filed
                .push(Filed { id: next, what: "a parameter", expected: share >= half });
            next += 1;

            // Suspension of a member holding no power: the LOWER bar, and the
            // reason the rule cannot be keyed on the kind.
            if let Some(o) = self.low_bar_target(view) {
                out.push(Intent::Propose(ProposalKind::Suspend { member: o }));
                self.filed
                    .push(Filed { id: next, what: "suspending a member with no power", expected: share >= half });
                next += 1;
            }
            if self.p.also_the_order {
                out.push(Intent::SetConsensusKey(Some(crate::keys::consensus_key(self.me as usize))));
                out.push(Intent::Propose(ProposalKind::ValidatorPower { member: self.me, power: 1 }));
                self.filed
                    .push(Filed { id: next, what: "a seat in the validator set", expected: share >= two_thirds });
                next += 1;
                if let Some(v) = self.a_validator(view) {
                    out.push(Intent::Propose(ProposalKind::Suspend { member: v }));
                    self.filed.push(Filed {
                        id: next,
                        what: "suspending a sitting validator",
                        expected: share >= two_thirds,
                    });
                }
            }
            return out;
        }
        // Everybody assents to everything the coalition filed, accomplices
        // included — which is the point: an assent from outside the electorate
        // is refused rather than discounted.
        for (&id, p) in &view.st.proposals {
            if p.enacted || !self.peers.contains(&p.author) || p.assents.contains(&self.me) {
                continue;
            }
            // The author of a seed amendment is its beneficiary, so their own
            // assent is a vote on their own supply. No amendment is filed
            // here, but the rule is why an assent is never automatic.
            out.push(Intent::Assent(id));
        }
        out
    }

    fn consents(&mut self, _view: &AgentView, ask: &Ask, _rng: &mut Rng) -> bool {
        if self.peers.contains(&ask.from) {
            return true;
        }
        crate::intent::is_payment_to_me(ask) || matches!(ask.role, Role::Debtor | Role::Buyer)
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        self.mandate_expected = !view.st.underwriters.contains_key(&self.me);
        for l in log {
            if let (Intent::Assent(id), outcome) = (&l.intent, &l.outcome) {
                match outcome {
                    Outcome::Refused(edet_state::errors::ET_GOV_NO_MANDATE) => self.no_mandate = true,
                    Outcome::Applied if !self.assented.contains(id) => self.assented.push(*id),
                    _ => {}
                }
            }
        }
        self.verdicts = self
            .filed
            .iter()
            .map(|f| (f.what, f.expected, view.st.proposals.get(&f.id).is_some_and(|p| p.enacted)))
            .collect();
    }

    fn probe(&self) -> Probe {
        if self.rank == 0 {
            let held = !self.verdicts.is_empty() && self.verdicts.iter().all(|&(_, want, got)| want == got);
            return Probe {
                what: "each proposal enacts exactly at the threshold its TARGET puts it in",
                reached: held,
                detail: self
                    .verdicts
                    .iter()
                    .map(|(w, want, got)| format!("{w}: expected {want}, enacted {got}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            };
        }
        // An accomplice the seed never reached: refused, not discounted.
        let reached =
            if self.mandate_expected { self.no_mandate } else { !self.assented.is_empty() && !self.no_mandate };
        Probe {
            what: "an assent counts exactly when the member holds external supply, and is refused otherwise",
            reached,
            detail: format!(
                "holds no supply: {}, refused ET-GOV-007: {}, assents recorded: {}",
                self.mandate_expected,
                self.no_mandate,
                self.assented.len()
            ),
        }
    }
}
