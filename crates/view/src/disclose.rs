//! **What a viewer may see.** The per-viewer rules the node's read views
//! apply — which amounts are exact and which are bucketed, which counterparties
//! are named, which relationship and bond fields exist at all — and the JSON
//! each view is built as.
//!
//! One implementation, because a second one drifts: the node serves these
//! shapes (`just view-shape-check` pins what it serves) and a simulated member
//! reads the same ones, so the blindness a member has in production is the
//! blindness it has in a simulation by construction rather than by agreement.
//! What stays with the node is what only a node has: paging over a lock, the
//! capacity cache, proofs, the mempool.

use serde_json::{json, Value};

use edet_state::types::{ContractStatus, MemberStatus, ParamKey, Party, ProposalKind};

use crate::hex32;
use crate::pending::{PendingEntry, PendingPool};

pub fn status_str(s: MemberStatus) -> &'static str {
    match s {
        MemberStatus::Active => "active",
        MemberStatus::Suspended => "suspended",
        MemberStatus::Exited => "exited",
    }
}

pub fn contract_status_str(s: ContractStatus) -> &'static str {
    match s {
        ContractStatus::Active => "active",
        ContractStatus::Transferred => "transferred",
        ContractStatus::Settled => "settled",
        ContractStatus::Expired => "expired",
        ContractStatus::Cured => "cured",
    }
}

/// The `SealAmounts` charter policy (governable, `params.seal_amounts`),
/// finally consulted somewhere. When armed (>= 0.5), read views replace a
/// precise denomination amount with its power-of-two magnitude bucket
/// (`edet_kernel::cascade::pow2_bucket`) instead of the exact figure — e.g.
/// an outstanding balance of 700 reads as 1024, not 700 — for whichever
/// caller doesn't qualify for the exact figure. `visible_amount` (below) is
/// the per-viewer entry point every call site now uses; this function is
/// its "otherwise" case and the no-viewer (`viewer_is_party == false`)
/// behavior every read produced before per-viewer visibility landed.
pub fn seal_amount(st: &edet_state::State, x: f64) -> f64 {
    if st.params.seal_amounts >= 0.5 {
        edet_kernel::cascade::pow2_bucket(x)
    } else {
        x
    }
}

/// Per-viewer amount visibility: exact for a party to the record, or for a
/// validator — they already attest to state transitions, so blanket visibility
/// matches their role — and bucketed for everybody else. A call site with no
/// resolved identity to check passes `false` and gets the sealed reading.
pub fn visible_amount(st: &edet_state::State, x: f64, viewer_is_party: bool) -> f64 {
    if viewer_is_party {
        x
    } else {
        seal_amount(st, x)
    }
}

/// `visible_amount` for a stored amount. The ledger holds minor units and the
/// wire speaks major ones: THIS is the edge the conversion belongs at, and the
/// only one on the read path.
pub fn visible_minor(st: &edet_state::State, x: u64, viewer_is_party: bool) -> f64 {
    visible_amount(st, edet_state::State::from_minor(x), viewer_is_party)
}

/// the visibility default: a validator sees exact everything (they already attest
/// to state transitions, so blanket visibility matches their role). Shared
/// by every view below that needs to know "is this viewer a validator".
pub fn viewer_is_validator(st: &edet_state::State, viewer: Option<u64>) -> bool {
    viewer.map(|v| st.validators.contains_key(&v)).unwrap_or(false)
}

