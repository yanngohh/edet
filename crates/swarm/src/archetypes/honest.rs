//! **The control.** Trades, settles what it owes on its maturity, and applies
//! the wallet's own acceptance rule to everybody who asks it for credit.
//!
//! Every other archetype's figure is a ratio against a population of these,
//! aged identically — so what this one does is the denominator of the whole
//! crate, and a behaviour here that is not the ledger's own reading of a
//! quantity is an error in every figure downstream.

use std::collections::BTreeSet;

use edet_kernel::constants as k;
use edet_state::state::State;
use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Role};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    /// Probability of opening a trade in a tick.
    pub rate: f64,
    pub mean_amount_minor: u64,
    /// `(term_epochs, cumulative_percent)`, summing to 100. The shortest may
    /// not go below `min_maturity_epochs`, which no `ParamKey` reaches.
    pub terms: Vec<(u64, u64)>,
    /// Share of trades taken as the creditor rather than the debtor.
    pub lend_share: f64,
    /// Share of discharges paid by SELLING to the creditor, which is how the
    /// wallet pays and what reaches `cascade::net_mutual`.
    pub net_by_sale: f64,
    /// What the wallet does with a cold start: it holds it for a human, and
    /// this is how often the human says yes.
    pub carry_newcomer: f64,
    pub arb_share: f64,
    /// **How often this member brings a NEWCOMER in** — lends to a key that
    /// has no row yet, seating it. Zero by default, because most presets are
    /// about a community that already exists and a newcomer stream would
    /// change every one of their scenes.
    ///
    /// It is the honest counterpart of the farm's wash, and the two are the
    /// same reading per member: both hold one bond unit of the seater's reach
    /// for as long as the row exists. What separates them is not the ledger's
    /// arithmetic but what the row goes on to do, so a preset that turns this
    /// up is measuring whether the seat ceiling refuses an ORDINARY community
    /// before it refuses a farm.
    pub greet: f64,
    /// What a greeting trade carries.
    pub greet_amount_minor: u64,
    /// **A deliberate malformation, at a small rate, on purpose.** A
    /// population whose traffic is junk-free never produces
    /// `refused["ET-CTR-005"]`, so dropping the maturity floor from `accept`
    /// changes no summary and the corpus is not the mutation detector it
    /// claims to be. This is the rate at which the honest trader draws a term
    /// one epoch below the floor — a fat finger, and the one refusal code a
    /// working community really does see.
    pub junk_rate: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            rate: 0.35,
            mean_amount_minor: 10_000,
            terms: vec![(30, 60), (60, 90), (90, 100)],
            lend_share: 0.5,
            net_by_sale: 0.15,
            carry_newcomer: 0.5,
            arb_share: 0.05,
            greet: 0.0,
            greet_amount_minor: 5_000,
            junk_rate: 0.01,
        }
    }
}

pub struct Honest {
    p: Params,
    me: MemberId,
    agent: usize,
    /// Keys greeted so far, so two greetings in one run name two keys.
    greeted: u32,
    /// Rows this agent owed that the sweep expired, counted once each — a row
    /// stays `Expired` for the rest of its life, so a per-tick count would
    /// report the same default as many defaults.
    expired_owed: std::collections::BTreeSet<ContractId>,
    /// Whether it ever stood as creditor on credit the community insured.
    insured_as_creditor: bool,
    settled: u64,
}

impl Honest {
    pub fn new(p: Params, at: &Seating) -> Self {
        Honest {
            p,
            me: at.seat,
            agent: at.agent,
            greeted: 0,
            expired_owed: Default::default(),
            insured_as_creditor: false,
            settled: 0,
        }
    }

    fn term(&self, rng: &mut Rng) -> u64 {
        let roll = rng.below(100);
        self.p
            .terms
            .iter()
            .find(|&&(_, cum)| roll < cum)
            .map(|&(t, _)| t)
            .unwrap_or(k::MIN_MATURITY_EPOCHS)
    }

