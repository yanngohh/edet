//! What an agent is: a behaviour, a consent rule, and the probe it exists for.

use std::cell::RefCell;
use std::collections::BTreeMap;

use edet_kernel::constants as k;
use edet_state::state::State;
use edet_state::types::*;

use crate::intent::{Ask, Intent};
use crate::population::World;
use crate::rng::Rng;

/// What one archetype exists to reach, and whether it reached it.
///
/// An archetype that never emits the transition it exists to test is scenery,
/// so this is part of every run's summary rather than a thing a test asks for.
#[derive(Clone, Debug)]
pub struct Probe {
    /// The transition it exists to reach and the property it asserts.
    pub what: &'static str,
    pub reached: bool,
    pub detail: String,
}

impl Probe {
    pub fn new(what: &'static str) -> Self {
        Probe { what, reached: false, detail: String::new() }
    }
}

/// A behaving agent.
pub trait Strategy {
    fn name(&self) -> &'static str;

    /// What this agent does this tick, in order. At most
    /// [`crate::intent::MAX_INTENTS_PER_TICK`].
    fn act(&mut self, view: &AgentView, rng: &mut Rng) -> Vec<Intent>;

    /// Would this agent sign, in `ask.role`, the intent another agent
    /// emitted?
    ///
    /// The default is the one a population needs to be honest about: no. A
    /// strategy that wants to trade says so.
    fn consents(&mut self, _view: &AgentView, _ask: &Ask, _rng: &mut Rng) -> bool {
        false
    }

    /// Called once at the end of every tick, and once more at the end of the
    /// run, with everything this agent emitted. Where a probe that is about a
    /// SEQUENCE — a row that expired through the sweep, a bound that must not
    /// grow with the tick — accumulates its evidence.
    fn observe(&mut self, _view: &AgentView, _log: &[crate::intent::AgentLog]) {}

    fn probe(&self) -> Probe;
}

/// Everything read off the ledger once per tick and shared by every agent.
///
/// The capacity distribution costs one cut per member and a strategy reads a
/// capacity several times a tick, so a population of `n` would otherwise cost
/// `n²` cuts a tick — which is the whole run, spent on a quantity that did not
/// move between two reads inside one tick.
#[derive(Default)]
pub struct TickCache {
    capacity: RefCell<BTreeMap<MemberId, u64>>,
    capacity_raw: RefCell<BTreeMap<MemberId, u64>>,
    conferrable: RefCell<BTreeMap<MemberId, u64>>,
    seed_reach: RefCell<BTreeMap<MemberId, u64>>,
    active: RefCell<Option<Vec<MemberId>>>,
    /// The contract book indexed three ways, built in ONE pass the first time
    /// any agent asks. Without it every agent walks every row every tick, so
    /// a population of `n` over a book of `r` costs `n x r` a tick — the run,
    /// spent re-deriving an index that did not move inside the tick.
    rows: RefCell<Option<Rows>>,
}

/// The contract book, indexed by the three questions a strategy asks of it.
#[derive(Default)]
struct Rows {
    by_debtor: BTreeMap<MemberId, Vec<ContractId>>,
    by_creditor: BTreeMap<MemberId, Vec<ContractId>>,
    panels: BTreeMap<MemberId, Vec<ContractId>>,
}

impl TickCache {
    /// A tick is one epoch, and an epoch decays every stake, so nothing here
    /// survives one.
    pub fn clear(&mut self) {
        self.capacity.borrow_mut().clear();
        self.capacity_raw.borrow_mut().clear();
        self.conferrable.borrow_mut().clear();
        self.seed_reach.borrow_mut().clear();
        *self.active.borrow_mut() = None;
        *self.rows.borrow_mut() = None;
    }
}

/// The ledger as one agent reads it this tick.
pub struct AgentView<'a> {
    pub st: &'a State,
    pub world: &'a World,
    pub cache: &'a TickCache,
    /// The seat this agent acts from.
    pub me: MemberId,
    /// The agent's own index, which is what "controlled by me" is about.
    pub agent: usize,
    pub tick: u64,
}

