//! **What a person can do, as it is written to the tape.**
//!
//! A tool call a person makes during a day is checked at once against a copy of
//! the world and kept as an [`Act`]; when the day ends, its acts are applied to
//! the world in order and written, with what each came to, in one event. A day
//! that fails applies nothing. Replaying the tape applies the same acts in the
//! same order and must come to the same results.
//!
//! **The acting person is never in an act.** It is the day's, supplied by the
//! dispatcher from the session that made the call, so no act can name somebody
//! else as the one acting.

use serde::{Deserialize, Serialize};

use edet_state::types::{ContractId, Key, MemberId, Party, ProposalId, ProposalKind};
use edet_swarm::intent::{AgentRef, Intent};

/// One side of a trade: a member, a person of the town who has no account yet
/// and is named by address, or a stranger whose first trade this is. A
/// newcomer is named by the `n`-th key the person introducing them mints; a
/// person is named by their index, and the world supplies their key.
///
/// **A neighbour named by address is the same seating as a minted key.**
/// Pilots 4 to 6 had only the mint: `"new"` opened a fresh household, and an
/// address with no row was answered "that person has no account yet" while
/// the note said a trade recorded with them would open one. Every seat in
/// those runs was a stranger the offer created, and not one of pilot-6's 42
/// account-less townsfolk was seated in seventy days — members' notes named
/// one 219 times, and 75 offers to one were refused at the tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Member(MemberId),
    Newcomer { owner: usize, n: u32 },
    Person(usize),
}

impl Side {
    fn agent_ref(self, key_of: &dyn Fn(usize) -> Key) -> AgentRef {
        match self {
            Side::Member(id) => AgentRef::Member(id),
            Side::Newcomer { owner, n } => AgentRef::Fresh { owner, n },
            Side::Person(i) => AgentRef::Key(key_of(i)),
        }
    }
}

/// The member alphabet: what a wallet lets a member ask the ledger for. The
/// swarm's harness moves — a dud, a stripped envelope, a replay, acting as
/// another member — are not in it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ask {
    Lend { creditor: Side, debtor: Side, amount_minor: u64, term: u64 },
    Sell { seller: Side, buyer: Side, amount_minor: u64, term: u64 },
    Settle { contract: ContractId, amount_minor: u64 },
    Cure { contract: ContractId, amount_minor: u64 },
    Extend { contract: ContractId, new_maturity_epoch: u64 },
    Transfer { contract: ContractId, new_debtor: MemberId },
    Declare { supply_minor: u64 },
    Exit,
    Propose { proposal: ProposalKind },
    Assent { proposal: ProposalId },
    ListBeneficiaries { entries: Vec<(MemberId, f64)> },
    ApproveSupporter { supporter: MemberId, approved: bool },
    RegisterGuardians { guardians: Vec<MemberId>, threshold: u32, veto_window_epochs: u64 },
}

impl Ask {
    /// The swarm intent `compose` builds the envelope from. `key_of` is the
    /// world's answer for a person named by index, which an act does not carry.
    pub fn intent(&self, key_of: &dyn Fn(usize) -> Key) -> Intent {
        match self.clone() {
            Ask::Lend { creditor, debtor, amount_minor, term } => Intent::Lend {
                creditor: creditor.agent_ref(key_of),
                debtor: debtor.agent_ref(key_of),
                amount_minor,
                term,
                arb: None,
            },
            Ask::Sell { seller, buyer, amount_minor, term } => {
                Intent::Sell { seller: seller.agent_ref(key_of), buyer: buyer.agent_ref(key_of), amount_minor, term }
            }
            Ask::Settle { contract, amount_minor } => Intent::Settle { contract, amount_minor },
            Ask::Cure { contract, amount_minor } => Intent::Cure { contract, amount_minor },
            Ask::Extend { contract, new_maturity_epoch } => Intent::Extend { contract, new_maturity_epoch },
            Ask::Transfer { contract, new_debtor } => Intent::Transfer { contract, new_debtor },
            Ask::Declare { supply_minor } => Intent::Declare { supply_minor },
            Ask::Exit => Intent::Exit,
            Ask::Propose { proposal } => Intent::Propose(proposal),
            Ask::Assent { proposal } => Intent::Assent(proposal),
            Ask::ListBeneficiaries { entries } => Intent::ListBeneficiaries(entries),
            Ask::ApproveSupporter { supporter, approved } => Intent::ApproveSupporter { supporter, approved },
            Ask::RegisterGuardians { guardians, threshold, veto_window_epochs } => {
                Intent::RegisterGuardians { guardians, threshold, veto_window_epochs }
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Ask::Lend { .. } => "lend",
            Ask::Sell { .. } => "sell",
            Ask::Settle { .. } => "settle",
            Ask::Cure { .. } => "cure",
            Ask::Extend { .. } => "extend",
            Ask::Transfer { .. } => "transfer",
            Ask::Declare { .. } => "declare_supply",
            Ask::Exit => "exit",
            Ask::Propose { .. } => "propose",
            Ask::Assent { .. } => "assent",
            Ask::ListBeneficiaries { .. } => "list_beneficiaries",
            Ask::ApproveSupporter { .. } => "approve_supporter",
            Ask::RegisterGuardians { .. } => "register_guardians",
        }
    }

    fn sides(&self) -> Option<[Side; 2]> {
        match self {
            Ask::Lend { creditor, debtor, .. } => Some([*creditor, *debtor]),
            Ask::Sell { seller, buyer, .. } => Some([*seller, *buyer]),
            _ => None,
        }
    }

    /// The newcomer this ask introduces, if it names one.
    pub fn newcomer(&self) -> Option<(usize, u32)> {
        self.sides()?.iter().find_map(|s| match s {
            Side::Newcomer { owner, n } => Some((*owner, *n)),
            Side::Member(_) | Side::Person(_) => None,
        })
    }

    /// The person of the town with no account this ask names, if it names one.
    pub fn person(&self) -> Option<usize> {
        self.sides()?.iter().find_map(|s| match s {
            Side::Person(i) => Some(*i),
            Side::Member(_) | Side::Newcomer { .. } => None,
        })
    }
}

/// How a discharge was paid: by delivering something, which only the person's
/// word attests, or in money, which moves a balance the economy keeps. The
/// ledger records the same transition either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaidWith {
    Value,
    Cash,
}

