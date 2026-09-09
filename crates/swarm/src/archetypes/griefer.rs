//! **Writes to burn other people's budget and its own.** The probe for what a
//! failed transaction costs, and for the two envelope rules nothing else in a
//! population exercises.
//!
//! Three properties, each with an edge somebody once got wrong. A transaction
//! refused at the GATE leaves nothing behind — it was never admitted, so no
//! replay id is spent. An envelope that authorises NOTHING is refunded,
//! unbilled and re-appliable, because burning its id would let anyone who saw
//! a pending request strip a co-signature and kill the genuine transaction for
//! free. And a permissionless crank that found nothing to do spends no id
//! either, or a key belonging to nobody writes into state the root hashes.

use edet_state::errors::*;
use edet_state::types::*;

use super::Seating;
use crate::intent::{AgentRef, Ask, Intent, Outcome};
use crate::rng::Rng;
use crate::strategy::{AgentView, Probe, Strategy};

#[derive(Clone, Debug)]
pub struct Params {
    pub duds_per_tick: u32,
    /// How often it isolates the stripped-envelope probe: on those ticks it
    /// emits that and nothing else, so the encumbrance before and after is
    /// about one envelope.
    pub strip_every: u64,
    pub replay: bool,
    pub junk: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { duds_per_tick: 80, strip_every: 7, replay: true, junk: true }
    }
}

#[derive(Default)]
struct Seen {
    denied: bool,
    forfeited: u64,
    /// A stripped envelope refused ET-MEM-003 whose id never reached the
    /// replay cache.
    strip_unbilled: bool,
    strip_billed: bool,
    /// The same transaction, properly signed, applied afterwards.
    restored_applied: bool,
    replay_refused: bool,
    /// A crank that found nothing and left no id.
    crank_left_no_id: bool,
    /// A durable write it can replay. Priced, and priced for this reason.
    key_rewritten: bool,
}

pub struct Griefer {
    p: Params,
    me: MemberId,
    seen: Seen,
    /// The counterparty it grieves, and the encumbrance it had when the
    /// stripped envelope went out.
    target: Option<MemberId>,
    before: Option<(u64, u32)>,
    /// An emission of its own it means to replay, and the tick it was made.
    replayable: Option<usize>,
}

impl Griefer {
    pub fn new(p: Params, at: &Seating) -> Self {
        Griefer { p, me: at.seat, seen: Seen::default(), target: None, before: None, replayable: None }
    }
}

