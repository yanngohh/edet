//! State views as JSON, shared by the HTTP handlers and the Tauri IPC
//! commands so both surfaces return identical shapes.

use serde_json::{json, Value};

use edet_state::types::{ContractStatus, MemberStatus, ParamKey, Party, ProposalKind};

use super::core::NodeCore;
use super::Config;

/// Hard cap on how many entries any single view (`members`, `contracts`)
/// serializes into one response, and the default page size.
///
/// `/members` and `/contracts` would otherwise walk the ENTIRE replicated set
/// with no bound, so response cost would grow linearly with community size —
/// exactly the shape that gets worse as a deployment succeeds. `/members` also
/// answers a max-flow per row it serves, which is why `read_cost` prices it
/// against this number rather than against one request.
pub const MAX_VIEW_ITEMS: usize = 500;

/// Which page of a listing the caller asked for.
///
/// A CURSOR rather than an offset, and the difference is not stylistic: rows
/// are seated and retired between two reads, so an offset walk over a moving
/// `BTreeMap` skips rows and repeats others. `after` is the last id the caller
/// already holds, and the next page starts strictly past it, so a row seated
/// behind the cursor is missed and nothing is ever served twice.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
pub struct Page {
    /// The last id already served. The page begins strictly after it.
    pub after: Option<u64>,
    /// How many rows to serve, clamped to `MAX_VIEW_ITEMS`.
    pub limit: Option<usize>,
}

impl Page {
    /// The first page, unbounded — what a caller who asked for nothing gets.
    pub fn first() -> Self {
        Page::default()
    }

    fn take(&self) -> usize {
        self.limit.unwrap_or(MAX_VIEW_ITEMS).clamp(1, MAX_VIEW_ITEMS)
    }

    /// The `BTreeMap` range this page covers.
    fn range(&self) -> (std::ops::Bound<u64>, std::ops::Bound<u64>) {
        use std::ops::Bound::{Excluded, Unbounded};
        (self.after.map(Excluded).unwrap_or(Unbounded), Unbounded)
    }
}