impl AgentView<'_> {
    pub fn epoch(&self) -> u64 {
        self.st.epoch
    }

    /// Does this agent control that member — its own seat, or a row it
    /// seated? A member it controls is never asked for consent.
    pub fn controls(&self, id: MemberId) -> bool {
        self.world.agent_of(id) == self.agent
    }

    /// The members this agent controls, in ascending id.
    pub fn seats(&self) -> &[MemberId] {
        self.world.seats_of(self.agent)
    }

    pub fn capacity_minor(&self, id: MemberId) -> u64 {
        if let Some(&v) = self.cache.capacity.borrow().get(&id) {
            return v;
        }
        let v = self.st.capacity_of_minor(id);
        self.cache.capacity.borrow_mut().insert(id, v);
        v
    }

    /// The PRISTINE cut: what the community would confer on this member with
    /// nothing outstanding. What a declaration and a farm's growth are about.
    pub fn capacity_raw_minor(&self, id: MemberId) -> u64 {
        if let Some(&v) = self.cache.capacity_raw.borrow().get(&id) {
            return v;
        }
        let v = self.st.capacity_raw_minor(id);
        self.cache.capacity_raw.borrow_mut().insert(id, v);
        v
    }

    pub fn conferrable_minor(&self, id: MemberId) -> u64 {
        if let Some(&v) = self.cache.conferrable.borrow().get(&id) {
            return v;
        }
        let v = self.st.conferrable_minor(id);
        self.cache.conferrable.borrow_mut().insert(id, v);
        v
    }

    pub fn seed_reach_minor(&self, id: MemberId) -> u64 {
        if let Some(&v) = self.cache.seed_reach.borrow().get(&id) {
            return v;
        }
        let v = self.st.seed_reach_minor(id);
        self.cache.seed_reach.borrow_mut().insert(id, v);
        v
    }

    /// Every Active member, in ascending id.
    pub fn active(&self) -> Vec<MemberId> {
        if let Some(v) = self.cache.active.borrow().as_ref() {
            return v.clone();
        }
        let v: Vec<MemberId> = self
            .st
            .members
            .values()
            .filter(|m| m.status == MemberStatus::Active)
            .map(|m| m.id)
            .collect();
        *self.cache.active.borrow_mut() = Some(v.clone());
        v
    }

    /// Active members this agent does not control — who there is to trade
    /// with.
    pub fn counterparties(&self) -> Vec<MemberId> {
        self.active().into_iter().filter(|&id| !self.controls(id)).collect()
    }

    /// **The wallet's own acceptance rule**, over the figures a view serves.
    /// The client's score and this are one call, not two implementations.
    pub fn risk(&self, id: MemberId) -> f64 {
        let Some(m) = self.st.members.get(&id) else { return 1.0 };
        edet_kernel::risk::member_risk(
            State::from_minor(self.capacity_minor(id)),
            State::from_minor(m.debt_out),
            m.rep.d_in,
            m.rep.d_out,
            self.st.params.risk_k,
            self.st.params.v_base,
        )
    }

    /// Build the tick's row index if nobody has yet.
    fn rows(&self) {
        if self.cache.rows.borrow().is_some() {
            return;
        }
        let mut rows = Rows::default();
        for c in self.st.contracts.values() {
            if matches!(c.status, ContractStatus::Active | ContractStatus::Expired) {
                rows.by_debtor.entry(c.debtor).or_default().push(c.id);
                rows.by_creditor.entry(c.creditor).or_default().push(c.id);
            }
            if let Some(t) = c.arb.as_ref() {
                if !c.arb_awarded {
                    for &a in &t.arbiters {
                        rows.panels.entry(a).or_default().push(c.id);
                    }
                }
            }
        }
        *self.cache.rows.borrow_mut() = Some(rows);
    }

    /// Live obligations owed by this member, oldest first.
    pub fn debts_of(&self, id: MemberId) -> Vec<ContractId> {
        self.rows();
        let rows = self.cache.rows.borrow();
        rows.as_ref().and_then(|r| r.by_debtor.get(&id)).cloned().unwrap_or_default()
    }

    /// Live claims held by this member, oldest first.
    pub fn claims_of(&self, id: MemberId) -> Vec<ContractId> {
        self.rows();
        let rows = self.cache.rows.borrow();
        rows.as_ref().and_then(|r| r.by_creditor.get(&id)).cloned().unwrap_or_default()
    }

    /// Rows whose consented panel names this member and whose award has not
    /// been minted — an arbiter's duty, which nobody can ask them to perform
    /// because the AMOUNT is their own judgement.
    pub fn panels_of(&self, id: MemberId) -> Vec<ContractId> {
        self.rows();
        let rows = self.cache.rows.borrow();
        rows.as_ref().and_then(|r| r.panels.get(&id)).cloned().unwrap_or_default()
    }

    /// Is this member's write budget open at all?
    pub fn established(&self, id: MemberId) -> bool {
        edet_state::bond::established(self.st, id)
    }

    pub fn bond_headroom_minor(&self, id: MemberId) -> u64 {
        self.st.bond_headroom_minor(id)
    }

    pub fn bond_unit_minor(&self) -> u64 {
        self.st.params.bond_unit_minor()
    }

    /// **What a member's standing still carries in NEW ROWS**, minor units,
    /// over `bond_unit_minor`.
    ///
    /// Served so an archetype can MEASURE the ceiling, never so it can decide
    /// against it: a farm that pre-filtered on this would be measuring its own
    /// strategy rather than the ledger's rule, which is why the farm goes on
    /// asking `bond_headroom_minor` and lets `ET-BND-006` be the signal.
    pub fn seat_reach_minor(&self, id: MemberId) -> u64 {
        self.st.seat_reach_minor(id)
    }

    /// The shortest settlement term any chain admits. No `ParamKey` reaches
    /// it, so a strategy that draws below it has drawn a refusal.
    pub fn min_term(&self) -> u64 {
        self.st.params.min_maturity_epochs.max(k::MIN_MATURITY_EPOCHS)
    }
}
