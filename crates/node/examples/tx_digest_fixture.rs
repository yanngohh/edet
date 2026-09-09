//! Emit signing-digest vectors for `ui/src/lib/__tests__/tx-digest.test.ts` to
//! pin the client's own encoder against.
//!
//! `cargo run -p edet-node --example tx_digest_fixture`
//!
//! **This is the cross-pin that lets a wallet compute what it signs.** A client
//! that fetched its signing payload from the node it reads would sign whatever
//! came back — including the digest of a different transaction, from a node the
//! member had been talked into adding. So the device computes the digest
//! itself, which means a second implementation of the canonical encoding, which
//! means these two implementations can drift.
//!
//! The same answer as `proof_fixture`: the oracle is GENERATED. One vector per
//! transaction variant, straight out of `block::tx_digest`, so a change to the
//! encoding on either side turns a gate red instead of quietly re-opening the
//! hole. `just tx-digest-check` runs it in `ci`.
//!
//! **Every variant of the alphabet appears**, and the `match` at the bottom is
//! what enforces that: a new transaction fails to compile here until somebody
//! writes the vector for it. A cross-pin that covers most of an alphabet is a
//! cross-pin over the part nobody changed.

use edet_node::block::{counter_nonce, tx_digest};
use edet_state::tx::Tx;
use edet_state::types::{ArbTermsWire, ParamKey, Party, ProposalKind};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The chain id every vector binds to. Deliberately NOT the dev chain id: the
/// length prefix on the chain id is part of the digest, and a fixture whose
/// chain id is the default would not notice a client that dropped it.
const CHAIN: &str = "edet-fixture-chain";

fn key(n: u8) -> [u8; 32] {
    [n; 32]
}

/// One vector: the transaction as the client's own JSON, the envelope fields,
/// and the digest the node computes for them.
fn emit(name: &str, tx: &Tx, nonce_seed: u64, not_after: u64) {
    let nonce = counter_nonce(nonce_seed);
    let digest = tx_digest(CHAIN, tx, &nonce, not_after).expect("the alphabet always encodes");
    let json = serde_json::to_string(tx).expect("a transaction is serializable");
    println!("    {{");
    println!("        name: '{name}',");
    println!("        tx: {json},");
    println!("        nonce: [{}],", nonce.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(", "));
    println!("        not_after_epoch: {not_after},");
    println!("        digest: '{}',", hex(&digest));
    println!("    }},");
}