/// Wrap a page of rows in the listing envelope.
///
/// A bare array only for a caller who asked for no page and got everything —
/// the shape every existing consumer parses. Anything else carries the
/// envelope, with `next` present exactly when another page exists, so a client
/// can walk without guessing and a lying node cannot make it loop for ever by
/// omitting the field.
fn paged(key: &str, rows: Vec<Value>, total: usize, page: Page, next: Option<u64>) -> Value {
    if page.after.is_none() && next.is_none() && page.limit.is_none() {
        return Value::Array(rows);
    }
    let mut out = serde_json::Map::new();
    out.insert(key.to_string(), Value::Array(rows));
    out.insert("truncated".into(), json!(next.is_some()));
    out.insert("total".into(), json!(total));
    if let Some(n) = next {
        out.insert("next".into(), json!(n));
    }
    Value::Object(out)
}

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
fn seal_amount(st: &edet_state::State, x: f64) -> f64 {
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
fn visible_amount(st: &edet_state::State, x: f64, viewer_is_party: bool) -> f64 {
    if viewer_is_party {
        x
    } else {
        seal_amount(st, x)
    }
}

/// `visible_amount` for a stored amount. The ledger holds minor units and the
/// wire speaks major ones: THIS is the edge the conversion belongs at, and the
/// only one on the read path.
fn visible_minor(st: &edet_state::State, x: u64, viewer_is_party: bool) -> f64 {
    visible_amount(st, edet_state::State::from_minor(x), viewer_is_party)
}

/// the visibility default: a validator sees exact everything (they already attest
/// to state transitions, so blanket visibility matches their role). Shared
/// by every view below that needs to know "is this viewer a validator".
fn viewer_is_validator(st: &edet_state::State, viewer: Option<u64>) -> bool {
    viewer.map(|v| st.validators.contains_key(&v)).unwrap_or(false)
}

/// Every read view takes a `viewer`, and most of them now act on it —
/// per-field visibility, `whois`'s gate, `pending`'s viewer-must-be-the-member
/// rule (§Standing's matrix). A handful legitimately do not, and say so at the
/// `let _ = viewer` where they ignore it: what they return is public by
/// design, so there is nothing for an identity to unlock.
///
/// Some views DO consult it, and a shared explanation that claims otherwise is
/// exactly what goes stale unnoticed while three other comments point at it.
///
/// `head` is one of the public ones: chain height, epoch and commit count
/// are the same facts for everybody.
pub fn head(core: &NodeCore, viewer: Option<u64>) -> Value {
    let _ = viewer;
    json!({
        "index": core.index,
        "height": core.head(),
        "epoch": core.replica.state.epoch,
        "committed": core.committed_count,
        // The state commitment at this height, so a client can ask two nodes
        // the same question and compare the answers. Every other read is this
        // node's unsigned word for what the ledger says; without a value to
        // cross-check, a member talking to one node has no way to notice it
        // lying, which is a strange property for a BFT system to hand its
        // users. Public by construction: it is in every block header, and it
        // is a hash over salted leaves, so it discloses no record.
        //
        // This is the cross-check, not yet the proof — `root::prove` produces
        // inclusion proofs against exactly this value, and serving them is the
        // remaining half.
        "app_hash": crate::block::hex32(&core.replica.app_hash()),
        // **The community's seat commitment against its seed** — the figure
        // an operator asks for when nobody can onboard. A row is a stock
        // priced by one bond unit of flow held on the seed's reach and never
        // released, so a community seats `Σ supply / unit` rows and then
        // nobody until a ceremony raises the seed: `seats` is how many rows
        // trades have seated, `seat_committed` what those seats hold on the
        // supply arcs, `seat_ceiling_rows` the seed over the unit, and
        // `seat_room_rows` what is left. Aggregate, and public for the reason
        // `members` on `/network` is.
        "seed": core.replica.state.external_seed(),
        "seats": core.replica.state.members.values().filter(|m| m.seat.is_some()).count(),
        "seat_committed": edet_state::State::from_minor(edet_kernel::flow::committed_total(
            &core.replica.state.seat_committed,
        )),
        "seat_ceiling_rows": seat_ceiling_rows(&core.replica.state),
        "seat_room_rows": seat_ceiling_rows(&core.replica.state).saturating_sub(
            edet_kernel::flow::committed_total(&core.replica.state.seat_committed)
                / core.replica.state.params.bond_unit_minor().max(1),
        ),
    })
}

/// How many rows the seed carries: `Σ supply` over the bond unit, in whole
/// rows — the ceiling `State::reserve_seat` reaches and then refuses at.
fn seat_ceiling_rows(st: &edet_state::State) -> u64 {
    let unit = st.params.bond_unit_minor();
    if unit == 0 {
        return 0;
    }
    edet_state::State::to_minor(st.external_seed()) / unit
}

pub fn network(core: &NodeCore, cfg: &Config, viewer: Option<u64>) -> Value {
    let _ = viewer;
    let st = &core.replica.state;
    let active = st.members.values().filter(|m| matches!(m.status, MemberStatus::Active)).count();
    json!({
        "index": core.index,
        "n": core.n,
        "height": core.head(),
        // The digest a wallet signs is bound to the chain id, so a wallet
        // computing its own (`ui/src/lib/txdigest.ts`) reads it from here or
        // from its network declaration rather than guessing `DEV_CHAIN_ID`.
        "chain_id": st.chain_id,
        "epoch": st.epoch,
        "mempool": core.mempool.len(),
        "members": st.members.len(),
        "active_members": active,
        "epoch_secs": edet_kernel::constants::EPOCH_SECS,
        "min_maturity_epochs": st.params.min_maturity_epochs,
        // The insured horizon beside the maturity floor: the two bounds a
        // wallet needs at the point of decision, since a claim maturing past
        // the horizon books uninsured however much capacity carries it.
        "insured_horizon_epochs": st.params.insured_horizon_epochs(),
        "theta_adopt": st.params.theta_adopt,
        "dust": st.params.dust,
        "v_base": st.params.v_base,
        "validators": st.validators.keys().copied().collect::<Vec<_>>(),
        // §Adoption's honest signals.
        //
        // There was a `declared_supply` here beside `external_seed`, and it was
        // an UPPER BOUND rather than a measure: a member whose own capacity came
        // from inside could declare against it, so the declared total inflated
        // geometrically — seed 100, twelve joiners, 204,900 — while what those
        // twelve could owe together stayed at 100. It is gone, and so is the
        // caveat, because the second kind of declaration is gone: every supply
        // arc is ceremony-seated now, so the roll and the seed are one figure.
        "underwriters": st.underwriters.len(),
        "external_seed": st.external_seed(),
        // What is real and drawn: the flow actually carried through the
        // underwriters, which every insured unit crosses exactly once.
        "insured_credit": st.committed_total(),
        // What one more epoch of amendments may still admit (§Governance), so a
        // community planning a ceremony can see the bound before it drafts
        // one rather than after it is refused.
        "seed_headroom": edet_state::seed::headroom(st),
        // Pinned at 1.0 means the ceiling binds and credit is being rationed
        // first-come-first-served.
        "utilisation": st.utilisation(),
        // The refusal count: obligations that fell to the uninsured tier.
        // Derived rather than counted, so it cannot drift from the book —
        // and it is the figure that tells an underwriter their seed is small,
        // where utilisation only tells them it is full.
        "uninsured_obligations": st
            .contracts
            .values()
            .filter(|c| !c.insured)
            .filter(|c| matches!(c.status, ContractStatus::Active | ContractStatus::Expired))
            .count(),
        "peers": cfg.peers,
        "state_hash": crate::block::hex32(&core.replica.app_hash()),
    })
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
pub(crate) fn member_address_bytes(m: &edet_state::types::Member) -> [u8; 20] {
    let key: &[u8] = m.keys.first().map(|k| &k[..]).unwrap_or(&[]);
    let mut bytes = Vec::with_capacity(12 + 8 + key.len());
    bytes.extend_from_slice(b"edet-addr-v3");
    bytes.extend_from_slice(&(key.len() as u64).to_be_bytes());
    bytes.extend_from_slice(key);
    let h = crate::block::sha256(&bytes);
    let mut out = [0u8; 20];
    out.copy_from_slice(&h[..20]);
    out
}

/// Wallet-like address for a member: `0x` + `member_address_bytes`, hex.
pub fn member_address(m: &edet_state::types::Member) -> String {
    let bytes = member_address_bytes(m);
    let mut s = String::with_capacity(42);
    s.push_str("0x");
    for b in &bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A member whose capacity the cache did not hold at read time: computed off
/// the lock by the handler (`http::members`) and patched into the row.
#[derive(Clone, Copy, Debug)]
pub struct PendingCapacity {
    pub id: u64,
    pub full_access: bool,
}

/// The two halves of a `/members` read: the rows as they stand under the
/// lock, and the capacities still to compute — with the cut's inputs copied
/// out so they can be, off the lock (`super::core::CapacitySnapshot`).
pub struct MembersRead {
    pub value: Value,
    pub pending: Vec<PendingCapacity>,
    pub snapshot: Option<super::core::CapacitySnapshot>,
}

/// The whole read under one lock: the in-process path for callers that hold
/// no HTTP handler to split it across — the tests here. It computes exactly
/// what `http::members` computes and no more.
pub fn members(core: &NodeCore, viewer: Option<u64>, page: Page) -> Value {
    let read = members_read(core, viewer, page);
    let computed: Vec<(u64, f64)> = match &read.snapshot {
        Some(snap) => read
            .pending
            .iter()
            .take(super::core::MAX_COLD_CAPACITY_PER_READ)
            .map(|p| (p.id, snap.capacity_of(p.id)))
            .collect(),
        None => Vec::new(),
    };
    if let Some(snap) = &read.snapshot {
        core.fill_capacity_cache(snap, &computed);
    }
    members_finish(read, &computed)
}

/// Patch the capacities a handler computed into the rows they belong to,
/// sealed the way every other amount in the view is. A pending row the
/// handler did not reach stays ABSENT: not known for this read, never zero.
pub fn members_finish(read: MembersRead, computed: &[(u64, f64)]) -> Value {
    let MembersRead { mut value, pending, snapshot } = read;
    let Some(snap) = snapshot else { return value };
    let rows = match &mut value {
        Value::Array(rows) => rows,
        Value::Object(map) => match map.get_mut("members") {
            Some(Value::Array(rows)) => rows,
            _ => return value,
        },
        _ => return value,
    };
    for &(id, cap) in computed {
        let Some(p) = pending.iter().find(|p| p.id == id) else { continue };
        if let Some(Value::Object(entry)) = rows.iter_mut().find(|r| r["id"].as_u64() == Some(id)) {
            entry.insert("capacity".into(), json!(snap.visible(cap, p.full_access)));
        }
    }
    value
}

/// Phase one of a `/members` read, under the lock: every row, with the
/// capacities the cache already holds, and the list of the ones it does not.
pub fn members_read(core: &NodeCore, viewer: Option<u64>, page: Page) -> MembersRead {
    let st = &core.replica.state;
    let mut pending: Vec<PendingCapacity> = Vec::new();
    let dust = st.params.dust;
    let is_validator = viewer_is_validator(st, viewer);
    // The default resolution: reputation/risk signals (sigma_w, n_s,
    // open_default, trust, tau, brake, d_in/d_out) stay advisory-visible to
    // any AUTHENTICATED member, not just parties — pricing a stranger's risk
    // before transacting is the whole reason `members` exposes them (see the
    // spec's own note on this row diverging from the rest of the matrix). A
    // fully anonymous/unauthenticated caller sees none of it (these are risk
    // signals, not amounts a pow2 bucket could desensitize, so there is no
    // partial/bucketed middle ground here — see §Standing, §Standing).
    let reputation_visible = viewer.is_some();
    let total = st.members.len();
    // One page, in ascending id order. `st.members` is a `BTreeMap`, so
    // `range` already yields that order — the page is a window on it rather
    // than a different sort, which is what keeps it deterministic across
    // replicas.
    // **Capacity is a max-flow per member, and this view serves up to 500 of
    // them.** Three things bound what that costs, because without them one
    // source at the allowed read rate holds the node lock — the same lock a
    // commit needs — continuously, and the validator misses its rounds. Behind
    // a NAT every member shares one bucket, so it is self-inflicted at scale
    // as well.
    //
    // The first is that an ANONYMOUS caller gets no capacity at all. It is a
    // risk signal, and this view already gates the other risk signals on being
    // authenticated (`reputation_visible`); a member's wallet holds a session,
    // so nothing a member does is affected, while an unauthenticated flood
    // costs no queries whatever.
    //
    // The second is the node's cache, which answers every read whose cut
    // inputs have not moved (`core::CapacityCache`). The third is the
    // handler's budget, which bounds the first read after each state-changing
    // block: what it cannot pay for is OMITTED rather than invented, and the
    // caller is charged for what it did spend. And what it does pay for it
    // computes OFF this lock, on a copy of the cut's inputs taken here
    // (`core::CapacitySnapshot`): a free settle moves `reserved` and makes
    // every read cold, so an attacker who trades once a block and reads from
    // a few addresses held the lock a commit needs for most of every second.
    let mut out: Vec<Value> = st
        .members
        .range(page.range())
        .take(page.take())
        .map(|(_, m)| {
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
                entry.insert("keys".into(), json!(m.keys.iter().map(crate::block::hex32).collect::<Vec<_>>()));
            }
            entry.insert("status".into(), json!(status_str(m.status)));
            // Absent means "not known for this read", never zero: the client
            // reads a missing risk input as unknown and holds, which is the
            // answer a member can act on. A zero would read as "nobody backs
            // them".
            if viewer.is_some() {
                if let Some(cap) = core.capacity_cached_only(m.id) {
                    entry.insert("capacity".into(), json!(visible_amount(st, cap, full_access)));
                } else if !matches!(m.status, MemberStatus::Active) {
                    // Zero by rule for a member who cannot originate, and no
                    // query to run for it.
                    entry.insert("capacity".into(), json!(visible_amount(st, 0.0, full_access)));
                } else {
                    pending.push(PendingCapacity { id: m.id, full_access });
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
            let _ = dust;
            Value::Object(entry)
        })
        .collect();
    out.sort_by_key(|m| m["id"].as_u64().unwrap_or(0));
    // `next` is the last id SERVED, and it is present exactly when another row
    // exists past it — asked of the map rather than inferred from the page
    // being full, so a page that ends on the last row says so instead of
    // sending the caller round once more.
    let last = out.last().and_then(|m| m["id"].as_u64());
    let next = last.filter(|&id| {
        st.members
            .range((std::ops::Bound::Excluded(id), std::ops::Bound::Unbounded))
            .next()
            .is_some()
    });
    // The copy is taken only when something is left to compute, and only for
    // a viewer who may see a capacity at all.
    let snapshot = (!pending.is_empty()).then(|| core.capacity_snapshot());
    MembersRead { value: paged("members", out, total, page, next), pending, snapshot }
}

/// §Standing's contract-view row: `debtor`/`creditor` (counterparty identity) and
/// the arbitration tribunal are exact only for a party (debtor, creditor, a
/// named arbiter) or a validator — ABSENT otherwise, per the default
/// resolution (hidden, not pseudonymized: bucketing the amount alone still
/// leaks the graph edge if the identity stays attached, per §Model). `viewer =
/// None` (no viewer threaded at all) behaves exactly like a non-party: the
/// existing sealed/unsealed unit tests below call this with `None` and must
/// keep passing unmodified.
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

pub fn member(core: &NodeCore, id: u64, viewer: Option<u64>) -> Value {
    let st = &core.replica.state;
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
        out.insert("keys".into(), json!(m.keys.iter().map(crate::block::hex32).collect::<Vec<_>>()));
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

pub fn contracts(core: &NodeCore, viewer: Option<u64>, page: Page) -> Value {
    let st = &core.replica.state;
    let total = st.contracts.len();
    // See `members`' matching comment — a window on the `BTreeMap`'s natural
    // ascending-id order, never a different sort.
    let mut v: Vec<Value> = st
        .contracts
        .range(page.range())
        .take(page.take())
        .map(|(_, c)| contract_view(c, st, viewer))
        .collect();
    v.sort_by_key(|c| c["id"].as_u64().unwrap_or(0));
    let last = v.last().and_then(|c| c["id"].as_u64());
    let next = last.filter(|&id| {
        st.contracts
            .range((std::ops::Bound::Excluded(id), std::ops::Bound::Unbounded))
            .next()
            .is_some()
    });
    paged("contracts", v, total, page, next)
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

pub fn params(core: &NodeCore, viewer: Option<u64>) -> Value {
    let _ = viewer; // Chain metadata: the governed parameters and the bond
                    // schedule apply identically to every member (§Standing), so
                    // there is nothing here an identity would unlock.
    let p = &core.replica.state.params;
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

pub fn proposals(core: &NodeCore, viewer: Option<u64>) -> Value {
    let _ = viewer; // Governance is public by design (§Standing): proposals and who
                    // assented are what the community votes on in the open.
    let st = &core.replica.state;
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

/// A record's inclusion proof against this node's current state root.
///
/// P-4: without this a member holds nothing but the node's unsigned word.
/// `/head` publishes the `app_hash` a proof verifies against, so two nodes can
/// be compared; this is the other half — the evidence a member can KEEP, hand
/// to an arbitrator who has never run edet, and check again later against a
/// published root. The design names member-held commitments as the mitigation
/// for a `>2/3` federation rewriting history, and until a member can obtain
/// one that mitigation is a claim rather than a mechanism.
///
/// **Gated exactly as tightly as reading the record it proves.** A proof
/// discloses its leaf VERBATIM — the whole codec-encoded record, amounts
/// unbucketed and counterparties named — so an ungated proof endpoint would
/// hand back through a side door precisely what `/member/:id` and
/// `contract_view` withhold. The rule is therefore the same `full_access` the
/// read uses, not a weaker one: the member themself, a party to the contract,
/// or a validator. That is also enough for the purpose, which is proving
/// things about your OWN position.
///
/// The root and height travel WITH the proof, resolved under the same lock, so
/// a client is never left pairing a proof with a root it raced against.
fn proof_reply(core: &NodeCore, proof: Option<edet_state::root::InclusionProof>) -> Value {
    match proof {
        Some(p) => json!({
            "height": core.head(),
            "app_hash": crate::block::hex32(&core.replica.app_hash()),
            "proof": proof_json(&p),
        }),
        // Distinct from a refusal: the record genuinely is not in this
        // section. Presence is all this format proves — absence is provable
        // against the same leaves (they are key-ordered, each commits its
        // index, and each section commits its leaf count) but is not
        // implemented.
        None => json!({ "proof": Value::Null }),
    }
}

/// The wire shape: hashes and byte strings as hex, section as a lowercase
/// name. Hand-built rather than serde-derived so the client's verifier reads
/// one documented encoding instead of whatever `Vec<u8>` happens to serialise
/// as — the two implementations are byte-for-byte cross-pinned, so the wire
/// format is part of the security argument, not a detail.
fn proof_json(p: &edet_state::root::InclusionProof) -> Value {
    let hexes = |hs: &[[u8; 32]]| hs.iter().map(crate::block::hex32).collect::<Vec<_>>();
    json!({
        "section": section_name(p.section),
        "index": p.index,
        "leaf_count": p.leaf_count,
        "key": hex_bytes(&p.key),
        "value": hex_bytes(&p.value),
        "leaf_salt": crate::block::hex32(&p.leaf_salt),
        "path": hexes(&p.path),
        "section_path": hexes(&p.section_path),
    })
}

fn section_name(s: edet_state::root::Section) -> &'static str {
    use edet_state::root::Section;
    match s {
        Section::Members => "members",
        Section::Contracts => "contracts",
        Section::Proposals => "proposals",
        Section::Validators => "validators",
        Section::Stakes => "stakes",
        Section::Replay => "replay",
        Section::Ledger => "ledger",
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Prove member `id`'s record. `full_access` — the member themself or a
/// validator — mirroring `member`'s own rule for the unbucketed figures this
/// leaf contains.
pub fn proof_member(core: &NodeCore, id: u64, viewer: Option<u64>) -> Value {
    let st = &core.replica.state;
    if !(viewer == Some(id) || viewer_is_validator(st, viewer)) {
        return json!({ "error": "forbidden: a proof discloses the whole record, so it needs the same access the record does" });
    }
    match core.replica.prove_member(id) {
        Ok(p) => proof_reply(core, p),
        Err(e) => json!({ "error": format!("proof unavailable: {e}") }),
    }
}

/// Prove contract `id`'s record. A party or a validator — the same rule
/// `contract_view` applies to the counterparty identities this leaf names.
/// A named arbiter is deliberately NOT enough: `contract_view` grants them the
/// tribunal fields, not the parties' figures, and a proof cannot disclose less
/// than its leaf.
pub fn proof_contract(core: &NodeCore, id: u64, viewer: Option<u64>) -> Value {
    let st = &core.replica.state;
    let party = st
        .contracts
        .get(&id)
        .map(|c| viewer.is_some_and(|v| v == c.debtor || v == c.creditor))
        .unwrap_or(false);
    if !(party || viewer_is_validator(st, viewer)) {
        return json!({ "error": "forbidden: a proof discloses the whole record, so it needs the same access the record does" });
    }
    match core.replica.prove_contract(id) {
        Ok(p) => proof_reply(core, p),
        Err(e) => json!({ "error": format!("proof unavailable: {e}") }),
    }
}

/// Dry-run a transaction against a clone of current state at `now_secs`.
/// Lets a client surface the exact rejection code before submitting —
/// commit-time apply failures are otherwise silent from the client's side.
///
/// Mirrors what the commit path actually does — computes the envelope id
/// against THIS node's `chain_id` and passes it (with the envelope's own
/// `not_after_epoch`) into `apply`, so a dry-run against an already-applied
/// id correctly reports `ET_TX_REPLAY` instead of whatever the transaction's
/// content alone would have produced.
pub fn check_tx(core: &NodeCore, tx: &crate::block::SignedTx, now_secs: u64, viewer: Option<u64>) -> Value {
    // **Who the caller is, decided BEFORE anything is cloned.** The whole
    // ledger is copied to dry-run a transaction on, and an anonymous caller
    // never reaches the dry run — so cloning first hands one unauthenticated
    // request a copy of the entire state, under the node lock, per call. Read
    // off the live state; both reads are cheap map lookups.
    let live = &core.replica.state;
    // The OUTCOME is disclosed only to a party to this transaction (or a
    // validator); the bond quote below is public and always returned.
    //
    // The quote was the only thing this endpoint was careful about, and the
    // oracle was the reply code beside it. `apply` returns `ET-CAP-001`
    // exactly when `amount > (cap - debt) * brake`, so for a non-trial
    // `Accept` naming any member as debtor, `ok`/`code` is a one-bit probe of
    // that member's private headroom — 60 anonymous queries binary-search it
    // to the last decimal, while `/members` was disclosing only its
    // power-of-two bucket. `signers` is caller-supplied and unverified here,
    // so "who this transaction is about" cannot be trusted; what can be is
    // who the CALLER authenticated as. A wallet pre-flighting its own draft
    // names itself among the signers and is unaffected, which is the whole
    // purpose of the endpoint.
    let party = viewer.is_some_and(|v| {
        live.validators.contains_key(&v) || tx.signers.iter().any(|k| live.member_of_key(k) == Some(v))
    });
    // What this transition class reserves, read off the SAME schedule the
    // gate enforces (`bond::bond_multiple`) rather than a table duplicated
    // client-side — a second copy would drift the first time the schedule
    // moved, and a wallet quoting a stale number is worse than one quoting
    // none. Computed before `apply`, which mutates `st`.
    //
    // Deliberately the PUBLIC half only: the class multiple times the unit,
    // both already derivable from `/params` plus the transaction the caller
    // is holding in its hand. NOT the payer's headroom, remaining allowance,
    // or a yes/no affordability verdict — those ride `full_access` on
    // `/member/:id`, and repeating them here would make this a probe for any
    // member's write ceiling.
    //
    // Reported on the rejection paths too — an `ET-BND-001` reply that also
    // carries the amount is what lets the client say how far short it fell.
    let bond = json!({
        "amount": edet_state::bond::bond_multiple(live, &tx.tx) * live.params.bond_unit(),
        "release_epochs": live.params.bond_release_epochs,
    });
    if !party {
        return json!({ "bond": bond });
    }
    let mut st = live.clone();
    st.begin_block(now_secs);
    let Ok(tx_id) = tx.id(&st.chain_id) else {
        return json!({ "ok": false, "code": edet_state::errors::ET_TX_UNDIGESTABLE, "bond": bond });
    };
    match edet_state::apply(&mut st, tx.tx.clone(), tx_id, tx.not_after_epoch, &tx.signers, now_secs) {
        Ok(()) => json!({ "ok": true, "bond": bond }),
        Err(e) => json!({ "ok": false, "code": e.0, "bond": bond }),
    }
}

/// Whatever this replica knows about a submitted transaction, by its
/// content hash: `"ok"` if it committed and applied cleanly,
/// `"rejected"` with the ET code if it committed but was rejected at
/// apply-time (silent from the ingress's point of view otherwise —
/// `/tx` only reports whether it was QUEUED, never whether it later
/// stuck), `"pending"` if it is still queued, uncommitted, or `"unknown"`
/// if none of the above (bad hash, never seen, or evicted from the bounded
/// outcome window).
pub fn tx_outcome(core: &NodeCore, hash_hex: &str, viewer: Option<u64>) -> Value {
    // Keyed by a hash only the submitter can compute ahead of the commit,
    // and answering "ok" / an ET code / "pending" / "unknown" — nothing that
    // identifies a party or an amount. Gating it would cost the wallet its
    // only view of a commit-time rejection and buy no privacy.
    let _ = viewer;
    let Some(h) = crate::block::unhex32(hash_hex) else {
        return json!({ "status": "unknown" });
    };
    if let Some(outcome) = core.tx_outcome(&h) {
        return match outcome {
            Ok(()) => json!({ "status": "ok" }),
            Err(e) => json!({ "status": "rejected", "code": e.0 }),
        };
    }
    if core.tx_pending(&h) {
        return json!({ "status": "pending" });
    }
    json!({ "status": "unknown" })
}

/// The body of a `/tx/digest` request: the caller supplies its own
/// `nonce`/`not_after_epoch` — the digest depends on both, and only the
/// signer can choose them — so the response is the digest for THIS EXACT
/// envelope, not for `tx` alone.
#[derive(serde::Deserialize)]
pub struct TxDigestReq {
    pub tx: edet_state::Tx,
    pub nonce: [u8; 16],
    pub not_after_epoch: u64,
}

/// The signing payload for a transaction envelope, as hex, bound to
/// `chain_id`. **A harness convenience, not a wallet's signing path.** A
/// wallet that signed what a node it reads hands it would be signing on that
/// node's word: a node reached through a `custom` URL can answer with the
/// digest of `Accept { debtor: victim, creditor: attacker, amount: 10000 }`
/// and collect a valid signature, and `/tx/check` is no defence because the
/// same node answers it. So the wallet computes the digest on the device
/// (`ui/src/lib/txdigest.ts`, cross-pinned to `block::tx_digest` by `just
/// tx-digest-check`), and a test forbids any client module from reaching for
/// this route. What is served here serves the e2e harnesses in `ui/scripts/`,
/// which hold no member's seed and drive a node they started themselves.
pub fn tx_digest(chain_id: &str, req: &TxDigestReq) -> Value {
    match crate::block::tx_digest(chain_id, &req.tx, &req.nonce, req.not_after_epoch) {
        Ok(d) => json!({ "digest": crate::block::hex32(&d) }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

/// Multi-party proposals involving `member`: awaiting their signature vs
/// theirs-awaiting-others. Gated (§Standing, §Stability step 3): the path parameter alone
/// is not trust-worthy — a full pending `tx` payload (itself potentially a
/// `Sale`/`Accept` carrying amounts and counterparties) must only reach the
/// authenticated `member` it belongs to, or a validator (the validator carve-out:
/// validators may need visibility into stuck multi-sig proposals for
/// arbitration).
pub fn pending(core: &NodeCore, member: u64, viewer: Option<u64>) -> Value {
    let st = &core.replica.state;
    if viewer != Some(member) && !viewer_is_validator(st, viewer) {
        return json!({ "error": "forbidden: pending queue is visible only to the member or a validator" });
    }
    pending_for(core, Party::Member(member))
}

/// The same queue for a caller who is only a KEY — a device whose first trade
/// has not been assembled yet, so the ledger has no member to name it by
/// (a trade seats the row that names it).
///
/// Gated identically, and the gate is the only thing that can be: the caller
/// must have proved possession of exactly this key. There is no id to compare
/// against, so `ViewerParty` doing the proof IS the authorisation, and a
/// validator's carve-out is kept for the same reason it exists on the member
/// form — a stuck multi-party proposal has to be visible to someone.
pub fn pending_by_key(core: &NodeCore, key: edet_state::types::Key, viewer: Option<Party>) -> Value {
    let st = &core.replica.state;
    let is_validator = matches!(viewer, Some(Party::Member(id)) if st.validators.contains_key(&id));
    if viewer != Some(Party::Key(key)) && !is_validator {
        return json!({ "error": "forbidden: pending queue is visible only to the holder of this key or a validator" });
    }
    // A key the ledger DOES attribute is not addressed here: it has a member
    // id, and serving it by key as well would be a second door onto the same
    // queue with a different gate on it.
    if let Some(id) = st.member_of_key(&key) {
        return json!({ "error": format!("this key is member {id}; read /pending/{id}") });
    }
    pending_for(core, Party::Key(key))
}

fn pending_for(core: &NodeCore, party: Party) -> Value {
    let st = &core.replica.state;
    let entry_view = |digest: &[u8; 32], e: &super::pending::PendingEntry| {
        json!({
            "digest": crate::block::hex32(digest),
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
            "signed_by": core.pending.signed_parties_of(st, e),
        })
    };
    let (awaiting, mine) = core.pending.for_party(st, party);
    json!({
        "awaiting_me": awaiting.iter().map(|(d, e)| entry_view(d, e)).collect::<Vec<_>>(),
        "mine": mine.iter().map(|(d, e)| entry_view(d, e)).collect::<Vec<_>>(),
    })
}

/// Reverse lookup: who is this? Accepts either an Ed25519 public key
/// (64 hex — the restore-from-recovery-phrase flow) or a wallet address
/// (0x + 40 hex — the "pay this address" flow: manual entry or QR scan).
///
/// Gated (§Standing, §Stability step 3). The spec's matrix row for this endpoint reads:
/// "requires the viewer to **already hold the address/pubkey they're asking
/// about**, or be that member, or a validator — an anonymous
/// scan-and-resolve of an arbitrary address is exactly the de-anonymization
/// primitive §Model flags".
///
/// The thing being refused is the ANONYMOUS scan. The first clause — the
/// viewer already holds the needle — is what every ordinary payment depends
/// on: you are handed an address (typed, or scanned off a QR), and turning
/// it into a member id is the whole point of having been given it. That
/// clause went unimplemented, leaving only "be that member, or a validator",
/// and the consequence was not subtle: `AddressInput` resolves the
/// counterparty for every purchase, vouch and admission, so a member who was
/// not a validator could not transact with anyone. It survived because
/// `dev_genesis` seeds every founder as a validator, so no harness could tell
/// the difference. `just e2e` now walks the journey as a real admitted
/// ordinary member and asserts `is_validator == false` before it proves
/// anything, precisely so this class of break cannot hide again.
///
/// "Already holds it" is not provable — presenting an address in the request
/// IS holding it, and no signature can distinguish an address you were given
/// from one you guessed. So the implementable reading is the one the threat
/// model actually cares about: require an authenticated member, and refuse
/// the anonymous caller. Enumeration is bounded by having to know a 160-bit
/// address up front, and by `read_rate_limit` on top.
///
/// This discloses nothing new to a member: the same matrix makes "member
/// existence, `id`, `address`, `status`" visible to EVERYONE, including
/// anonymously, so `/members` already publishes the very mapping this
/// resolves — but truncates at `MAX_VIEW_ITEMS`, which is why scanning that
/// list is not a substitute. The de-anonymization §Model warns about is the
/// composition with financial and relationship fields, and those stay gated
/// where they are, on `full_access`.
///
/// A needle that resolves to no member returns `{"member": null}` for any
/// viewer, anonymous included — that discloses nothing about a real member,
/// and it is what lets a newly created identity poll for its own admission.
pub fn whois(core: &NodeCore, needle: &str, viewer: Option<u64>) -> Value {
    let st = &core.replica.state;
    match resolve_needle(core, st, needle) {
        // `viewer` is `Some` only for an authenticated member: the extractor
        // mints it from a session token or a verified key proof, never from
        // an unverified claim.
        Some((id, addr)) if viewer.is_some() => {
            json!({ "member": id, "address": addr })
        }
        Some(_) => json!({ "error": "forbidden: sign in as a member to resolve an address" }),
        // Deliberately distinguishable from the refusal above, and the review's
        // Low finding on that is declined rather than overlooked. Collapsing
        // the two shapes closes an oracle that discloses nothing `/members`
        // does not already publish to an anonymous caller (id, address,
        // status), and it breaks the one path that has no alternative: a
        // newly created identity polling whether its own admission has
        // landed yet is, by construction, not a member, so it reads exactly
        // this reply — `ui/scripts/e2e-first-contact.mjs` gates that journey.
        // Enumeration still costs a 160-bit address up front plus
        // `read_rate_limit`.
        None => json!({ "member": null }),
    }
}

/// (the paper's §Implementation Part 2): the address path
/// now resolves through `NodeCore`'s cached address index (O(1) amortized,
/// rebuilt only when membership changes) instead of an O(members)
/// SHA-256-per-member scan on every call; the pubkey path already used
/// `State::member_of_key`'s indexed `key_index` and is unchanged.
fn resolve_needle(core: &NodeCore, st: &edet_state::State, needle: &str) -> Option<(u64, String)> {
    let clean = needle.strip_prefix("0x").unwrap_or(needle);
    if let Some(addr) = unhex20(clean) {
        let id = core.resolve_address(&addr)?;
        let m = st.members.get(&id)?;
        return Some((id, member_address(m)));
    }
    let key = crate::block::unhex32(clean)?;
    let id = st.member_of_key(&key)?;
    let m = st.members.get(&id)?;
    Some((id, member_address(m)))
}

fn unhex20(s: &str) -> Option<[u8; 20]> {
    if s.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edet_state::apply;

    /// Genesis + one Accept contract of 30 (within a fresh founder's
    /// starting capacity — see `three_node_cluster_commits_and_agrees` in
    /// `serve::tests` for the same amount), at the given `seal_amounts`
    /// setting.
    fn contract_after_accept(seal_amounts: f64) -> (edet_state::State, edet_state::types::Contract) {
        let mut st = super::super::dev_genesis(5);
        st.params.seal_amounts = seal_amounts;
        st.begin_block(60);
        let debtor_key = st.members[&0].keys[0];
        let creditor_key = st.members[&1].keys[0];
        apply(
            &mut st,
            edet_state::Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 30.0,
                maturity_epochs: 30,
                arb: None,
            },
            [1u8; 32],
            30,
            &[debtor_key, creditor_key],
            60,
        )
        .expect("accept should succeed");
        let c = st.contracts.values().next().cloned().expect("contract created");
        (st, c)
    }

    #[test]
    fn sealed_view_buckets_amounts_and_unsealed_view_reports_them_exactly() {
        let (st_sealed, c_sealed) = contract_after_accept(1.0);
        // No viewer threaded (`None`): the no-viewer path must keep today's
        // sealed-for-everyone behavior (§Stability step 4) — the whole point of
        // this pre-existing test staying green unmodified.
        let sealed = contract_view(&c_sealed, &st_sealed, None);
        assert_eq!(sealed["outstanding"].as_f64(), Some(32.0), "30 must round up to the next power of two");
        assert_eq!(sealed["original"].as_f64(), Some(32.0));

        let (st_open, c_open) = contract_after_accept(0.0);
        let open = contract_view(&c_open, &st_open, None);
        assert_eq!(open["outstanding"].as_f64(), Some(30.0), "unsealed view must report the exact amount");
        assert_eq!(open["original"].as_f64(), Some(30.0));
    }

    #[test]
    fn sealing_threshold_is_the_documented_half() {
        // Below 0.5: still open. At/above: sealed. (Matches `seal_amount`'s
        // `>= 0.5` check, so a governed value mid-transition reads
        // predictably.)
        let (below, c_below) = contract_after_accept(0.49);
        assert_eq!(contract_view(&c_below, &below, None)["outstanding"].as_f64(), Some(30.0));
        let (at, c_at) = contract_after_accept(0.5);
        assert_eq!(contract_view(&c_at, &at, None)["outstanding"].as_f64(), Some(32.0));
    }

    // --- regression pass: the visibility matrix ------------------------

    /// A sealed genesis (`seal_amounts = 1.0`) with 4 members that do NOT all
    /// become validators (unlike `dev_genesis`, which seeds every founder as
    /// a validator — useless for testing the "authenticated but not a
    /// validator, not a party" cell of the matrix): 0 is the debtor, 1 the
    /// creditor, 2 an unrelated bystander member, 3 a validator uninvolved
    /// in the contract. Returns the built `NodeCore` and the one contract.
    fn visibility_fixture() -> (NodeCore, edet_state::types::Contract) {
        let mut st = edet_state::State::default();
        for i in 0..4u8 {
            st.add_underwriter(vec![[i; 32]], 25_000.0).expect("add member");
        }
        st.set_consensus_key(3, [0xC3u8; 32]).expect("consensus key");
        st.set_genesis_validator(3, 1).expect("member 3 is a validator");
        st.params.seal_amounts = 1.0;
        st.begin_block(60);
        apply(
            &mut st,
            edet_state::Tx::Accept {
                debtor: Party::Member(0),
                creditor: Party::Member(1),
                amount: 30.0,
                maturity_epochs: 30,
                arb: None,
            },
            [2u8; 32],
            30,
            &[[0u8; 32], [1u8; 32]],
            60,
        )
        .expect("accept should succeed");
        let c = st.contracts.values().next().cloned().expect("contract created");
        let core = NodeCore::new(0, 1, st);
        (core, c)
    }

    #[test]
    fn anonymous_read_sees_a_bucketed_amount_and_no_counterparty_identity() {
        let (core, _c) = visibility_fixture();
        let v = contracts(&core, None, Page::first());
        let entry = &v.as_array().unwrap()[0];
        assert_eq!(entry["outstanding"].as_f64(), Some(32.0), "anonymous read must bucket the amount");
        assert!(entry.get("debtor").is_none(), "anonymous read must not learn the debtor's identity");
        assert!(entry.get("creditor").is_none(), "anonymous read must not learn the creditor's identity");
    }

    #[test]
    fn a_party_sees_the_exact_amount_and_identity() {
        let (core, _c) = visibility_fixture();
        let v = contracts(&core, Some(0), Page::first()); // the debtor themself
        let entry = &v.as_array().unwrap()[0];
        assert_eq!(entry["outstanding"].as_f64(), Some(30.0), "a party must see the exact amount");
        assert_eq!(entry["debtor"].as_u64(), Some(0));
        assert_eq!(entry["creditor"].as_u64(), Some(1));
    }

    #[test]
    fn a_non_party_authenticated_member_sees_a_bucketed_amount_and_no_counterparty() {
        let (core, _c) = visibility_fixture();
        let v = contracts(&core, Some(2), Page::first()); // bystander: authenticated, not a party, not a validator
        let entry = &v.as_array().unwrap()[0];
        assert_eq!(entry["outstanding"].as_f64(), Some(32.0), "a non-party member still gets the bucket");
        assert!(entry.get("debtor").is_none(), "a non-party member must not learn the debtor's identity");
        assert!(entry.get("creditor").is_none(), "a non-party member must not learn the creditor's identity");
    }

    #[test]
    fn a_validator_sees_exact_everything() {
        let (core, _c) = visibility_fixture();
        let v = contracts(&core, Some(3), Page::first()); // validator, uninvolved in the contract
        let entry = &v.as_array().unwrap()[0];
        assert_eq!(entry["outstanding"].as_f64(), Some(30.0), "a validator sees the exact amount");
        assert_eq!(entry["debtor"].as_u64(), Some(0));
        assert_eq!(entry["creditor"].as_u64(), Some(1));
    }

    #[test]
    fn reputation_fields_are_hidden_from_an_anonymous_caller_but_visible_to_any_member() {
        let (core, _c) = visibility_fixture();
        let anon = members(&core, None, Page::first());
        let m0_anon = anon.as_array().unwrap().iter().find(|m| m["id"] == 0).unwrap();
        assert!(m0_anon.get("open_default").is_none(), "anonymous callers see no reputation signal at all");
        assert!(m0_anon.get("d_in").is_none());

        // the default: ANY authenticated member (not just a party) prices a
        // stranger's risk — member 2 is a bystander, not a party to anything
        // involving member 0.
        let auth = members(&core, Some(2), Page::first());
        let m0_auth = auth.as_array().unwrap().iter().find(|m| m["id"] == 0).unwrap();
        assert!(m0_auth.get("open_default").is_some(), "an authenticated member sees reputation signals");
    }

    #[test]
    fn member_view_drops_relationship_fields_for_a_non_qualifying_viewer() {
        let (core, _c) = visibility_fixture();
        let non_party = member(&core, 0, Some(2));
        assert!(
            non_party.get("guardian").is_none(),
            "the relationship family must be absent, not zeroed, for a non-qualifying viewer"
        );
        assert!(non_party.get("operation_bond").is_none());
        assert!(non_party.get("beneficiaries").is_none());
        assert!(non_party.get("pool").is_none());
        // But `backers` and `capacity` are PUBLIC, and deliberately so: the
        // whole model rests on standing being legible, and a stake is a
        // creditor's recorded placement rather than a private fact. The
        // amounts inside still follow the visibility rule every amount does.
        assert!(non_party.get("backers").is_some(), "who backs an account is public; how much is not");
        assert!(non_party.get("capacity").is_some());

        let self_view = member(&core, 0, Some(0));
        assert!(self_view.get("guardian").is_some(), "the member themself sees their own relationship fields");
        assert!(self_view.get("operation_bond").is_some());

        let validator_view = member(&core, 0, Some(3));
        assert!(validator_view.get("guardian").is_some(), "a validator sees relationship fields too");
    }

    #[test]
    fn pending_view_refuses_a_non_member_viewer_and_allows_the_member_or_a_validator() {
        use ed25519_dalek::{Signer, SigningKey};

        let sk0 = SigningKey::from_bytes(&[21u8; 32]);
        let sk1 = SigningKey::from_bytes(&[22u8; 32]);
        let mut st = edet_state::State::default();
        st.add_underwriter(vec![sk0.verifying_key().to_bytes()], 25_000.0)
            .expect("member 0");
        st.add_underwriter(vec![sk1.verifying_key().to_bytes()], 25_000.0)
            .expect("member 1");
        st.add_underwriter(vec![[23u8; 32]], 25_000.0).expect("member 2 (bystander)");
        st.add_underwriter(vec![[24u8; 32]], 25_000.0).expect("member 3 (validator)");
        st.set_consensus_key(3, [0xC3u8; 32]).expect("consensus key");
        st.set_genesis_validator(3, 1).expect("member 3 is a validator");

        let tx = edet_state::Tx::Accept {
            debtor: Party::Member(0),
            creditor: Party::Member(1),
            amount: 20.0,
            maturity_epochs: 30,
            arb: None,
        };
        let nonce = crate::block::counter_nonce(0);
        let not_after_epoch = 30;
        let digest = crate::block::tx_digest(crate::block::DEV_CHAIN_ID, &tx, &nonce, not_after_epoch).expect("digest");
        let req = super::super::pending::PendingSignReq {
            tx,
            nonce,
            not_after_epoch,
            required: vec![Party::Member(0), Party::Member(1)],
            min_sigs: 2,
            signer: sk0.verifying_key().to_bytes(),
            signature: sk0.sign(&digest).to_bytes().to_vec(),
            invite: None,
        };
        let mut core = NodeCore::new(0, 1, st);
        let state_snapshot = core.replica.state.clone();
        let _ = core.pending.sign(&state_snapshot, req, 1_000);

        // Anonymous: refused.
        assert!(pending(&core, 0, None).get("error").is_some(), "an anonymous viewer must be refused");
        // A bystander member (authenticated, not member 0, not a validator): refused.
        assert!(pending(&core, 0, Some(2)).get("error").is_some(), "a non-member viewer must be refused");
        // The member themself: allowed.
        let mine = pending(&core, 0, Some(0));
        assert!(mine.get("error").is_none(), "the member themself must be allowed");
        assert_eq!(mine["mine"].as_array().map(|a| a.len()), Some(1));
        // A validator, even though uninvolved: allowed (the validator carve-out).
        assert!(pending(&core, 0, Some(3)).get("error").is_none(), "a validator must be allowed");
    }

    /// CHANGED DELIBERATELY: this test previously asserted that an
    /// authenticated non-validator member scanning someone else's address was
    /// REFUSED. That assertion encoded a misreading of the spec's `whois`
    /// row, which permits "the viewer to already hold the address/pubkey
    /// they're asking about" as its first clause, and it made the product
    /// unusable — a member who is not a validator could not resolve a seller,
    /// so could not buy from anyone (see `whois`'s doc comment; `just e2e`
    /// proves it against a running node with a real admitted member, and
    /// fails on exactly this step if the old rule comes back).
    ///
    /// What must NOT change is the anonymous case: that is the
    /// scan-and-resolve primitive §Model actually flags, and it stays refused.
    #[test]
    fn whois_refuses_an_anonymous_scan_but_allows_any_authenticated_member() {
        let mut st = edet_state::State::default();
        st.add_underwriter(vec![[31u8; 32]], 25_000.0).expect("member 0");
        st.add_underwriter(vec![[32u8; 32]], 25_000.0).expect("member 1 (bystander)");
        st.add_underwriter(vec![[33u8; 32]], 25_000.0).expect("member 2 (validator)");
        st.set_consensus_key(2, [0xC2u8; 32]).expect("consensus key");
        st.set_genesis_validator(2, 1).expect("member 2 is a validator");
        let addr0 = member_address(&st.members[&0]);
        let core = NodeCore::new(0, 1, st);

        // Anonymous scan of an address the caller does not hold: refused.
        let r = whois(&core, &addr0, None);
        assert_eq!(r["member"], Value::Null);
        assert!(r.get("error").is_some(), "an anonymous scan-and-resolve must be refused");

        // An authenticated bystander resolving an address they were handed:
        // ALLOWED. This is the buyer-with-a-QR-code case, and it is the whole
        // reason addresses are published in the first place. The matrix makes
        // `id`/`address`/`status` visible to everyone anyway, so this reveals
        // nothing the members list does not — the financial and relationship
        // families it must not compose with stay behind `full_access`.
        let r = whois(&core, &addr0, Some(1));
        assert_eq!(r["member"].as_u64(), Some(0), "a member must be able to resolve a counterparty");
        assert!(r.get("error").is_none());

        // The member resolving their own address: allowed.
        let r = whois(&core, &addr0, Some(0));
        assert_eq!(r["member"].as_u64(), Some(0));
        assert!(r.get("error").is_none());

        // A validator resolving someone else's address: allowed.
        let r = whois(&core, &addr0, Some(2));
        assert_eq!(r["member"].as_u64(), Some(0));
        assert!(r.get("error").is_none());

        // A needle that matches no one reads as "not yet" for ANY viewer,
        // anonymous included. This is what lets a newly created identity poll
        // for its own admission before it is a member — see `whois`'s own note
        // on why the resulting member/non-member distinction is accepted.
        let miss = whois(&core, "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef", None);
        assert_eq!(miss["member"], Value::Null);
        assert!(miss.get("error").is_none(), "the admission poll must read as 'not yet', never as an error");
    }

    // --- operation-bond disclosure -----------------------------------------

    /// Wrap a bare `Tx` in the envelope shape `check_tx` takes, signed by
    /// nobody: the dry-run never verifies signatures (it trusts the claimed
    /// signers), so the bond disclosure is exercised without minting keys.
    fn unsigned(tx: edet_state::Tx, signers: Vec<edet_state::types::Key>) -> crate::block::SignedTx {
        crate::block::SignedTx { tx, nonce: [7u8; 16], not_after_epoch: 30, signers, signatures: vec![] }
    }

    #[test]
    fn check_tx_quotes_the_bond_from_the_same_schedule_the_gate_enforces() {
        let core = NodeCore::new(0, 1, super::super::dev_genesis(5));
        let unit = core.replica.state.params.bond_unit();
        let key0 = core.replica.state.members[&0].keys[0];

        // A state-growing class quotes multiple x unit, and the wallet is
        // told when the reservation comes back.
        let propose = || edet_state::Tx::Propose {
            author: 0,
            kind: edet_state::types::ProposalKind::ParamChange { key: ParamKey::RiskK, value: 0.6 },
        };
        let v = check_tx(&core, &unsigned(propose(), vec![key0]), 60, Some(0));
        assert_eq!(
            v["bond"]["amount"].as_f64(),
            Some(edet_state::bond::bond_multiple(&core.replica.state, &propose()) * unit),
            "the quote must be the schedule's own multiple, not a second copy of the table"
        );
        assert_eq!(v["bond"]["release_epochs"].as_u64(), Some(core.replica.state.params.bond_release_epochs));

        // The recovery classes are free, and a wallet must be able to say so
        // BEFORE the member is cornered — that is the whole reason a
        // defaulted member can still settle its way out.
        let settle = edet_state::Tx::Settle { contract: 0, amount: 1.0 };
        let v = check_tx(&core, &unsigned(settle, vec![key0]), 60, Some(0));
        assert_eq!(v["bond"]["amount"].as_f64(), Some(0.0), "settle must quote free");
    }

    #[test]
    fn check_tx_quotes_the_bond_on_rejection_too_and_never_leaks_the_payers_position() {
        let core = NodeCore::new(0, 1, super::super::dev_genesis(5));
        // A transaction that fails for an ordinary reason: the quote must
        // still be there, so a refusal can explain the cost it fell short of
        // rather than only naming a code.
        let v = check_tx(
            &core,
            &unsigned(edet_state::Tx::DeclareSupply { member: 4242, supply: 1.0 }, vec![[9u8; 32]]),
            60,
            Some(0),
        );
        assert_eq!(v["ok"].as_bool(), Some(false));
        assert!(v["bond"]["amount"].as_f64().is_some(), "a rejected dry-run must still quote the bond");

        // `/tx/check` takes no viewer, so the reply must carry NOTHING about
        // the payer's own ceiling — those fields ride `full_access` on
        // `/member/:id`, and repeating them here would make this endpoint a
        // probe for any member's headroom (see `check_tx`'s doc comment).
        for leak in ["headroom", "free_remaining", "encumbered", "affordable", "saturated_epochs", "established"] {
            assert!(v["bond"].get(leak).is_none(), "check_tx must not disclose `{leak}` — it is unauthenticated");
        }
    }

    #[test]
    fn params_publishes_the_bond_schedule_constants() {
        let core = NodeCore::new(0, 1, super::super::dev_genesis(5));
        let p = params(&core, None);
        let sp = &core.replica.state.params;
        // Public on purpose: a client cannot explain "free, covered by your
        // allowance" versus "reserves X" without the allowance size and the
        // release window, and both are constitutional constants applied
        // identically to every member.
        assert_eq!(p["bond_unit"].as_f64(), Some(sp.bond_unit()));
        assert_eq!(p["bond_free_allowance"].as_u64(), Some(u64::from(sp.bond_free_allowance)));
        assert_eq!(p["bond_release_epochs"].as_u64(), Some(sp.bond_release_epochs));
        assert_eq!(p["bond_forfeit_epochs"].as_u64(), Some(sp.bond_forfeit_epochs));
    }

    /// **The proposals view serves the measure the ledger applies** (§Governance):
    /// the external seed as the denominator, and the share of it behind each
    /// proposal as the numerator.
    ///
    /// It served `active_members` until the seed became the weight, and the
    /// client rendered `ceil(theta_adopt * active_members)` as "N assents
    /// needed" — a headcount quorum, which is a rule this ledger has never run
    /// at any point. A view describing a mechanism the chain does not have is
    /// read as a promise, so this gate names the field that must not come back
    /// as much as the two that must be there.
    #[test]
    fn the_proposals_view_reports_the_seed_and_never_a_headcount() {
        let mut core = NodeCore::new(0, 1, super::super::dev_genesis(4));
        let seed = core.replica.state.external_seed();
        assert_eq!(seed, 4.0 * super::super::DEV_SUPPLY, "four dev founders, each seeded by the ceremony");

        let key0 = core.replica.state.members[&0].keys[0];
        let kind = edet_state::types::ProposalKind::ParamChange { key: ParamKey::RiskK, value: 0.6 };
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::Propose { author: 0, kind },
            [1u8; 32],
            30,
            &[key0],
            0,
        )
        .expect("a founder may propose");

        let v = proposals(&core, None);
        assert_eq!(v["external_seed"].as_f64(), Some(seed), "the denominator is the ceremony, not the roll");
        assert!(v.get("active_members").is_none(), "a headcount must not reappear as the governance measure");
        assert_eq!(v["proposals"][0]["assented_seed"].as_f64(), Some(0.0), "proposing is not assenting");

        // One founder of four assents: a quarter of the seed, against a bar of
        // half. The view must say so in the units the rule is written in.
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::Assent { member: 0, proposal: 0 },
            [2u8; 32],
            30,
            &[key0],
            0,
        )
        .expect("a founder holds external supply, so they are in the electorate");
        let v = proposals(&core, None);
        assert_eq!(v["proposals"][0]["assented_seed"].as_f64(), Some(seed / 4.0));
        assert_eq!(v["proposals"][0]["enacted"].as_bool(), Some(false));
        assert!(
            v["proposals"][0]["assented_seed"].as_f64().unwrap()
                < v["theta_adopt"].as_f64().unwrap() * v["external_seed"].as_f64().unwrap(),
            "and the client's own threshold arithmetic must agree with the ledger's refusal"
        );
    }
    /// **The network view serves the model this ledger runs, and never the one
    /// it replaced.** Same shape as the proposals gate above, found by the
    /// paper pass rather than by an attack.
    ///
    /// A `NetworkView` declaring `phi`, `gauge_g` and `kappa_vol` — an activity
    /// gauge, a macroprudential governor and a volatility term — gets two of
    /// them RENDERED, as "Activity φ" and a "Community brake" whose help text
    /// tells every member that "new credit is being tightened across the
    /// community to contain contagion risk". The node serves none of the three,
    /// so the card reads `NaN%`; and there is no brake in this model at all,
    /// deliberately (the paper §stability: a global multiplier is a second
    /// answer to the question this design exists to have one answer to). **A
    /// view describing a mechanism the chain does not run is read as a
    /// promise**, which is why `just view-shape-check` gates a client type
    /// against what `views.rs` actually serves.
    ///
    /// So this names the three that must not come back, and the honest signals
    /// that took their place.
    #[test]
    fn the_network_view_reports_the_seed_and_never_a_governor() {
        let core = NodeCore::new(0, 1, super::super::dev_genesis(4));
        let cfg = Config {
            index: 0,
            n: 1,
            listen_port: 0,
            peers: vec![],
            allow_unsigned: false,
            data_dir: None,
            snapshot_interval: 0,
            prune_margin_blocks: crate::replica::DEFAULT_PRUNE_MARGIN_BLOCKS,
            bind_all: false,
            cors_ports: vec![],
            cluster_token: None,
            trust_forwarded_for: false,
        };
        let v = network(&core, &cfg, None);

        // `declared_supply` sits with the governor terms for the same reason:
        // it describes a quantity the ledger does not have. Every declaration
        // is ceremony-seated, so the declared total IS the external seed, and
        // serving both invites a reader to look for a gap between them that
        // cannot exist.
        for gone in ["phi", "gauge_g", "kappa_vol", "declared_supply"] {
            assert!(v.get(gone).is_none(), "a view of a mechanism the chain does not run: `{gone}`");
        }
        for honest in
            ["underwriters", "insured_credit", "external_seed", "seed_headroom", "utilisation", "uninsured_obligations"]
        {
            assert!(v.get(honest).is_some(), "the network view must serve `{honest}`");
        }
        // Nothing is drawn on a fresh ledger, so the ceiling is idle rather
        // than throttled — the distinction the deleted card could not make.
        assert_eq!(v["insured_credit"].as_f64(), Some(0.0));
        assert_eq!(v["utilisation"].as_f64(), Some(0.0));
        assert_eq!(v["external_seed"].as_f64(), Some(4.0 * super::super::DEV_SUPPLY));
    }

    /// **A member's own view must report the allowance the gate will grant**,
    /// not the allowance minus what they have spent.
    ///
    /// `free_remaining` was `bond_free_allowance - bond_free_used`, with no test
    /// that the member qualifies for an allowance at all — and the gate grants
    /// none to a member with nothing to lose, because a per-key allowance is the
    /// free-signature bound's own defect one layer down. So a member whose
    /// backing had been withdrawn read **32 of 32 free actions** in their own
    /// wallet, in the "good" tone, while every write came back `ET-BND-001`.
    /// That is the allowance cliff, and the client could not warn about it
    /// because the figure it was given said there was nothing to warn about.
    #[test]
    fn the_allowance_a_member_reads_is_the_allowance_the_gate_grants() {
        let mut core = NodeCore::new(0, 1, super::super::dev_genesis(2));
        let st = &mut core.replica.state;
        let m = st.new_account(vec![[9u8; 32]]);

        // Nobody has backed them, so the gate grants nothing and the view must
        // agree — this is also every newcomer's first moment.
        let before = member(&core, m, Some(m));
        assert_eq!(before["operation_bond"]["free_remaining"].as_u64(), Some(0));
        assert_eq!(before["operation_bond"]["established"].as_bool(), Some(false));
        assert_eq!(edet_state::bond::free_remaining(&core.replica.state, m), 0);

        // Backed the only way anything is backed here: they owe, and they pay.
        let key0 = core.replica.state.members[&0].keys[0];
        let mine = [9u8; 32];
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::Accept {
                debtor: edet_state::types::Party::Member(m),
                creditor: edet_state::types::Party::Member(0),
                amount: 100.0,
                maturity_epochs: 30,
                arb: None,
            },
            [21u8; 32],
            30,
            &[key0, mine],
            0,
        )
        .expect("a first trade is uninsured, not refused");
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::Settle { contract: 0, amount: 100.0 },
            [22u8; 32],
            30,
            &[key0, mine],
            0,
        )
        .expect("and settling it is what confers the standing");
        assert!(core.replica.state.conferrable(m) > 0.0, "the fixture must actually have backed them");

        let after = member(&core, m, Some(m));
        assert_eq!(
            after["operation_bond"]["free_remaining"].as_u64(),
            Some(u64::from(core.replica.state.params.bond_free_allowance)),
            "the view and the gate answer with one function"
        );
        assert_eq!(
            after["operation_bond"]["established"].as_bool(),
            Some(edet_state::bond::established(&core.replica.state, m)),
            "and the qualification underneath it is served rather than re-derived"
        );
    }

    /// **A support edge serves the quantity that decides whether it carries
    /// anything**, and the quantity is not the weight and not the approval.
    ///
    /// The drain across an edge is `NU_DRAIN` times what the pair has staked in
    /// one another, with deliberately no floor: a pair that has never settled
    /// anything drains ZERO, approved or not, because the whitelist is not what
    /// keeps strangers out — the cut is. The page showed a weight and an
    /// approval pill, so two members could list, approve, see both go green,
    /// and route nothing between them for ever with no indication why.
    #[test]
    fn a_support_edge_serves_the_cap_that_decides_it() {
        let mut core = NodeCore::new(0, 1, super::super::dev_genesis(2));
        let key0 = core.replica.state.members[&0].keys[0];
        let key1 = core.replica.state.members[&1].keys[0];

        // 0 lists 1 and 1 approves 0: every declaration the mechanism asks for.
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::ListBeneficiaries { supporter: 0, entries: vec![(0, 1.0), (1, 1.0)] },
            [30u8; 32],
            30,
            &[key0],
            0,
        )
        .expect("a supporter lists a beneficiary");
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::ApproveSupporter { beneficiary: 1, supporter: 0, approved: true },
            [31u8; 32],
            30,
            &[key1],
            0,
        )
        .expect("and the beneficiary approves them");

        let listed = member(&core, 0, Some(0));
        // Toward oneself there is no pair and therefore no cap: the self entry
        // is a SHARE of the sale, not a route to anybody.
        let own = &listed["beneficiaries"][0];
        assert_eq!(own["member"].as_u64(), Some(0));
        assert!(own["drain_cap"].is_null(), "a share is not an edge");

        let ben = &listed["beneficiaries"][1];
        assert_eq!(ben["member"].as_u64(), Some(1));
        assert_eq!(ben["approved"].as_bool(), Some(true), "both declarations are in");
        assert_eq!(
            ben["drain_cap"].as_f64(),
            Some(0.0),
            "and it still carries NOTHING, because the pair has never settled anything"
        );

        // A settled trade between them, and the edge acquires a cap.
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::Accept {
                debtor: edet_state::types::Party::Member(1),
                creditor: edet_state::types::Party::Member(0),
                amount: 40.0,
                maturity_epochs: 30,
                arb: None,
            },
            [32u8; 32],
            30,
            &[key0, key1],
            0,
        )
        .expect("accepted");
        let cid = core.replica.state.next_contract - 1;
        edet_state::apply(
            &mut core.replica.state,
            edet_state::Tx::Settle { contract: cid, amount: 40.0 },
            [33u8; 32],
            30,
            &[key0, key1],
            0,
        )
        .expect("settled");

        let after = member(&core, 0, Some(0));
        assert_eq!(
            after["beneficiaries"][1]["drain_cap"].as_f64(),
            Some(edet_kernel::constants::NU_DRAIN * 40.0),
            "nu times what the pair staked, which settlement is the only writer of"
        );
        // And the other side reads the same edge, from its own view.
        let theirs = member(&core, 1, Some(1));
        assert_eq!(theirs["supporters"][0]["drain_cap"].as_f64(), Some(edet_kernel::constants::NU_DRAIN * 40.0));
    }
}