/// Raw 20 address bytes: SHA-256("edet-addr-v3" ‖ be_len(key) ‖ key),
/// truncated — derived from the account's KEY, and from nothing else.
///
/// **A member id must never appear here.** Ids are dense and assigned per
/// community (`State::new_account` increments a local counter), so id 7 in one
/// community and id 7 in another are different people. An address derived from
/// the id is therefore byte-identical for two strangers in two communities:
/// scan an address in one, paste it in the other, and it resolves — to
/// somebody else. That is not a rejected transaction, it is a valid one
/// against the wrong person, and it is the single worst failure this surface
/// can produce. A previous version of this function did exactly that.
///
/// A key is globally unique and cannot be claimed twice (`key_is_claimed`), so
/// deriving from it makes an address unique across every community there is —
/// and makes the same person the same address in each, which is what lets one
/// scan name a correspondent in both halves of an exchange (§Model). Resolving
/// that address to a LOCAL id is `whois`, per community, which is the only
/// place a local id should ever come from.
///
/// The cost, stated plainly: a guardian rotation changes the account's keys
/// and therefore its address. The old derivation used the admission
/// attestation to survive that, and attestations are gone with admission — so
/// there is no longer anything both immutable and globally unique to hang it
/// on. A member who rotates must be re-scanned, exactly as if they had moved.
///
/// The first key, framed by its length: `keys` is ordered, so this is
/// deterministic, and framing keeps the preimage injective for the day an
/// account's key length is not fixed.
pub fn member_address_bytes(m: &edet_state::types::Member) -> [u8; 20] {
    key_address_bytes(m.keys.first().map(|k| &k[..]).unwrap_or(&[]))
}

/// The same address for a key that has no account yet: what a newcomer's
/// wallet shows before the trade that seats it, and what the account's
/// address will be once it is seated with this key first.
pub fn key_address_bytes(key: &[u8]) -> [u8; 20] {
    let mut bytes = Vec::with_capacity(12 + 8 + key.len());
    bytes.extend_from_slice(b"edet-addr-v3");
    bytes.extend_from_slice(&(key.len() as u64).to_be_bytes());
    bytes.extend_from_slice(key);
    let h = crate::sha256(&bytes);
    let mut out = [0u8; 20];
    out.copy_from_slice(&h[..20]);
    out
}

/// Wallet-like address for a member: `0x` + `member_address_bytes`, hex.
pub fn member_address(m: &edet_state::types::Member) -> String {
    address_hex(&member_address_bytes(m))
}