/// A standing instruction: a policy a person leaves, which the world acts on
/// without the person being present.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Instruction {
    /// Offer to settle what is outstanding on this contract the day before it
    /// falls due.
    PayAtMaturity { contract: ContractId, paid_with: PaidWith },
    /// Co-sign any settlement or cure paying me. **No longer offered**: a
    /// payment to you is a receipt you confirm, and a wallet cannot confirm
    /// cash it never saw. Kept so that tapes written when it was offered
    /// still replay.
    AcceptPayments,
    /// Co-sign a loan, a sale or a payment this member offers me, up to an
    /// amount. Nothing that names no amount — a transfer of a debt to me above
    /// all — is accepted by it.
    AcceptFrom { member: MemberId, max_amount_minor: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Open an entry in the pending pool, signed by the person, waiting on the
    /// others it names. `newcomer` is the index the person it introduces takes
    /// when the offer is applied.
    Offer {
        ask: Ask,
        required: Vec<Party>,
        nonce: String,
        not_after_epoch: u64,
        newcomer: Option<usize>,
        paid_with: Option<PaidWith>,
    },
    /// A transaction only the person has to sign, applied at once.
    Solo {
        ask: Ask,
    },
    /// Add the person's signature to an entry waiting on them.
    Sign {
        digest: String,
    },
    Decline {
        digest: String,
    },
    PayCash {
        to: usize,
        amount_minor: u64,
        note: String,
    },
    Post {
        text: String,
    },
    Message {
        to: usize,
        text: String,
    },
    Diary {
        text: String,
    },
    SetInstruction {
        instruction: Instruction,
    },
    RevokeInstruction {
        id: u64,
    },
}

impl Act {
    pub fn name(&self) -> String {
        match self {
            Act::Offer { ask, .. } => format!("offer:{}", ask.name()),
            Act::Solo { ask } => ask.name().to_string(),
            Act::Sign { .. } => "sign".into(),
            Act::Decline { .. } => "decline".into(),
            Act::PayCash { .. } => "pay_cash".into(),
            Act::Post { .. } => "post".into(),
            Act::Message { .. } => "message".into(),
            Act::Diary { .. } => "diary".into(),
            Act::SetInstruction { .. } => "set_instruction".into(),
            Act::RevokeInstruction { .. } => "revoke_instruction".into(),
        }
    }
}

/// What an act came to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ActResult {
    /// An offer now waits in the pool under this digest.
    Opened {
        digest: String,
    },
    /// A signature was added and the entry still waits on others.
    Signed {
        digest: String,
    },
    /// The ledger applied a transaction: what it booked and whom it seated.
    Applied {
        tx_id: String,
        contract: Option<ContractId>,
        seated: Vec<MemberId>,
        cash_moved: u64,
    },
    /// The ledger refused it, by code.
    Refused {
        code: String,
    },
    /// The pool or the world refused it before the ledger saw it.
    Rejected {
        reason: String,
    },
    Declined {
        digest: String,
    },
    Paid,
    Said,
    InstructionSet {
        id: u64,
    },
    InstructionRevoked {
        id: u64,
    },
}

impl ActResult {
    pub fn describe(&self) -> String {
        match self {
            ActResult::Opened { digest } => format!("offer sent, waiting for the others (ref {})", &digest[..12]),
            ActResult::Signed { digest } => format!("signed; still waiting for others (ref {})", &digest[..12]),
            ActResult::Applied { contract, cash_moved, .. } => {
                let mut s = "done".to_string();
                if let Some(c) = contract {
                    s.push_str(&format!(", contract {c}"));
                }
                if *cash_moved > 0 {
                    s.push_str(&format!(", {} in cash paid", crate::fmt_minor(*cash_moved)));
                }
                s
            }
            ActResult::Refused { code } => format!("refused by the ledger: {code}"),
            ActResult::Rejected { reason } => format!("not possible: {reason}"),
            ActResult::Declined { digest } => format!("declined (ref {})", &digest[..12]),
            ActResult::Paid => "paid".into(),
            ActResult::Said => "done".into(),
            ActResult::InstructionSet { id } => format!("instruction {id} set"),
            ActResult::InstructionRevoked { id } => format!("instruction {id} revoked"),
        }
    }
}
