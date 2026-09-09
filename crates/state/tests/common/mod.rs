//! The fixtures every state-layer suite drives the transition function
//! through, over the one driver the whole tree shares.
//!
//! One `Chain` rather than one per file, because the suites disagreed about
//! what a community was and the disagreement hid things: a probe written
//! against a fixture with no underwriters is measuring a community where zero
//! is absorbing, and every capacity assertion in it passes for the wrong
//! reason. Here a community is founded the one way §Adoption describes — people
//! willing to stand behind it, no graph — and standing is earned by trading.
//!
//! **The mechanics are `edet_swarm::Driver` and this is its fixtures.** A
//! second driver would drift, and the one that drifted would be the one the
//! rest of the tree does not exercise: `Driver` is what a population of
//! strategies runs on, what the conformance test replays a node's own blocks
//! through, and what every suite here writes against.
//!
//! **Every transition is audited.** `Driver::apply` runs the whole
//! §Verification audit after each call, accepted or refused, so a property
//! that holds in the scene it was written for and breaks the ledger elsewhere
//! fails here rather than in production.

#![allow(dead_code)]
// Each suite is its own binary and uses a different part of this module, so a
// re-export nobody in THIS binary names is not dead code — it is the module
// being shared.
#![allow(unused_imports)]

use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

pub use edet_swarm::driver::{bonded_dud, epochs, witness_conserved, Audit, Driver};
pub use edet_swarm::keys::{consensus_key, member_key as key, stranger_key};

/// What a founding underwriter declares, in denomination units.
pub const SUPPLY: f64 = 2500.0;
/// A maturity comfortably inside every horizon bound.
pub const MATURITY: u64 = 30;

/// A driver with the fixtures a state-layer suite writes against.
pub struct Chain(pub Driver);

impl std::ops::Deref for Chain {
    type Target = Driver;
    fn deref(&self) -> &Driver {
        &self.0
    }
}

impl std::ops::DerefMut for Chain {
    fn deref_mut(&mut self) -> &mut Driver {
        &mut self.0
    }
}

impl Chain {
    /// `k` founding underwriters, each declaring `SUPPLY`, then `m` ordinary
    /// accounts. No stakes: a community begins with people willing to stand
    /// behind it, not with a graph.
    pub fn founded(k: usize, m: usize) -> Self {
        Self::founded_with(&vec![SUPPLY; k], m)
    }

    /// The same, with the founding supplies named one by one.
    pub fn founded_with(supplies: &[f64], m: usize) -> Self {
        let mut st = State::default();
        for (i, &s) in supplies.iter().enumerate() {
            st.add_underwriter(vec![key(i)], s).expect("founding underwriter");
        }
        for i in supplies.len()..supplies.len() + m {
            st.new_account(vec![key(i)]);
        }
        Chain(Driver::new(st))
    }

    /// Designate `id` a founding validator. Ordering the chain and funding it
    /// are two roles; only the tests about the validator set need this one.
    pub fn validator(&mut self, id: MemberId) -> &mut Self {
        self.st
            .set_consensus_key(id, consensus_key(id as usize))
            .expect("consensus key");
        self.st.set_genesis_validator(id, 1).expect("genesis validator");
        self
    }

    /// A copy of this chain to age alongside it, for a probe that has to
    /// separate the effect it is measuring from the decay every epoch boundary
    /// applies. **Measure against a counterfactual, not a before**: a window
    /// that also advances the clock gets credited with the clock's work.
    pub fn clone_for_control(&self) -> Self {
        Chain(self.0.clone_for_control())
    }

    /// Drive only the cold audit, for a fixture that repeats one scene shape
    /// tens of thousands of times.
    pub fn cold_audit_only(self) -> Self {
        Chain(self.0.cold_audit_only())
    }

    // The mutating half of the driver is forwarded rather than reached
    // through `DerefMut`, and that is not decoration: two-phase borrows apply
    // to an INHERENT method, so `c.err(tx, .., c.st.epoch + 1)` compiles here
    // and does not when the same call has to go through `DerefMut` first.
    // Every suite in the tree writes that shape.

    /// A fresh transaction id.
    pub fn tx_id(&mut self) -> [u8; 32] {
        self.0.tx_id()
    }

    /// Apply under a fresh id and a window wide enough never to be the reason
    /// a call fails.
    pub fn apply(&mut self, tx: Tx, signers: &[Key]) -> edet_state::errors::Res<()> {
        self.0.apply(tx, signers)
    }