impl Strategy for Griefer {
    fn name(&self) -> &'static str {
        "griefer"
    }

    fn act(&mut self, view: &AgentView, rng: &mut Rng) -> Vec<Intent> {
        let me = self.me;
        if view.st.members.get(&me).is_none_or(|m| m.status != MemberStatus::Active) {
            return Vec::new();
        }
        // **The pair it grieves is an established member**, drawn once and
        // kept. A counterparty the seed does not reach declines everything and
        // carries no bond, so grieving one measures nothing.
        let peers: Vec<MemberId> = view.counterparties().into_iter().filter(|&id| view.established(id)).collect();
        let Some(&other) = self.target.as_ref().filter(|t| peers.contains(t)).or(rng.pick(&peers)) else {
            return Vec::new();
        };

        // **The stripped envelope, alone in its tick.** Nothing else this
        // agent emits may touch a bond in the same tick, or the reading below
        // is about two envelopes.
        if self.p.strip_every > 0 && view.tick.is_multiple_of(self.p.strip_every) {
            self.target = Some(other);
            // **Its OWN encumbrance, and only its own.** The stripped envelope
            // carries one signature, so the gate bills the one signer — and
            // the counterparty's bonds move in the same tick for reasons that
            // have nothing to do with this envelope, which would make a
            // reading of them a reading of the rest of the population.
            let mine = view.st.members.get(&me);
            self.before = Some((mine.map(|m| m.bond_enc()).unwrap_or(0), mine.map(|m| m.bond_free_used).unwrap_or(0)));
            return vec![Intent::Stripped(Box::new(Intent::Lend {
                creditor: AgentRef::Member(other),
                debtor: AgentRef::Member(me),
                amount_minor: 5_000,
                term: view.min_term(),
                arb: None,
            }))];
        }
        // The tick after: the same transaction with the co-signature restored,
        // which must apply.
        if self.p.strip_every > 0 && view.tick % self.p.strip_every == 1 && !self.seen.restored_applied {
            if let Some(t) = self.target {
                return vec![Intent::Lend {
                    creditor: AgentRef::Member(t),
                    debtor: AgentRef::Member(me),
                    amount_minor: 5_000,
                    term: view.min_term(),
                    arb: None,
                }];
            }
        }

        let mut out = Vec::new();
        // **Re-registering a consensus key**, which is the transition whose
        // own doc comment says it would be an unlimited free channel if it
        // were not priced. It applies, which is what a replay needs: a dud
        // fails on its own merits and there is nothing to replay in a refusal.
        out.push(Intent::SetConsensusKey(Some(crate::keys::consensus_key(me as usize))));
        // Duds until the gate refuses. Each one is well-formed, bonded at the
        // ordinary multiple, and fails on its own merits.
        for _ in 0..self.p.duds_per_tick {
            out.push(Intent::Dud);
        }
        if self.p.replay {
            if let Some(e) = self.replayable.take() {
                out.push(Intent::Replayed { emission: e });
            }
        }
        if self.p.junk {
            // Each of these must fail at dispatch, and each is a different
            // door. The crank on a contract that does not exist is the one
            // that must leave no id at all.
            out.push(Intent::MarkExpired { contract: u64::MAX - 1 });
            out.push(Intent::Lend {
                creditor: AgentRef::Member(other),
                debtor: AgentRef::Member(me),
                amount_minor: 0,
                term: view.min_term(),
                arb: None,
            });
            out.push(Intent::Extend {
                contract: 0,
                new_maturity_epoch: edet_kernel::constants::MAX_HORIZON_EPOCHS * 2,
            });
        }
        out
    }

    /// It signs nothing for anybody.
    fn consents(&mut self, _view: &AgentView, _ask: &Ask, _rng: &mut Rng) -> bool {
        false
    }

    fn observe(&mut self, view: &AgentView, log: &[crate::intent::AgentLog]) {
        if let Some(m) = view.st.members.get(&self.me) {
            if m.bond_denied_this_epoch || m.bond_saturated_epochs > 0 {
                self.seen.denied = true;
            }
        }
        self.seen.forfeited = view.st.forfeit_reserve.get(&self.me).copied().unwrap_or(0);

        // The whole log for the flags that only ever go from false to true;
        // the CURRENT tick for the encumbrance reading, which is about one
        // envelope and pairs with the snapshot `act` took this tick.
        for (i, l) in log.iter().enumerate() {
            match (&l.intent, &l.outcome) {
                (Intent::Stripped(_), Outcome::Refused(ET_MEM_NOT_SIGNER)) => {
                    if l.envelope.as_ref().is_some_and(|e| !view.st.is_applied(&e.id, e.not_after)) {
                        self.seen.strip_unbilled = true;
                    }
                    if l.tick == view.tick {
                        if let Some((enc, used)) = self.before.take() {
                            let now = view.st.members.get(&self.me);
                            let moved = now.map(|m| m.bond_enc()).unwrap_or(0) != enc
                                || now.map(|m| m.bond_free_used).unwrap_or(0) != used;
                            if moved {
                                self.seen.strip_billed = true;
                            }
                        }
                    }
                }
                (Intent::Lend { .. }, Outcome::Applied) => self.seen.restored_applied = true,
                (Intent::SetConsensusKey(_), Outcome::Applied) => self.seen.key_rewritten = true,
                (Intent::Dud, Outcome::Refused(ET_BOND_EXHAUSTED)) => self.seen.denied = true,
                (Intent::Replayed { .. }, Outcome::Refused(ET_TX_REPLAY)) => self.seen.replay_refused = true,
                (Intent::MarkExpired { .. }, Outcome::Refused(_))
                    if l.envelope.as_ref().is_some_and(|e| !view.st.is_applied(&e.id, e.not_after)) =>
                {
                    self.seen.crank_left_no_id = true;
                }
                _ => {}
            }
            // **A replay has to land inside its own window**, or it is
            // `ET-TX-002` and measures expiry rather than replay. The
            // driver's window is `epoch + 5`, so only an emission from this
            // tick is worth resubmitting next tick.
            if l.tick == view.tick && l.outcome == Outcome::Applied && self.replayable.is_none() {
                self.replayable = Some(i);
            }
        }
    }

    fn probe(&self) -> Probe {
        let s = &self.seen;
        Probe {
            what: "the gate denies it, a stripped envelope is unbilled and re-appliable, a replay is refused, a crank that found nothing spends no id",
            reached: s.denied
                && s.strip_unbilled
                && !s.strip_billed
                && s.key_rewritten
                && s.replay_refused
                && s.crank_left_no_id,
            detail: format!(
                "denied: {}, stripped unbilled: {}, the strip billed its signer: {}, the restored transaction applied: {}, a durable write applied: {}, replay refused: {}, crank left no id: {}, forfeited {} minor",
                s.denied,
                s.strip_unbilled,
                s.strip_billed,
                s.restored_applied,
                s.key_rewritten,
                s.replay_refused,
                s.crank_left_no_id,
                s.forfeited
            ),
        }
    }
}