/// `0x` and the address bytes, hex.
pub fn address_hex(bytes: &[u8; 20]) -> String {
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A member whose capacity the caller did not hold when the row was built: the
/// node computes it off its lock and patches it into the row.
#[derive(Clone, Copy, Debug)]
pub struct PendingCapacity {
    pub id: u64,
    pub full_access: bool,
}

/// One row of `/members`, as `viewer` may see it.
///
/// `cached` answers the capacities its caller already holds; a row whose
/// capacity it does not answer, for a viewer who may see one, comes back
/// without the field and with the `PendingCapacity` the caller still owes it.
/// The default resolution for the risk signals: they stay advisory-visible to
/// any AUTHENTICATED member, not just parties — pricing a stranger's risk
/// before transacting is the whole reason `members` exposes them (see the
/// spec's own note on this row diverging from the rest of the matrix). A
/// fully anonymous/unauthenticated caller sees none of it (these are risk
/// signals, not amounts a pow2 bucket could desensitize, so there is no
/// partial/bucketed middle ground here — see §Standing).
pub fn member_row(
    st: &edet_state::State,
    m: &edet_state::types::Member,
    viewer: Option<u64>,
    cached: &dyn Fn(u64) -> Option<f64>,
) -> (Value, Option<PendingCapacity>) {
    let is_validator = viewer_is_validator(st, viewer);
    let reputation_visible = viewer.is_some();
    let mut pending = None;
    let full_access = is_validator || viewer == Some(m.id);
    let mut entry = serde_json::Map::new();
    entry.insert("id".into(), json!(m.id));
    entry.insert("address".into(), json!(member_address(m)));
    // §Standing's public row is id/address/status. Keys are what a member's
    // wallet needs to name a counterparty as a signer, so they are
    // gated to an authenticated viewer rather than published: an
    // anonymous caller holding a member's public key can claim it as
    // a `signers` entry on `/tx/check`, which is what turned that
    // endpoint into a headroom oracle.
    if viewer.is_some() {
        entry.insert("keys".into(), json!(m.keys.iter().map(hex32).collect::<Vec<_>>()));
    }
    entry.insert("status".into(), json!(status_str(m.status)));
    // Absent means "not known for this read", never zero: the client
    // reads a missing risk input as unknown and holds, which is the
    // answer a member can act on. A zero would read as "nobody backs
    // them".
    if viewer.is_some() {
        if let Some(cap) = cached(m.id) {
            entry.insert("capacity".into(), json!(visible_amount(st, cap, full_access)));
        } else if !matches!(m.status, MemberStatus::Active) {
            // Zero by rule for a member who cannot originate, and no
            // query to run for it.
            entry.insert("capacity".into(), json!(visible_amount(st, 0.0, full_access)));
        } else {
            pending = Some(PendingCapacity { id: m.id, full_access });
        }
    }
    entry.insert("debt".into(), json!(visible_minor(st, m.debt_out, full_access)));
    // Whether this account stands behind others, and for how much.
    // Public, and deliberately so: it is an accepted liability rather
    // than a rank, and somebody deciding whether to trade should be
    // able to see who is carrying the community.
    if let Some(&supply) = st.underwriters.get(&m.id) {
        entry.insert("supply".into(), json!(edet_state::state::State::from_minor(supply)));
    }
    if reputation_visible {
        entry.insert("open_default".into(), json!(m.rep.open_default));
        entry.insert("d_in".into(), json!(m.rep.d_in));
        entry.insert("d_out".into(), json!(m.rep.d_out));
    }
    (Value::Object(entry), pending)
}

/// §Standing's contract-view row: `debtor`/`creditor` (counterparty identity) and
/// the arbitration tribunal are exact only for a party (debtor, creditor, a
/// named arbiter) or a validator — ABSENT otherwise, per the default
/// resolution (hidden, not pseudonymized: bucketing the amount alone still
/// leaks the graph edge if the identity stays attached, per §Model). `viewer =
/// None` (no viewer threaded at all) behaves exactly like a non-party: the
/// node's sealed/unsealed unit tests call this with `None`.
pub fn contract_view(c: &edet_state::types::Contract, st: &edet_state::State, viewer: Option<u64>) -> Value {
    let is_validator = viewer_is_validator(st, viewer);
    let is_party = viewer.map(|v| v == c.debtor || v == c.creditor).unwrap_or(false);
    let full = is_party || is_validator;
    let is_named_arbiter = viewer
        .and_then(|v| c.arb.as_ref().map(|t| t.arbiters.contains(&v)))
        .unwrap_or(false);
    let arb_access = full || is_named_arbiter;

    let mut out = serde_json::Map::new();
    out.insert("id".into(), json!(c.id));
    if full {
        out.insert("debtor".into(), json!(c.debtor));
        out.insert("creditor".into(), json!(c.creditor));
    }
    out.insert("outstanding".into(), json!(visible_minor(st, c.outstanding, full)));
    out.insert("original".into(), json!(visible_minor(st, c.original, full)));
    out.insert("status".into(), json!(contract_status_str(c.status)));
    out.insert("maturity_epoch".into(), json!(c.maturity_epoch));
    out.insert("created_epoch".into(), json!(c.created_epoch));
    // The horizon base, which a transfer and a routed successor inherit where
    // `created_epoch` restarts: a wallet extending an insured claim needs it
    // to say, before the member signs, that a date past
    // `accepted_epoch + insured_horizon_epochs` drops the insurance.
    out.insert("accepted_epoch".into(), json!(c.accepted_epoch));
    // The one fact a creditor most needs about a claim: does the community
    // stand behind it, or am I carrying it alone (§Recourse)? Public, because it is
    // not an amount and hiding it would leave a creditor unable to price the
    // difference the whole tier exists to express.
    out.insert("insured".into(), json!(c.insured));
    if arb_access {
        out.insert(
            "arb".into(),
            json!(c.arb.as_ref().map(|t| json!({
                "arbiters": t.arbiters.iter().copied().collect::<Vec<_>>(),
                "quorum": t.quorum,
                "window_epochs": t.window_epochs,
                "award_cap": t.award_cap,
            }))),
        );
        out.insert("arb_attested".into(), json!(c.arb_attestations.keys().copied().collect::<Vec<_>>()));
        out.insert("arb_awarded".into(), json!(c.arb_awarded));
    }
    Value::Object(out)
}

/// One member, as `viewer` may see them: the node's `/member/:id`.
pub fn member(st: &edet_state::State, id: u64, viewer: Option<u64>) -> Value {
    let Some(m) = st.members.get(&id) else {
        return json!({ "error": "unknown member" });
    };
    let is_validator = viewer_is_validator(st, viewer);
    let full_access = is_validator || viewer == Some(id);
    // The default resolution — see `members`' doc comment on the same
    // rule; applies identically to the single-member detail view.
    let reputation_visible = viewer.is_some();
    // §Standing: guardian/pending_rotation are exact for the member themself, a
    // named guardian, or a validator — hidden (key absent) for everyone else.
    let is_named_guardian = viewer
        .and_then(|v| m.guardian.as_ref().map(|g| g.guardians.contains(&v)))
        .unwrap_or(false);
    let relationship_access = full_access || is_named_guardian;

    // §Standing's contract-view row, enforced by what goes INTO these arrays rather
    // than only by what `contract_view` redacts inside them.
    //
    // `contract_view` correctly drops `debtor`/`creditor` for a non-party —
    // and that was defeated by the name of the enclosing array. An entry in
    // member 7's `owes` array IS the fact "7 is the debtor of contract N", so
    // enumerating `/member/0..N` anonymously and unioning by contract id
    // reconstructed every `(debtor, creditor, amount, maturity)` edge the
    // matrix exists to hide, while `/contracts` correctly hid the same edges.
    // Per-field redaction cannot save a value whose position is the
    // disclosure; the composition has to be gated too.
    //
    // A viewer who is a party to a contract already knows that edge, so their
    // own rows stay visible: this narrows to exactly what §Standing grants.
    let edge_visible =
        |c: &edet_state::types::Contract| full_access || viewer.is_some_and(|v| v == c.debtor || v == c.creditor);
    let owes: Vec<Value> = st
        .contracts
        .values()
        .filter(|c| c.debtor == id && !matches!(c.status, ContractStatus::Settled | ContractStatus::Transferred))
        .filter(|c| edge_visible(c))
        .map(|c| contract_view(c, st, viewer))
        .collect();
    let owed: Vec<Value> = st
        .contracts
        .values()
        .filter(|c| c.creditor == id && !matches!(c.status, ContractStatus::Settled | ContractStatus::Transferred))
        .filter(|c| edge_visible(c))
        .map(|c| contract_view(c, st, viewer))
        .collect();

    let mut out = serde_json::Map::new();
    out.insert("id".into(), json!(m.id));
    out.insert("address".into(), json!(member_address(m)));
    if viewer.is_some() {
        out.insert("keys".into(), json!(m.keys.iter().map(hex32).collect::<Vec<_>>()));
    }
    out.insert("status".into(), json!(status_str(m.status)));
    // **Capacity: what the community has put behind you.** The one number the
    // whole model produces, and the one a client should lead with. Zero for an
    // account nobody has backed — which is not a rejection and must never be
    // presented as one: first trades are uninsured, they settle, and capacity
    // is their residue.
    out.insert("capacity".into(), json!(visible_amount(st, st.capacity_of(id), full_access)));
    // What this member may confer on somebody ELSE: their declared supply if
    // they underwrite, otherwise their own capacity. A different quantity from
    // capacity and not to be conflated with it — capacity is how much the
    // community will carry you, this is how much you may carry others. It
    // costs nothing to give and is not a balance anyone spends.
    out.insert("conferrable".into(), json!(visible_amount(st, st.conferrable(id), full_access)));
    out.insert("debt".into(), json!(visible_minor(st, m.debt_out, full_access)));
    out.insert("is_validator".into(), json!(st.validators.contains_key(&id)));
    out.insert("joined_epoch".into(), json!(m.joined_epoch));
    out.insert("owes".into(), Value::Array(owes));
    out.insert("owed".into(), Value::Array(owed));

    // The underwriter side, public: an accepted liability, not a rank.
    if let Some(&supply) = st.underwriters.get(&id) {
        out.insert(
            "supply".into(),
            json!({
                // What they have undertaken to carry, all of it seated by a
                // ceremony — genesis, or a §Governance amendment. There was an
                // `external` field beside this one, carrying the part of a
                // declaration that came from outside the community as opposed
                // to the part declared against standing the community itself
                // conferred. The second kind cannot be declared at all, so the
                // two figures are one and this is it. A view that keeps a field to preserve a
                // contrast the ledger no longer draws is read as a promise.
                "declared": edet_state::state::State::from_minor(supply),
                // What is drawn through them right now, and therefore the
                // floor a withdrawal may not go below (§Stability).
                "committed": edet_state::state::State::from_minor(st.supply_floor(id)),
            }),
        );
    }

    // **Backers: who has put standing behind this account, and how much.**
    //
    // Public in the same sense the underwriter set is: a stake is a creditor's
    // recorded placement, and the whole model rests on standing being legible.
    // The AMOUNTS follow the visibility rule every other amount does, so a
    // non-party sees the shape of the backing without its size.
    let backers: Vec<Value> = st
        .edges
        .iter()
        .filter(|((_, d), _)| *d == id as usize)
        .map(|((c, _), &w)| {
            json!({
                "member": *c as u64,
                "amount": visible_amount(st, edet_state::state::State::from_minor(w), full_access),
            })
        })
        .collect();
    out.insert("backers".into(), Value::Array(backers));

    if reputation_visible {
        // What is left of reputation once standing is a cut: whether this
        // account is currently in default, and the advisory velocity counters.
        // Settled volume, per-creditor evidence and underwriting yield were
        // all proxies for "how much does the community back this account", and
        // `capacity` answers that directly.
        out.insert("open_default".into(), json!(m.rep.open_default));
        out.insert("d_in".into(), json!(m.rep.d_in));
        out.insert("d_out".into(), json!(m.rep.d_out));
    }
    if relationship_access {
        out.insert(
            "guardian".into(),
            json!(m.guardian.as_ref().map(|g| json!({
                "guardians": g.guardians.iter().copied().collect::<Vec<_>>(),
                "threshold": g.threshold,
                "veto_window_epochs": g.veto_window_epochs,
            }))),
        );
        out.insert(
            "pending_rotation".into(),
            json!(m.pending_rotation.as_ref().map(|p| json!({
                "opened_epoch": p.opened_epoch,
            }))),
        );
    }
    // The waterfall graph (§Standing, §Standing): exact only for the member themself or a validator, ABSENT
    // otherwise. A finer per-entry disclosure (a bond's named counterparty,
    // or a listed beneficiary/supporter, seeing just their own entry even
    // without full_access — §Standing's table allows this reading) is a plausible
    // refinement the design doc leaves open; this lands the simpler
    // whole-family gate the task's "make ABSENT... when the viewer does not
    // qualify" instruction calls for, all-or-nothing per field family.
    if full_access {
        // Operation bonds: this member's own headroom, reserved against its
        // own traffic and returned on schedule. Never collected, so no balance
        // moves and nobody is credited for another member's writes.
        //
        // Behind `full_access` with the rest of the financial family: how
        // close a member is to its write ceiling is exactly as revealing as
        // its debt. The client needs it to warn BEFORE a member hits the
        // ceiling, which is the difference between a wallet that explains a
        // refusal and one that merely reports it.
        out.insert(
            "operation_bond".into(),
            json!({
                "encumbered": visible_minor(st, m.bond_enc(), full_access),
                "headroom": visible_amount(st, st.bond_headroom(id), full_access),
                "unit": visible_amount(st, st.params.bond_unit(), full_access),
                // `bond::free_remaining`, never `allowance - used`: the gate
                // grants no allowance at all to a member with nothing to lose
                // (a per-key allowance is the free-signature bound's own defect
                // one layer down), and this line did not know it. Measured
                // measured on the wrong reading: an underwriter withdraws, the member's
                // `conferrable` and `bond_headroom` both fall to 0, every write
                // comes back `ET-BND-001`, and their own wallet reported **32 of
                // 32 free actions** in the "good" tone. The gate and the view
                // answer with one function now.
                "free_remaining": edet_state::bond::free_remaining(st, id),
                // **The qualification itself, because the client was
                // re-deriving it.** `allowanceState` compared the `conferrable`
                // above against dust, which is what the gate did when that line
                // was written — and the gate now reads the same quantity GROSS
                // of live credit, so the comparison would have gone quietly
                // wrong in the one direction that matters: a member whose
                // backers were merely busy would be told nobody had backed
                // them. The ledger answers its own question here.
                "established": edet_state::bond::established(st, id),
                // **What a row costs, beside what the work costs.** A bond is a
                // rate — it refills every epoch — and a row is a stock, so
                // seating one holds a bond unit of this reach for as long as the
                // row exists and nothing gives it back. Divided by `unit` it is
                // how many more newcomers this member's standing carries, which
                // is a promise the ledger keeps rather than a figure beside it.
                "seat_reach": visible_amount(st, st.seat_reach(id), full_access),
                // The one self-act the seat paid for, while it is unspent: a
                // newcomer registers guardians once without a co-signer.
                "seat_slot": m.seat_slot,
                "saturated_epochs": m.bond_saturated_epochs,
            }),
        );
        // **What a listing and an approval do NOT decide**, and the reason this
        // is served at all: the drain across an edge is capped at `NU_DRAIN`
        // times what the pair has staked in one another, with deliberately no
        // floor (§Standing). A pair that has never settled anything drains **zero**,
        // approved or not — the whitelist is not what keeps strangers out, the
        // cut is.
        //
        // The client showed a weight and an "approved" pill and nothing else,
        // so two members could list, approve, see both pills go green, and
        // route exactly nothing between them for ever with no indication why.
        // The quantity the mechanism actually reads was the one quantity the
        // page could not see.
        let drain_cap = |other: u64| {
            let get = |x: u64, y: u64| st.edges.get(&(x as usize, y as usize)).copied().unwrap_or(0);
            let pair = edet_state::state::State::from_minor(get(id, other).saturating_add(get(other, id)));
            visible_amount(st, edet_kernel::constants::NU_DRAIN * pair, full_access)
        };
        let beneficiaries: Vec<Value> = m
            .beneficiaries
            .iter()
            .map(|(&to, &w)| {
                json!({
                    "member": to,
                    "weight": w,
                    // The self entry needs no approval — it is a share, not
                    // a support relationship.
                    "approved": to == id
                        || st.members.get(&to).map(|b| b.approved_supporters.contains(&id)).unwrap_or(false),
                    // Unbounded toward oneself: clearing your own debts is not
                    // a relationship and has no pair to have staked in.
                    "drain_cap": if to == id { Value::Null } else { json!(drain_cap(to)) },
                })
            })
            .collect();
        let supporters: Vec<Value> = m
            .supporters_of
            .iter()
            .map(|&from| {
                json!({
                    "member": from,
                    "weight": st.members.get(&from).and_then(|s| s.beneficiaries.get(&id)).copied().unwrap_or(0.0),
                    "approved": m.approved_supporters.contains(&from),
                    "drain_cap": json!(drain_cap(from)),
                })
            })
            .collect();
        out.insert("beneficiaries".into(), Value::Array(beneficiaries));
        out.insert("supporters".into(), Value::Array(supporters));
    }

    Value::Object(out)
}

pub fn param_key_str(k: ParamKey) -> &'static str {
    match k {
        ParamKey::RiskK => "RiskK",
        ParamKey::SealAmounts => "SealAmounts",
        ParamKey::BondFraction => "BondFraction",
        ParamKey::StakeDecay => "StakeDecay",
        ParamKey::SeedRate => "SeedRate",
        ParamKey::InsuredHorizon => "InsuredHorizon",
    }
}

