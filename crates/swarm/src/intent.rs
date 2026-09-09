//! What an agent asks for, and the one place an envelope is built.
//!
//! **A strategy emits intents, never envelopes.** A strategy that assembled
//! its own signer list is a strategy that can fail for reasons that have
//! nothing to do with the behaviour being modelled, and a run whose refusals
//! are mostly `ET-MEM-003` has measured the adapter rather than the ledger.
//! The two deliberate malformations a griefer needs — a stripped co-signature
//! and a replayed id — are intents too, so this stays the only door.
//!
//! **Consent is a strategy's decision, never the adapter's.** Every
//! counterparty an intent names is asked in its role, and its key goes on the
//! envelope only if it says yes. A refusal here is a *decline*: the
//! pending-signature pool, not `apply`, and counted apart from a ledger
//! refusal for exactly that reason.
//!
//! **A role is a signature.** There is no `Role::Beneficiary` and no
//! `Role::Arbiter`, because neither signs anything an emitter could compose:
//! `ApproveSupporter` carries only the beneficiary's own signature, and an
//! arbiter's attestation carries an AMOUNT that is the arbiter's own
//! judgement — an ask would carry the emitter's, which is the collusion the
//! late-defaulter archetype exists to measure rather than to perform. An
//! honest panel member attests on its own initiative, in `act`.

use edet_state::errors::Code;
use edet_state::state::State;
use edet_state::tx::Tx;
use edet_state::types::*;

use crate::keys::fresh_key;
use crate::population::World;

/// The most intents one agent may emit in one tick. An agent that controls a
/// farm emits for every member of it, so this is generous — what it bounds is
/// a strategy that has started to run away, and the run reports every intent
/// it dropped so the bound can never be mistaken for the population's own
/// behaviour.
pub const MAX_INTENTS_PER_TICK: usize = 32_768;

/// A party an intent names: an account that already exists, or the `n`-th key
/// `owner` is minting.
///
/// `Fresh` resolves to `Party::Key` until the ledger seats it and to the
/// seated `MemberId` afterwards — read off `State::member_of_key` at every
/// resolution, so an agent that seated a row last tick names a member this
/// tick without having to notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRef {
    Member(MemberId),
    Fresh { owner: usize, n: u32 },
}

/// The role a counterparty is asked to sign in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Debtor,
    Creditor,
    Buyer,
    Seller,
    NewDebtor,
    Guardian,
}

/// A request for a signature, put to the strategy that controls the member.
#[derive(Clone, Debug)]
pub struct Ask {
    pub from: MemberId,
    pub member: MemberId,
    pub role: Role,
    pub intent: Intent,
}

#[derive(Clone, Debug)]
pub enum Intent {
    Lend {
        creditor: AgentRef,
        debtor: AgentRef,
        amount_minor: u64,
        term: u64,
        arb: Option<ArbTermsWire>,
    },
    Sell {
        seller: AgentRef,
        buyer: AgentRef,
        amount_minor: u64,
        term: u64,
    },
    Settle {
        contract: ContractId,
        amount_minor: u64,
    },
    Cure {
        contract: ContractId,
        amount_minor: u64,
    },
    Transfer {
        contract: ContractId,
        new_debtor: MemberId,
    },
    Extend {
        contract: ContractId,
        new_maturity_epoch: u64,
    },
    MarkExpired {
        contract: ContractId,
    },
    Declare {
        supply_minor: u64,
    },
    Exit,
    Propose(ProposalKind),
    Assent(ProposalId),
    ListBeneficiaries(Vec<(MemberId, f64)>),
    ApproveSupporter {
        supporter: MemberId,
        approved: bool,
    },
    ArbAttest {
        contract: ContractId,
        amount_minor: u64,
    },
    RegisterGuardians {
        guardians: Vec<MemberId>,
        threshold: u32,
        veto_window_epochs: u64,
    },
    RotateRequest {
        member: MemberId,
        new_keys: Vec<Key>,
    },
    /// Stop a rotation of the ACTOR's own keys. It is the member's veto and
    /// not a guardian's: `rotate_veto` asks for the member's own signature,
    /// because what it stops is a threshold of their guardians moving their
    /// keys without them.
    RotateVeto,
    RotateFinalize {
        member: MemberId,
    },
    SetConsensusKey(Option<Key>),
    ForfeitBonds {
        member: MemberId,
    },
    /// Well-formed, bonded at the ordinary multiple, fails on its own merits
    /// (`ET-CTR-001`). Spends headroom and nothing else.
    Dud,
    /// The envelope of the inner intent with every co-signature but the
    /// actor's own dropped. The one intent that touches the envelope, and only
    /// by removal.
    Stripped(Box<Intent>),
    /// The actor's own earlier envelope (its `n`-th emission), resubmitted
    /// verbatim: same id, same window, same signers.
    Replayed {
        emission: usize,
    },
    /// Act as a member this agent controls — its own seat is one, a row it
    /// seated is another.
    ///
    /// An agent is not a member: a farm operator acts for every sybil it
    /// seated, and every intent that is about "me" (a `Dud`, an `Exit`, an
    /// `Assent`) has to be able to say which me. Naming a member the agent
    /// does NOT control composes an envelope without that member's signature,
    /// which the ledger refuses — the rule is enforced where every other
    /// consent rule is.
    As {
        member: MemberId,
        intent: Box<Intent>,
    },
}

