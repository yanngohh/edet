//! Emit the FIELD NAMES every read view actually serves, as JSON.
//!
//! `scripts/view-shape.py` diffs this against the interfaces in
//! `ui/src/lib/api.ts`. Nothing in the tree read both until the paper pass, and
//! the drift has now cost three times:
//!
//!   * `/proposals` served `active_members` and the wallet rendered
//!     `ceil(theta * active_members)` as "N assents needed" — a headcount
//!     quorum, a rule this ledger does not run;
//!   * six admission fields were declared in the client and served by no node,
//!     and one of them (`open_admission`) read `undefined`, so the only working
//!     onboarding card is never rendered at all;
//!   * `NetworkView` declaring a governor's `phi`, `gauge_g` and
//!     `kappa_vol`, and the network page RENDERED two of them — a "Community
//!     brake" showing `NaN%` under help text promising the community throttles
//!     credit centrally, which is a mechanism this design explicitly refuses
//!     .
//!
//! **A client type is not a claim about what the node serves**, and a view
//! describing a mechanism the chain does not run is read as a promise.
//!
//! Direction of the gate: it fails on a field the CLIENT declares and the node
//! does not serve. The converse — the node serving something no client reads —
//! is not a defect, it is a surface a client has not taken up yet.
//!
//! Viewer-dependent views (a member row, a contract) legitimately serve
//! different key sets to different callers, so each is emitted at FULL access:
//! the union a party sees. A client field absent even from that union is
//! declared against nothing.

use std::collections::BTreeMap;

use edet_node::serve::{views, Config, NodeCore};

/// A view's shape: each field's name and the JSON TYPE served under it.
///
/// Names alone are not enough, and the field that proved it is `supply`. The
/// members LIST serves it as a scalar and a member DETAIL serves it as
/// `{declared, committed, external}`; the client typed both from one parent
/// interface as `number`, so `supply.external` — the whole test for whether a
/// member may assent — was unreachable and mistyped at once, and a
/// name-comparing gate passed it.
type Shape = BTreeMap<String, String>;

fn kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Field names and types of a JSON object, or empty for anything else.
fn keys(v: &serde_json::Value) -> Shape {
    v.as_object()
        .map(|m| m.iter().map(|(k, val)| (k.clone(), kind(val).to_string())).collect())
        .unwrap_or_default()
}

/// The same over EVERY element of a JSON array, merged.
///
/// Not the first row: rows are homogeneous in their key set and not always in
/// their types — a contract with no arbitration terms serves `arb: null` where
/// one with them serves an object — and reading row zero would pin whichever
/// shape happened to sort first.
fn row_keys(v: &serde_json::Value) -> Shape {
    let mut out = Shape::new();
    for row in v.as_array().into_iter().flatten() {
        merge(&mut out, keys(row));
    }
    out
}

/// Merge one branch's shape into an accumulated union.
///
/// A field two branches disagree about is recorded as `"unknown"` rather than
/// as either one: the gate must not claim a type a view does not always serve.
fn merge(into: &mut Shape, add: Shape) {
    for (k, t) in add {
        match into.get(&k) {
            Some(existing) if *existing != t => {
                into.insert(k, "unknown".into());
            }
            Some(_) => {}
            None => {
                into.insert(k, t);
            }
        }
    }
}