/// The governed parameters and the bond schedule. Chain metadata: they apply
/// identically to every member (§Standing), so there is nothing here an
/// identity would unlock, and the view takes no viewer.
pub fn params(st: &edet_state::State) -> Value {
    let p = &st.params;
    let governed: Vec<Value> =
        [ParamKey::RiskK, ParamKey::SealAmounts, ParamKey::BondFraction, ParamKey::StakeDecay, ParamKey::SeedRate]
            .into_iter()
            .map(|k| {
                let (min, max) = edet_state::params::Params::safe_range(k);
                json!({
                    "key": param_key_str(k),
                    "value": p.get(k),
                    "min": min,
                    "max": max,
                    "last_amend_epoch": p.last_amend_epoch.get(&k).copied(),
                })
            })
            .collect();
    json!({
        "governed": governed,
        // No admission key of any kind, and none is coming: an account exists
        // as soon as a key signs. A client asking "can I join?" is asking a
        // question with one permanent answer, and what it should ask instead
        // is "what will this community carry for me", which is `/member/:id`'s
        // `capacity` and starts at zero for everybody.
        "v_base": p.v_base,
        "dust": p.dust,
        "stake_decay_per_epoch": {
            "num": p.decay_ratio().0,
            "den": p.decay_ratio().1,
        },
        // The same ratio as a figure a member can act on: the epochs an
        // unrenewed stake takes to halve, and what is left of it after a year.
        // At the genesis ratio that is about thirty epochs and two ten-thousandths,
        // which a community trading on a seasonal rhythm needs to see BEFORE it
        // sizes anything — an edge nobody renews between harvests is gone by
        // the next one.
        "stake_half_life_epochs": decay_half_life(p.decay_ratio()),
        "stake_left_after_a_year": stake_left_after_a_year(p.decay_ratio()),
        "epoch_secs": edet_kernel::constants::EPOCH_SECS,
        "min_maturity_epochs": p.min_maturity_epochs,
        // How far past its acceptance a claim may mature and still be insured
        // — the governed horizon a wallet needs at the point of decision, since
        // a maturity beyond it books uninsured however much capacity carries.
        "insured_horizon_epochs": p.insured_horizon_epochs(),
        "gov_cooldown_epochs": p.gov_cooldown_epochs,
        "last_redenom_epoch": p.last_redenom_epoch,
        // Operation-bond schedule. Public chain metadata like everything
        // else in this view, and deliberately so: `bond_unit` is
        // `BondFraction * v_base`, both of which are ALREADY here, and the
        // allowance/release/forfeit counts are constitutional constants
        // every member is judged by identically. A client needs them to
        // explain what a write reserves BEFORE a member hits the ceiling,
        // which is the difference between a wallet that teaches the
        // mechanism and one that only reports `ET-BND-001` after the fact.
        //
        // The per-member POSITION (how much of the allowance this member has
        // left, what it has encumbered) is a different question and stays
        // behind `full_access` on `/member/:id` — how close someone is to
        // their write ceiling is as revealing as their debt.
        "bond_unit": p.bond_unit(),
        "bond_free_allowance": p.bond_free_allowance,
        "bond_release_epochs": p.bond_release_epochs,
        "bond_forfeit_epochs": p.bond_forfeit_epochs,
    })
}