    /// A panel of members the seed reaches, neither of them a party. An
    /// arbiter is billed on their own standing, so a panel of free keys
    /// attests nothing.
    fn panel(&self, view: &AgentView, other: MemberId, amount_minor: u64) -> Option<ArbTermsWire> {
        let arbiters: BTreeSet<MemberId> = view
            .active()
            .into_iter()
            .filter(|&id| id != self.me && id != other && view.established(id))
            .take(3)
            .collect();
        if arbiters.len() < 2 {
            return None;
        }
        Some(ArbTermsWire { quorum: 2, window_epochs: 120, award_cap: State::from_minor(amount_minor), arbiters })
    }
}

impl Strategy for Honest {
    fn name(&self) -> &'static str {
        "honest"
    }

    fn act(&mut self, view: &AgentView, rng: &mut Rng) -> Vec<Intent> {
        let me = self.me;
        let Some(m) = view.st.members.get(&me) else { return Vec::new() };
        if m.status == MemberStatus::Exited {
            return Vec::new();
        }
        // **A non-Active member may still discharge**, and must: settle, cure,
        // transfer out and exit are priced at zero for exactly that reason,
        // and a strategy that stopped paying under sanction would turn a
        // suspension into the default it is not.
        let originating = m.status == MemberStatus::Active;
        let dust = view.st.params.dust_minor();
        let mut out = Vec::new();

        // Everything due, discharged in full. `Cure` for a row the sweep
        // already expired — the recovery path is priced at zero and a member
        // inside their own default must always be able to take it.
        for cid in view.debts_of(me) {
            let Some(c) = view.st.contracts.get(&cid) else { continue };
            if c.outstanding <= dust || c.maturity_epoch > view.epoch() {
                continue;
            }
            if c.status == ContractStatus::Expired {
                out.push(Intent::Cure { contract: cid, amount_minor: c.outstanding });
            } else if rng.chance(self.p.net_by_sale) {
                out.push(Intent::Sell {
                    seller: AgentRef::Member(me),
                    buyer: AgentRef::Member(c.creditor),
                    amount_minor: c.outstanding,
                    term: self.term(rng),
                });
            } else {
                out.push(Intent::Settle { contract: cid, amount_minor: c.outstanding });
            }
        }

        // **An arbiter's duty, taken on its own initiative.** The amount is
        // this member's own judgement, so nobody can ask for it: an ask
        // carries the EMITTER's amount, which is the collusion the
        // late-defaulter archetype exists to measure. An honest panel member
        // with no evidence of non-delivery attests nothing owed.
        for cid in view.panels_of(me) {
            let Some(c) = view.st.contracts.get(&cid) else { continue };
            if c.arb_attestations.contains_key(&me) || c.status != ContractStatus::Expired {
                continue;
            }
            out.push(Intent::ArbAttest { contract: cid, amount_minor: 0 });
        }

        // One trade, with a counterparty drawn uniformly from the Active
        // members this agent does not control. Origination is the half
        // suspension revokes.
        if originating && rng.chance(self.p.rate) {
            let peers = view.counterparties();
            if let Some(&other) = rng.pick(&peers) {
                let amount = rng.amount_minor(self.p.mean_amount_minor);
                let term = self.term(rng);
                let arb = if rng.chance(self.p.arb_share) { self.panel(view, other, amount) } else { None };
                let (creditor, debtor) = if rng.chance(self.p.lend_share) { (me, other) } else { (other, me) };
                out.push(Intent::Lend {
                    creditor: AgentRef::Member(creditor),
                    debtor: AgentRef::Member(debtor),
                    amount_minor: amount,
                    term,
                    arb,
                });
            }
        }

        // **A newcomer, brought in.** The same transition a farm uses and the
        // same price — one bond unit of this member's reach, held for as long
        // as the row exists — asked of an ordinary community, so that a
        // ceiling which refused honest onboarding below its own limit would
        // show up here rather than nowhere.
        if originating && rng.chance(self.p.greet) {
            self.greeted += 1;
            out.push(Intent::Lend {
                creditor: AgentRef::Member(me),
                debtor: AgentRef::Fresh { owner: self.agent, n: self.greeted },
                amount_minor: self.p.greet_amount_minor,
                term: self.term(rng),
                arb: None,
            });
        }

        // The fat finger. See `Params::junk_rate`.
        if originating && rng.chance(self.p.junk_rate) {
            let peers = view.counterparties();
            if let Some(&other) = rng.pick(&peers) {
                out.push(Intent::Lend {
                    creditor: AgentRef::Member(me),
                    debtor: AgentRef::Member(other),
                    amount_minor: rng.amount_minor(self.p.mean_amount_minor),
                    term: view.min_term().saturating_sub(1),
                    arb: None,
                });
            }
        }
        out
    }

    fn consents(&mut self, view: &AgentView, ask: &Ask, rng: &mut Rng) -> bool {
        // **The wallet's rule, over the figures a view serves** — the same
        // call `ui/src/lib/pricing.ts` makes, not a second implementation of
        // it. A cold start is HELD for a human, because a fresh key and a
        // fully drawn ceiling are indistinguishable to the score.
        let carry = |view: &AgentView, id: MemberId, rng: &mut Rng| -> bool {
            if view.capacity_minor(id) == 0 {
                return rng.chance(self.p.carry_newcomer);
            }
            let r = view.risk(id);
            if r <= k::WALLET_ACCEPT {
                true
            } else if r >= k::WALLET_REJECT {
                false
            } else {
                rng.chance(self.p.carry_newcomer)
            }
        };
        match ask.role {
            // Discharge in every form: yes, always. A creditor asked to
            // countersign the payment they are owed has nothing to weigh.
            Role::Creditor
                if matches!(ask.intent, Intent::Settle { .. } | Intent::Cure { .. } | Intent::Extend { .. }) =>
            {
                true
            }
            Role::Creditor => match &ask.intent {
                Intent::Lend { debtor, .. } => match crate::intent::party(view.st, debtor) {
                    Party::Member(id) => carry(view, id, rng),
                    // A newcomer whose first trade seats them: the cold start,
                    // held for a human.
                    Party::Key(_) => rng.chance(self.p.carry_newcomer),
                },
                // The successor on a debtor swap, judged exactly as a fresh
                // borrower would be.
                Intent::Transfer { new_debtor, .. } => carry(view, *new_debtor, rng),
                _ => false,
            },
            // Taking on debt it means to pay.
            Role::Debtor | Role::Buyer => true,
            // A sale leaves the seller holding the remainder as a claim, so
            // it is the creditor's question one step over.
            Role::Seller => match &ask.intent {
                Intent::Sell { buyer, .. } => match crate::intent::party(view.st, buyer) {
                    Party::Member(id) => carry(view, id, rng),
                    Party::Key(_) => rng.chance(self.p.carry_newcomer),
                },
                _ => false,
            },
            // **A debtor swap is a debt for nothing.** An honest trader is
            // offered no consideration for taking somebody else's obligation,
            // so it declines; the archetypes that need one control both ends.
            Role::NewDebtor => false,
            // Stopping a theft costs nothing and is what a guardian is for.
            Role::Guardian => true,
        }
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        for cid in view.debts_of(self.me) {
            if view.st.contracts.get(&cid).is_some_and(|c| c.status == ContractStatus::Expired) {
                self.expired_owed.insert(cid);
            }
        }
        for cid in view.claims_of(self.me) {
            if view.st.contracts.get(&cid).is_some_and(|c| c.insured) {
                self.insured_as_creditor = true;
            }
        }
        self.settled = log
            .iter()
            .filter(|l| {
                l.outcome == crate::intent::Outcome::Applied
                    && matches!(l.intent, Intent::Settle { .. } | Intent::Sell { .. })
            })
            .count() as u64;
    }

    /// **What one honest trader can be held to on its own**: it discharged
    /// everything it owed before the sweep could expire it.
    ///
    /// The other half of the sentence — that the community WORKS — is a
    /// population reading and is pinned where it belongs, in the `honest`
    /// corpus entry: zero `expired_rows` and a positive `insured_minor` over
    /// the whole run. Asking one agent for it makes the probe a statement
    /// about who that agent happened to be drawn against, which in a mixed
    /// population is mostly somebody else's archetype.
    fn probe(&self) -> Probe {
        Probe {
            what: "it discharges every obligation it owes before the sweep can expire it",
            reached: self.expired_owed.is_empty(),
            detail: format!(
                "{} discharges applied, {} of its own rows expired, insured as creditor: {}",
                self.settled,
                self.expired_owed.len(),
                self.insured_as_creditor
            ),
        }
    }
}