impl Intent {
    /// A short name for the summary and the violation report — the alphabet
    /// letter, not the payload.
    pub fn kind(&self) -> &'static str {
        match self {
            Intent::Lend { .. } => "Lend",
            Intent::Sell { .. } => "Sell",
            Intent::Settle { .. } => "Settle",
            Intent::Cure { .. } => "Cure",
            Intent::Transfer { .. } => "Transfer",
            Intent::Extend { .. } => "Extend",
            Intent::MarkExpired { .. } => "MarkExpired",
            Intent::Declare { .. } => "Declare",
            Intent::Exit => "Exit",
            Intent::Propose(_) => "Propose",
            Intent::Assent(_) => "Assent",
            Intent::ListBeneficiaries(_) => "ListBeneficiaries",
            Intent::ApproveSupporter { .. } => "ApproveSupporter",
            Intent::ArbAttest { .. } => "ArbAttest",
            Intent::RegisterGuardians { .. } => "RegisterGuardians",
            Intent::RotateRequest { .. } => "RotateRequest",
            Intent::RotateVeto => "RotateVeto",
            Intent::RotateFinalize { .. } => "RotateFinalize",
            Intent::SetConsensusKey(_) => "SetConsensusKey",
            Intent::ForfeitBonds { .. } => "ForfeitBonds",
            Intent::Dud => "Dud",
            Intent::Stripped(_) => "Stripped",
            Intent::Replayed { .. } => "Replayed",
            Intent::As { intent, .. } => intent.kind(),
        }
    }
}

/// What one emission came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    /// The ledger refused it, by code.
    Refused(Code),
    /// A counterparty would not sign. The pending-signature pool, not `apply`.
    Declined(MemberId),
    /// The adapter had nothing to build from — a replay of an emission that
    /// does not exist. Counted rather than dropped, because an archetype
    /// whose intents mostly go unbuilt is scenery and the summary has to say
    /// so.
    Unbuilt(&'static str),
}

/// One emission, with everything a violation report needs to name it.
#[derive(Clone, Debug)]
pub struct AgentLog {
    pub tick: u64,
    pub intent: Intent,
    pub envelope: Option<crate::driver::Envelope>,
    pub outcome: Outcome,
    /// The row an `Accept` or a `Sale` booked, read off `next_contract`
    /// before the apply.
    pub contract: Option<ContractId>,
}

/// A signature the adapter must go and ask for.
#[derive(Clone, Debug)]
pub struct Asked {
    pub ask: Ask,
    pub key: Key,
    /// Whether a decline kills the intent or only drops the key. The
    /// creditor's signature on a `Transfer` is the one optional case: the
    /// ledger refuses the uninsured swap without it and takes the insured one
    /// without, so which it needed is the ledger's answer and not the
    /// adapter's.
    pub required: bool,
}

/// A transaction and the signatures it is waiting on.
#[derive(Clone, Debug)]
pub struct Composed {
    pub tx: Tx,
    /// Keys that go on the envelope whatever anybody says: the actor's own,
    /// the members it controls, and the fresh keys it is minting.
    pub signers: Vec<Key>,
    pub asks: Vec<Asked>,
}

/// The `Party` an [`AgentRef`] names against current state.
pub fn party(st: &State, r: &AgentRef) -> Party {
    match r {
        AgentRef::Member(id) => Party::Member(*id),
        AgentRef::Fresh { owner, n } => {
            let k = fresh_key(*owner, *n);
            match st.member_of_key(&k) {
                Some(id) => Party::Member(id),
                None => Party::Key(k),
            }
        }
    }
}

/// The key that signs for an [`AgentRef`], and the member it resolves to.
fn signing_key(st: &State, r: &AgentRef) -> Option<(Key, Option<MemberId>)> {
    match r {
        AgentRef::Member(id) => st.members.get(id).and_then(|m| m.keys.first()).map(|k| (*k, Some(*id))),
        AgentRef::Fresh { owner, n } => {
            let k = fresh_key(*owner, *n);
            Some((k, st.member_of_key(&k)))
        }
    }
}