/// The share of an unrenewed stake left after a year of epochs at `num/den`.
fn stake_left_after_a_year(ratio: (u64, u64)) -> f64 {
    let (num, den) = ratio;
    (num as f64 / den as f64).powi(365)
}

/// Epochs an unrenewed stake takes to halve under `num/den` per epoch.
fn decay_half_life(ratio: (u64, u64)) -> f64 {
    let (num, den) = ratio;
    let r = num as f64 / den as f64;
    if !(r > 0.0 && r < 1.0) {
        return f64::INFINITY;
    }
    (0.5f64).ln() / r.ln()
}

fn proposal_kind_view(k: &ProposalKind) -> Value {
    match k {
        ProposalKind::ParamChange { key, value } => {
            json!({ "type": "param_change", "key": param_key_str(*key), "value": value })
        }
        ProposalKind::Redenominate { num, den } => json!({ "type": "redenominate", "num": num, "den": den }),
        ProposalKind::Suspend { member } => json!({ "type": "suspend", "member": member }),
        ProposalKind::Unsuspend { member } => json!({ "type": "unsuspend", "member": member }),
        ProposalKind::ValidatorPower { member, power } => {
            json!({ "type": "validator_power", "member": member, "power": power })
        }
        // The beneficiary is the proposal's own author and the view says so
        // rather than leaving a client to know it: an amendment that named
        // somebody else would volunteer a member to underwrite, which is why
        // the kind carries no member field at all (§Governance).
        ProposalKind::SeedAmendment { amount } => json!({ "type": "seed_amendment", "amount": amount }),
    }
}