    /// Apply with the envelope fields chosen by the caller — replay, expiry
    /// and window are exactly what these want to control.
    pub fn apply_raw(&mut self, tx: Tx, id: [u8; 32], not_after: u64, signers: &[Key]) -> edet_state::errors::Res<()> {
        self.0.apply_raw(tx, id, not_after, signers)
    }

    pub fn ok(&mut self, tx: Tx, signers: &[Key]) {
        self.0.ok(tx, signers)
    }

    pub fn err(&mut self, tx: Tx, signers: &[Key], code: edet_state::errors::Code) {
        self.0.err(tx, signers, code)
    }

    /// Advance to `epoch`, running every epoch boundary along the way.
    pub fn goto(&mut self, epoch: u64) {
        self.0.goto(epoch)
    }

    /// Book an obligation. Returns the contract id.
    pub fn lend(&mut self, creditor: MemberId, debtor: MemberId, amount: f64) -> ContractId {
        let id = self.st.next_contract;
        self.ok(
            Tx::Accept {
                debtor: Party::Member(debtor),
                creditor: Party::Member(creditor),
                amount,
                maturity_epochs: MATURITY,
                arb: None,
            },
            &[key(creditor as usize), key(debtor as usize)],
        );
        id
    }

    pub fn settle(&mut self, contract: ContractId, amount: f64) {
        let c = self.st.contracts[&contract].clone();
        self.ok(Tx::Settle { contract, amount }, &[key(c.debtor as usize), key(c.creditor as usize)]);
    }

    /// The whole round trip that confers standing: `creditor` lends to
    /// `debtor`, `debtor` honours it. This is the ONLY way a member earns
    /// capacity, which is why every fixture that needs standing calls it
    /// rather than writing an edge directly.
    pub fn back(&mut self, creditor: MemberId, debtor: MemberId, amount: f64) {
        let c = self.lend(creditor, debtor, amount);
        self.settle(c, amount);
    }

    /// Advance past `contract`'s maturity and let the epoch crank do its work.
    ///
    /// **A default is something that HAPPENS rather than something somebody
    /// calls.** `MarkExpired` is still in the alphabet and still
    /// permissionless, but the epoch sweep runs it at every boundary, so a
    /// test that wants a default gets one by waiting. Asserting the status
    /// here makes every caller a check on the sweep as well as a fixture.
    pub fn default_on(&mut self, contract: ContractId) {
        let due = self.st.contracts[&contract].maturity_epoch;
        self.goto(due + 1);
        assert_eq!(
            self.st.contracts[&contract].status,
            ContractStatus::Expired,
            "the epoch crank must expire a claim that has fallen due"
        );
    }

    pub fn cap(&self, id: MemberId) -> f64 {
        self.st.capacity_of(id)
    }

    pub fn status(&self, id: MemberId) -> MemberStatus {
        self.st.members[&id].status
    }

    /// A contract's outstanding amount in MAJOR units. The ledger stores minor
    /// units; a test reads in the denomination it wrote in.
    pub fn outstanding(&self, contract: ContractId) -> f64 {
        State::from_minor(self.st.contracts[&contract].outstanding)
    }

    /// Total debt the whole book carries, both ways of counting it. The two
    /// must agree; `audit` already insists, and this is for tests that want to
    /// compare the figure across a transition.
    pub fn total_debt(&self) -> f64 {
        State::from_minor(self.st.members.values().map(|m| m.debt_out).sum())
    }

    // ------------------------------------------------------------- bonds --

    /// Put the chain into the bonded regime with a hostile-but-legal bond size
    /// and bonds that do not release during the test.
    pub fn tighten(&mut self) {
        self.st.params.bond_free_allowance = 0;
        self.st.params.bond_fraction = 0.10; // the constitutional ceiling
        self.st.params.bond_release_epochs = 100;
    }

    /// Submit duds as `signer` until the gate refuses, returning how many were
    /// admitted first. Panics if the gate never engages — an unbounded write
    /// channel is the failure this is looking for.
    pub fn exhaust(&mut self, signer: Key) -> u32 {
        use edet_state::errors::*;
        for i in 0..10_000 {
            match self.apply(bonded_dud(), &[signer]) {
                Err(Error(ET_BOND_EXHAUSTED)) => return i,
                Err(Error(ET_CTR_UNKNOWN)) => continue,
                other => panic!("unexpected outcome from a dud at {i}: {other:?}"),
            }
        }
        panic!("the gate never denied — headroom is unbounded");
    }
}