/// The key that signs for a member that already exists.
fn key_of(st: &State, id: MemberId) -> Option<Key> {
    st.members.get(&id).and_then(|m| m.keys.first()).copied()
}

/// Build the envelope for one intent: the transaction, the signatures the
/// actor already holds, and the ones it has to ask for.
///
/// Every amount crosses `State::from_minor` here and nowhere else — the
/// payload is one of the boundary's two legal crossings, and a third would be
/// a defect.
pub fn compose(st: &State, world: &World, actor: MemberId, intent: &Intent) -> Composed {
    let mut b = Build { st, world, agent: world.agent_of(actor), actor, intent, signers: Vec::new(), asks: Vec::new() };
    let me = key_of(st, actor);
    let parties = |c: ContractId| st.contracts.get(&c).map(|x| (x.debtor, x.creditor));

    let tx = match intent {
        Intent::Lend { creditor, debtor, amount_minor, term, arb } => {
            b.side(creditor, Role::Creditor, true);
            b.side(debtor, Role::Debtor, true);
            Tx::Accept {
                debtor: party(st, debtor),
                creditor: party(st, creditor),
                amount: State::from_minor(*amount_minor),
                maturity_epochs: *term,
                arb: arb.clone(),
            }
        }
        Intent::Sell { seller, buyer, amount_minor, term } => {
            b.side(seller, Role::Seller, true);
            b.side(buyer, Role::Buyer, true);
            Tx::Sale {
                seller: party(st, seller),
                buyer: party(st, buyer),
                amount: State::from_minor(*amount_minor),
                maturity_epochs: *term,
            }
        }
        Intent::Settle { contract, amount_minor } => {
            if let Some((d, c)) = parties(*contract) {
                b.member(d, Role::Debtor, true);
                b.member(c, Role::Creditor, true);
            }
            Tx::Settle { contract: *contract, amount: State::from_minor(*amount_minor) }
        }
        Intent::Cure { contract, amount_minor } => {
            if let Some((d, c)) = parties(*contract) {
                b.member(d, Role::Debtor, true);
                b.member(c, Role::Creditor, true);
            }
            Tx::Cure { contract: *contract, amount: State::from_minor(*amount_minor) }
        }
        Intent::Extend { contract, new_maturity_epoch } => {
            if let Some((d, c)) = parties(*contract) {
                b.member(d, Role::Debtor, true);
                b.member(c, Role::Creditor, true);
            }
            Tx::Extend { contract: *contract, new_maturity_epoch: *new_maturity_epoch }
        }
        Intent::Transfer { contract, new_debtor } => {
            if let Some((d, c)) = parties(*contract) {
                b.member(d, Role::Debtor, true);
                b.member(c, Role::Creditor, false);
            }
            b.member(*new_debtor, Role::NewDebtor, true);
            Tx::Transfer { contract: *contract, new_debtor: *new_debtor }
        }
        // The two permissionless cranks carry no signature to defer, and
        // `bond::due` answers `Free` from the schedule before it asks who
        // would pay.
        Intent::MarkExpired { contract } => Tx::MarkExpired { contract: *contract },
        Intent::ForfeitBonds { member } => Tx::ForfeitBonds { member: *member },
        Intent::Declare { supply_minor } => {
            b.signers.extend(me);
            Tx::DeclareSupply { member: actor, supply: State::from_minor(*supply_minor) }
        }
        Intent::Exit => {
            b.signers.extend(me);
            Tx::Exit { member: actor }
        }
        Intent::Propose(kind) => {
            b.signers.extend(me);
            Tx::Propose { author: actor, kind: kind.clone() }
        }
        Intent::Assent(proposal) => {
            b.signers.extend(me);
            Tx::Assent { member: actor, proposal: *proposal }
        }
        Intent::ListBeneficiaries(entries) => {
            b.signers.extend(me);
            Tx::ListBeneficiaries { supporter: actor, entries: entries.clone() }
        }
        Intent::ApproveSupporter { supporter, approved } => {
            b.signers.extend(me);
            Tx::ApproveSupporter { beneficiary: actor, supporter: *supporter, approved: *approved }
        }
        Intent::ArbAttest { contract, amount_minor } => {
            b.signers.extend(me);
            Tx::ArbAttest { contract: *contract, arbiter: actor, amount: State::from_minor(*amount_minor) }
        }
        Intent::RegisterGuardians { guardians, threshold, veto_window_epochs } => {
            b.signers.extend(me);
            Tx::RegisterGuardians {
                member: actor,
                guardians: guardians.clone(),
                threshold: *threshold,
                veto_window_epochs: *veto_window_epochs,
            }
        }
        Intent::RotateRequest { member, new_keys } => {
            // A THRESHOLD of the member's own guardians authorises this, so
            // the actor's own signature is one of theirs or it is nothing.
            b.signers.extend(me);
            if let Some(cfg) = st.members.get(member).and_then(|m| m.guardian.as_ref()) {
                let guardians: Vec<MemberId> = cfg.guardians.iter().copied().collect();
                for g in guardians {
                    if g != actor {
                        b.member(g, Role::Guardian, false);
                    }
                }
            }
            Tx::RotateRequest { member: *member, new_keys: new_keys.clone() }
        }
        Intent::RotateVeto => {
            b.signers.extend(me);
            Tx::RotateVeto { member: actor }
        }
        Intent::RotateFinalize { member } => Tx::RotateFinalize { member: *member },
        Intent::SetConsensusKey(key) => {
            b.signers.extend(me);
            Tx::SetConsensusKey { member: actor, key: *key }
        }
        Intent::Dud => {
            b.signers.extend(me);
            crate::driver::bonded_dud()
        }
        Intent::As { member, intent } => return compose(st, world, *member, intent),
        Intent::Stripped(inner) => {
            let composed = compose(st, world, actor, inner);
            // Every co-signature but the actor's own, dropped. Nobody is
            // asked: a stripped envelope is a griefing move, and asking would
            // make it the honest transaction it exists not to be.
            return Composed { tx: composed.tx, signers: me.into_iter().collect(), asks: Vec::new() };
        }
        // Resubmitted verbatim by the caller, which is the only place the
        // earlier envelope exists. What is built here is never sent.
        Intent::Replayed { .. } => {
            b.signers.extend(me);
            crate::driver::bonded_dud()
        }
    };

    let mut signers = b.signers;
    // Signer order is irrelevant — `bond::payers` sorts canonically — and the
    // adapter emits them in ascending member id anyway, so an envelope read
    // back in a log is the envelope the gate saw.
    signers.sort_by_key(|k| st.member_of_key(k).unwrap_or(MemberId::MAX));
    signers.dedup();
    Composed { tx, signers, asks: b.asks }
}

