//! **Honest until the trigger, then out.** The probe for what `Exit` does and,
//! more to the point, for what it leaves behind.
//!
//! A member who leaves takes their origination with them and leaves their
//! stake edges where they are: standing is what OTHER people put behind this
//! account, so an exit cannot withdraw it, and the edges decay on the common
//! schedule like everybody else's. `Exit` succeeds at most once, and a
//! Suspended member may still call it — winding down under sanction is the
//! recovery path, not a privilege the sanction removes.

use edet_state::types::*;

use super::Seating;
use crate::intent::{Ask, Intent, Outcome};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

/// What makes it run.
#[derive(Clone, Copy, Debug)]
pub enum Trigger {
    Tick(u64),
    DefaultsSeen(u64),
}

#[derive(Clone, Debug)]
pub struct Params {
    pub trigger: Trigger,
    pub honest: super::honest::Params,
}

impl Default for Params {
    fn default() -> Self {
        Params { trigger: Trigger::Tick(40), honest: Default::default() }
    }
}

pub struct Exiter {
    p: Params,
    me: MemberId,
    honest: super::honest::Honest,
    panicking: bool,
    /// Emissions after the exit applied — each of which must be refused.
    refused_after: u64,
    admitted_after: u64,
    exited: bool,
    edges_after: usize,
}

impl Exiter {
    pub fn new(p: Params, at: &Seating) -> Self {
        let honest = super::honest::Honest::new(p.honest.clone(), at);
        Exiter {
            p,
            me: at.seat,
            honest,
            panicking: false,
            refused_after: 0,
            admitted_after: 0,
            exited: false,
            edges_after: 0,
        }
    }

    fn fires(&self, view: &AgentView) -> bool {
        match self.p.trigger {
            Trigger::Tick(t) => view.tick >= t,
            Trigger::DefaultsSeen(n) => {
                view.st
                    .contracts
                    .values()
                    .filter(|c| c.status == ContractStatus::Expired)
                    .count() as u64
                    >= n
            }
        }
    }
}

impl Strategy for Exiter {
    fn name(&self) -> &'static str {
        "exiter"
    }

    fn act(&mut self, view: &AgentView, rng: &mut Rng) -> Vec<Intent> {
        if !self.fires(view) {
            return self.honest.act(view, rng);
        }
        self.panicking = true;
        let me = self.me;
        let Some(m) = view.st.members.get(&me) else { return Vec::new() };
        let dust = view.st.params.dust_minor();
        let mut out = Vec::new();

        // Everything it owes, discharged. `Exit` is refused while any of it
        // stands, and a Suspended member may still take this path.
        for cid in view.debts_of(me) {
            let Some(c) = view.st.contracts.get(&cid) else { continue };
            if c.outstanding <= dust {
                continue;
            }
            out.push(if c.status == ContractStatus::Expired {
                Intent::Cure { contract: cid, amount_minor: c.outstanding }
            } else {
                Intent::Settle { contract: cid, amount_minor: c.outstanding }
            });
        }
        // Leaving the community and leaving the underwriter role are two acts,
        // and the second has a floor the first must not be able to jump.
        if view.st.underwriters.contains_key(&me) {
            out.push(Intent::Declare { supply_minor: 0 });
        }
        out.push(Intent::Exit);
        // The two that must be refused once it is gone. Emitted from the tick
        // after the exit onward, so a run that never exited never counts them.
        if m.status == MemberStatus::Exited {
            let peers = view.counterparties();
            if let Some(&other) = rng.pick(&peers) {
                out.push(Intent::Lend {
                    creditor: crate::intent::AgentRef::Member(other),
                    debtor: crate::intent::AgentRef::Member(me),
                    amount_minor: 1_000,
                    term: view.min_term(),
                    arb: None,
                });
            }
        }
        out
    }

    fn consents(&mut self, view: &AgentView, ask: &Ask, rng: &mut Rng) -> bool {
        if self.panicking {
            // It refuses every new trade and countersigns only discharge.
            return crate::intent::is_payment_to_me(ask);
        }
        self.honest.consents(view, ask, rng)
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        let exited = view.st.members.get(&self.me).is_some_and(|m| m.status == MemberStatus::Exited);
        if exited && !self.exited {
            self.exited = true;
        }
        if self.exited {
            // **Its stake edges, in and out, are still there.** An exit does
            // not withdraw what other people put behind it.
            self.edges_after = view
                .st
                .edges
                .keys()
                .filter(|&&(c, d)| c as MemberId == self.me || d as MemberId == self.me)
                .count();
        }
        // Everything it emitted after the exit applied.
        let mut after = false;
        self.refused_after = 0;
        self.admitted_after = 0;
        for l in log {
            if after {
                match l.outcome {
                    Outcome::Applied => self.admitted_after += 1,
                    Outcome::Refused(_) => self.refused_after += 1,
                    _ => {}
                }
            }
            if matches!(l.intent, Intent::Exit) && l.outcome == Outcome::Applied {
                after = true;
            }
        }
    }

    fn probe(&self) -> Probe {
        Probe {
            what: "it exits once, every later write is refused, and the stakes on it stay in the graph",
            reached: self.exited && self.admitted_after == 0 && self.refused_after > 0 && self.edges_after > 0,
            detail: format!(
                "exited: {}, {} refused and {} admitted after it, {} incident edges left standing",
                self.exited, self.refused_after, self.admitted_after, self.edges_after
            ),
        }
    }
}