/// Every proposal and who assented. Governance is public by design
/// (§Standing): proposals and who assented are what the community votes on in
/// the open, so the view takes no viewer.
pub fn proposals(st: &edet_state::State) -> Value {
    // The measure the ledger actually applies (§Governance): assent weight is the
    // share of the EXTERNAL seed that has assented, so what a member needs in
    // order to read a proposal's progress is that seed and the part of it
    // behind this proposal.
    //
    // It does NOT serve `active_members`. A client given that renders
    // `ceil(theta_adopt * active_members)` as "N assents needed" — a headcount
    // quorum, which is a rule this ledger does not implement.
    // A view that describes a mechanism the chain does not run is worse than
    // no view: it is read as a promise.
    let mut list: Vec<Value> = st
        .proposals
        .values()
        .map(|p| {
            let assented: u64 = p.assents.iter().filter_map(|id| st.underwriters.get(id)).sum();
            json!({
                "id": p.id,
                "author": p.author,
                "kind": proposal_kind_view(&p.kind),
                "assents": p.assents.iter().copied().collect::<Vec<_>>(),
                "assented_seed": edet_state::state::State::from_minor(assented),
                "enacted": p.enacted,
            })
        })
        .collect();
    list.sort_by_key(|p| std::cmp::Reverse(p["id"].as_u64().unwrap_or(0)));
    json!({
        "proposals": list,
        "external_seed": st.external_seed(),
        "theta_adopt": st.params.theta_adopt,
    })
}