/// One envelope under construction. A struct rather than a closure because
/// every side of a trade mutates the same two lists and a second borrow of
/// the first closure is not a thing the compiler will hand out.
struct Build<'a> {
    st: &'a State,
    world: &'a World,
    agent: usize,
    actor: MemberId,
    intent: &'a Intent,
    signers: Vec<Key>,
    asks: Vec<Asked>,
}

impl Build<'_> {
    /// Put one party on the envelope: signed outright when the acting agent
    /// controls it, asked otherwise.
    fn side(&mut self, r: &AgentRef, role: Role, required: bool) {
        let Some((key, member)) = signing_key(self.st, r) else { return };
        match member {
            // A key nobody holds yet is the actor's own mint: it signs, and
            // `resolve` demands exactly that signature before it seats a row.
            None => self.signers.push(key),
            Some(id) if self.world.agent_of(id) == self.agent => self.signers.push(key),
            Some(id) => self.asks.push(Asked {
                ask: Ask { from: self.actor, member: id, role, intent: self.intent.clone() },
                key,
                required,
            }),
        }
    }

    fn member(&mut self, id: MemberId, role: Role, required: bool) {
        self.side(&AgentRef::Member(id), role, required)
    }
}

/// **Is this an ask to be PAID?**
///
/// A creditor countersigning a settlement or a cure has nothing to weigh: the
/// money is coming to them. Every archetype says yes to this, whatever else it
/// does, because a strategy that refused it would strand its own counterparty
/// inside a default the ledger then reports as theirs — and every figure about
/// that member afterwards would be about the refusal rather than about them.
pub fn is_payment_to_me(ask: &Ask) -> bool {
    matches!(ask.role, Role::Creditor)
        && matches!(ask.intent, Intent::Settle { .. } | Intent::Cure { .. } | Intent::Extend { .. })
}

/// The contract id an intent's transaction would book, read off
/// `next_contract` before the apply — the only way to name a row a composite
/// created.
pub fn books_a_row(intent: &Intent) -> bool {
    match intent {
        Intent::Lend { .. } | Intent::Sell { .. } => true,
        Intent::As { intent, .. } => books_a_row(intent),
        // A stripped envelope authorises nothing, so it books nothing and is
        // not an attempt at a row — counting it would make every tick a
        // griefer acted in look deadlocked.
        _ => false,
    }
}