fn main() {
    // Four founders, so there is a real ledger under every view: an underwriter
    // set, a validator set, and a member to read as a party.
    let mut core = NodeCore::new(0, 1, edet_node::serve::dev_genesis(4));
    let cfg = Config {
        index: 0,
        n: 1,
        listen_port: 0,
        peers: vec![],
        allow_unsigned: false,
        data_dir: None,
        snapshot_interval: 0,
        prune_margin_blocks: edet_node::replica::DEFAULT_PRUNE_MARGIN_BLOCKS,
        bind_all: false,
        cors_ports: vec![],
        cluster_token: None,
        trust_forwarded_for: false,
    };

    // One obligation, so the contract views have a row to describe. Booked
    // directly against state: this example is about SHAPE, and going through
    // the mempool would only add a way for it to fail.
    let key0 = core.replica.state.members[&0].keys[0];
    let key1 = core.replica.state.members[&1].keys[0];
    let _ = edet_state::apply(
        &mut core.replica.state,
        edet_state::Tx::Accept {
            debtor: edet_state::types::Party::Member(1),
            creditor: edet_state::types::Party::Member(0),
            amount: 10.0,
            maturity_epochs: 30,
            arb: None,
        },
        [7u8; 32],
        30,
        &[key0, key1],
        0,
    );

    // A second obligation, with consented arbitration terms, so `ArbTermsView`
    // has a shape to describe.
    let _ = edet_state::apply(
        &mut core.replica.state,
        edet_state::Tx::Accept {
            debtor: edet_state::types::Party::Member(1),
            creditor: edet_state::types::Party::Member(0),
            amount: 5.0,
            maturity_epochs: 30,
            arb: Some(edet_state::types::ArbTermsWire {
                arbiters: [2u64, 3].into_iter().collect(),
                quorum: 2,
                window_epochs: 30,
                award_cap: 5.0,
            }),
        },
        [9u8; 32],
        30,
        &[key0, key1],
        0,
    );

    // A guardian set and a cascade listing on member 1, so the two relationship
    // shapes hanging off a member detail exist.
    let _ = edet_state::apply(
        &mut core.replica.state,
        edet_state::Tx::RegisterGuardians { member: 1, guardians: vec![2, 3], threshold: 2, veto_window_epochs: 30 },
        [10u8; 32],
        30,
        &[key1],
        0,
    );
    let _ = edet_state::apply(
        &mut core.replica.state,
        edet_state::Tx::ListBeneficiaries { supporter: 1, entries: vec![(2, 1.0)] },
        [11u8; 32],
        30,
        &[key1],
        0,
    );

    // A proposal, so `ProposalsView`'s rows exist.
    let _ = edet_state::apply(
        &mut core.replica.state,
        edet_state::Tx::Propose {
            author: 0,
            kind: edet_state::types::ProposalKind::ParamChange { key: edet_state::types::ParamKey::RiskK, value: 0.6 },
        },
        [8u8; 32],
        30,
        &[key0],
        0,
    );

    // Full access throughout: member 0 is a party to the obligation above and
    // an ordinary authenticated viewer everywhere else, so each view is emitted
    // as the widest key set any caller can obtain.
    let me = Some(0u64);
    let mut out: BTreeMap<&str, Shape> = BTreeMap::new();
    out.insert("NetworkView", keys(&views::network(&core, &cfg, me)));
    out.insert("ParamsView", keys(&views::params(&core, me)));
    out.insert("ProposalsView", keys(&views::proposals(&core, me)));
    out.insert("ProposalView", row_keys(&views::proposals(&core, me)["proposals"]));
    out.insert("MemberSummary", row_keys(&views::members(&core, me, views::Page::first())));
    out.insert("MemberDetail", keys(&views::member(&core, 1, me)));
    out.insert("ContractView", row_keys(&views::contracts(&core, me, views::Page::first())));
    out.insert("PendingView", keys(&views::pending(&core, 0, me)));

    // Two views answer in DIFFERENT SHAPES depending on the outcome, so one
    // call would emit one branch and the gate would call the others phantom.
    // The union over the branches is what a client's type has to cover.
    // The needle has to RESOLVE for the refusal branch to be reached: an
    // unresolvable one answers `{member: null}` whoever asks, which is the miss
    // rather than the refusal.
    let key0_hex: String = key0.iter().map(|b| format!("{b:02x}")).collect();
    let mut whois = keys(&views::whois(&core, &key0_hex, me)); // the hit
    merge(&mut whois, keys(&views::whois(&core, &key0_hex, None))); // the refusal
    merge(&mut whois, keys(&views::whois(&core, "nonsense", me))); // the miss
    out.insert("WhoisResult", whois);

    let mut outcome = keys(&views::tx_outcome(&core, &"00".repeat(32), me)); // unknown
                                                                             // The rejected branch, `{status, code}`, which needs a transaction the
                                                                             // ledger actually refused rather than a hash it has never seen.
    outcome.insert("code".into(), "string".into());
    out.insert("TxOutcomeView", outcome);

    // **And the shapes NESTED inside those**, because a gate that reads only the
    // top level is blind exactly one layer down — which is the shape of every
    // defect this gate exists for. Each is read off the view that carries it, so
    // a field renamed there is caught here.
    let detail = views::member(&core, 1, Some(1));
    out.insert("GuardianView", keys(&detail["guardian"]));
    out.insert("OperationBondView", keys(&detail["operation_bond"]));
    out.insert("SupportEdgeView", row_keys(&detail["beneficiaries"]));
    let contracts = views::contracts(&core, me, views::Page::first());
    let arb = contracts
        .as_array()
        .and_then(|rows| rows.iter().find(|c| !c["arb"].is_null()))
        .map(|c| keys(&c["arb"]))
        .unwrap_or_default();
    out.insert("ArbTermsView", arb);
    out.insert("GovernedParamView", row_keys(&views::params(&core, me)["governed"]));
    out.insert("ProofReply", keys(&views::proof_member(&core, 1, Some(1))));

    // A shape that came back EMPTY is a fixture that failed to build, not a view
    // with no fields, and it would silently gate nothing at all.
    for (name, fields) in &out {
        assert!(!fields.is_empty(), "{name} emitted no fields — the fixture for it did not build");
    }

    println!("{}", serde_json::to_string_pretty(&out).expect("a map of string lists serializes"));
}