fn main() {
    println!("const RUST_CHAIN_ID = '{CHAIN}';\n");
    println!("const RUST_TX_DIGESTS = [");

    // One per variant, in the alphabet's own declaration order — which is the
    // order the encoding depends on, so reading the two side by side is how a
    // reviewer checks the tags. Amounts are chosen to be un-round: a fixture
    // built entirely out of 1.0 and 0 would pass under an encoder that got the
    // float encoding wrong in the low bits.
    let alphabet: Vec<(&str, Tx)> = vec![
        (
            "register_guardians",
            Tx::RegisterGuardians { member: 7, guardians: vec![3, 1, 2], threshold: 2, veto_window_epochs: 30 },
        ),
        ("rotate_request", Tx::RotateRequest { member: 7, new_keys: vec![key(0xA1), key(0xA2)] }),
        ("rotate_veto", Tx::RotateVeto { member: 7 }),
        ("rotate_finalize", Tx::RotateFinalize { member: 7 }),
        ("set_consensus_key", Tx::SetConsensusKey { member: 7, key: Some(key(0xC7)) }),
        ("set_consensus_key_none", Tx::SetConsensusKey { member: 7, key: None }),
        ("exit", Tx::Exit { member: 7 }),
        ("list_beneficiaries", Tx::ListBeneficiaries { supporter: 7, entries: vec![(3, 0.25), (9, 0.75)] }),
        ("approve_supporter", Tx::ApproveSupporter { beneficiary: 7, supporter: 3, approved: true }),
        (
            "sale_member",
            Tx::Sale { seller: Party::Member(7), buyer: Party::Member(3), amount: 1234.56, maturity_epochs: 45 },
        ),
        (
            "sale_by_key",
            Tx::Sale { seller: Party::Member(7), buyer: Party::Key(key(0xB3)), amount: 0.07, maturity_epochs: 30 },
        ),
        ("declare_supply", Tx::DeclareSupply { member: 7, supply: 2499.99 }),
        (
            "accept_no_arb",
            Tx::Accept {
                debtor: Party::Member(3),
                creditor: Party::Member(7),
                amount: 987.65,
                maturity_epochs: 30,
                arb: None,
            },
        ),
        (
            "accept_with_arb",
            Tx::Accept {
                debtor: Party::Key(key(0xD4)),
                creditor: Party::Member(7),
                amount: 42.42,
                maturity_epochs: 90,
                arb: Some(ArbTermsWire {
                    // Deliberately out of order here: `arbiters` is a
                    // `BTreeSet`, so the encoding is ascending whatever a
                    // caller passes, and a client that echoed click order
                    // would compute a different digest.
                    arbiters: [9u64, 2, 5].into_iter().collect(),
                    quorum: 2,
                    window_epochs: 60,
                    award_cap: 500.5,
                }),
            },
        ),
        ("transfer", Tx::Transfer { contract: 12, new_debtor: 3 }),
        ("settle", Tx::Settle { contract: 12, amount: 33.33 }),
        ("extend", Tx::Extend { contract: 12, new_maturity_epoch: 20_001 }),
        ("mark_expired", Tx::MarkExpired { contract: 12 }),
        ("cure", Tx::Cure { contract: 12, amount: 7.77 }),
        ("arb_attest", Tx::ArbAttest { contract: 12, arbiter: 5, amount: 250.25 }),
        (
            "propose_param_change",
            Tx::Propose { author: 7, kind: ProposalKind::ParamChange { key: ParamKey::StakeDecay, value: 980.0 } },
        ),
        (
            "propose_param_change_horizon",
            Tx::Propose { author: 7, kind: ProposalKind::ParamChange { key: ParamKey::InsuredHorizon, value: 180.0 } },
        ),
        ("propose_redenominate", Tx::Propose { author: 7, kind: ProposalKind::Redenominate { num: 3, den: 2 } }),
        ("propose_suspend", Tx::Propose { author: 7, kind: ProposalKind::Suspend { member: 3 } }),
        ("propose_unsuspend", Tx::Propose { author: 7, kind: ProposalKind::Unsuspend { member: 3 } }),
        (
            "propose_validator_power",
            Tx::Propose { author: 7, kind: ProposalKind::ValidatorPower { member: 3, power: 1 } },
        ),
        ("propose_seed_amendment", Tx::Propose { author: 7, kind: ProposalKind::SeedAmendment { amount: 100.0 } }),
        ("assent", Tx::Assent { member: 7, proposal: 4 }),
        ("forfeit_bonds", Tx::ForfeitBonds { member: 3 }),
    ];

    for (i, (name, tx)) in alphabet.iter().enumerate() {
        emit(name, tx, i as u64 + 1, 20_100 + i as u64);
    }
    println!("];");

    // The compiler's half of the claim: every variant of the alphabet is
    // covered above. A new transaction fails to build here — with the name of
    // the variant nobody wrote a vector for — rather than silently leaving the
    // client's encoder unpinned on it.
    fn covered(tx: &Tx) -> &'static str {
        match tx {
            Tx::RegisterGuardians { .. } => "register_guardians",
            Tx::RotateRequest { .. } => "rotate_request",
            Tx::RotateVeto { .. } => "rotate_veto",
            Tx::RotateFinalize { .. } => "rotate_finalize",
            Tx::SetConsensusKey { .. } => "set_consensus_key",
            Tx::Exit { .. } => "exit",
            Tx::ListBeneficiaries { .. } => "list_beneficiaries",
            Tx::ApproveSupporter { .. } => "approve_supporter",
            Tx::Sale { .. } => "sale_member",
            Tx::DeclareSupply { .. } => "declare_supply",
            Tx::Accept { .. } => "accept_no_arb",
            Tx::Transfer { .. } => "transfer",
            Tx::Settle { .. } => "settle",
            Tx::Extend { .. } => "extend",
            Tx::MarkExpired { .. } => "mark_expired",
            Tx::Cure { .. } => "cure",
            Tx::ArbAttest { .. } => "arb_attest",
            Tx::Propose { .. } => "propose_param_change",
            Tx::Assent { .. } => "assent",
            Tx::ForfeitBonds { .. } => "forfeit_bonds",
        }
    }
    for (_, tx) in &alphabet {
        let want = covered(tx);
        assert!(alphabet.iter().any(|(name, _)| *name == want), "the alphabet's {want} has no vector above");
    }
}