/// A party's inbox: the entries awaiting its signature, and those it signed or
/// opened that wait on others. The gate — who may read whose queue — is the
/// caller's, since only the caller knows who it authenticated.
pub fn pending_for(st: &edet_state::State, pool: &PendingPool, party: Party) -> Value {
    let entry_view = |digest: &[u8; 32], e: &PendingEntry| {
        json!({
            "digest": hex32(digest),
            "tx": serde_json::to_value(&e.tx).unwrap_or(Value::Null),
            // A co-signer's client needs these to reproduce the exact
            // digest it must sign — entries are keyed by digest, so a
            // co-sign carrying a different nonce/window would land in a
            // NEW entry instead of completing this one.
            "nonce": e.nonce,
            "not_after_epoch": e.not_after_epoch,
            "required": e.required,
            "min_sigs": e.min_sigs,
            "initiator": e.initiator,
            // Who opened it: the initiator, or the invited key whose first
            // purchase this is — what the inviter's inbox shows as "from".
            "opener": e.opener,
            "created_secs": e.created_secs,
            "signed_by": pool.signed_parties_of(st, e),
        })
    };
    let (awaiting, mine) = pool.for_party(st, party);
    json!({
        "awaiting_me": awaiting.iter().map(|(d, e)| entry_view(d, e)).collect::<Vec<_>>(),
        "mine": mine.iter().map(|(d, e)| entry_view(d, e)).collect::<Vec<_>>(),
    })
}
