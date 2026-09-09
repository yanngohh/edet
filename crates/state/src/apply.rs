//! Transition application: validate against current state, then mutate.
//!
//! No code path here may panic on any input: every lookup returns a named
//! error, including "impossible" ones — a validator must survive arbitrary
//! transactions.

use std::collections::BTreeSet;

use edet_kernel::constants as k;

use crate::errors::*;
use crate::state::State;
use crate::tx::Tx;
use crate::types::*;

/// Apply one transaction. `signers` are the keys whose signatures the driver
/// has already verified over this transaction. `tx_id` is the caller-computed
/// envelope digest (`edet_node::block::SignedTx::id`) and `not_after_epoch`
/// its claimed validity window — `edet_state` stays the pure state machine
/// and never learns about `SignedTx` itself, so both are passed in rather
/// than derived here.
///
/// Replay/expiry/window are checked BEFORE dispatch, in this exact
/// order — epoch first (the window checks below are meaningless against a
/// stale `state.epoch`), then expiry, then the window ceiling, then replay —
/// and `tx_id` is recorded as applied UNCONDITIONALLY, regardless of whether
/// the dispatched transition below then succeeds or fails.
///
/// That last part is the non-obvious half. If a rejected transaction left no
/// trace, an attacker could hold a transaction that fails today (say,
/// insufficient capacity) and replay the identical signed bytes later, once
/// it WOULD succeed — exactly the attack this whole mechanism exists to
/// close, just deferred past the point a reviewer would think to look. The
/// cost is that a legitimately failed transaction must be re-signed with a
/// fresh nonce to retry; the client already does this for every other
/// rejection reason, so this is not a new burden, just an existing one
/// applied one case further.
pub fn apply(
    state: &mut State,
    tx: Tx,
    tx_id: [u8; 32],
    not_after_epoch: u64,
    signers: &[Key],
    now_secs: u64,
) -> Res<()> {
    apply_with_cache(state, tx, tx_id, not_after_epoch, signers, now_secs, &mut crate::bond::GateCache::default())
}

/// `apply`, with the write gate's `seed_reach` memo carried across calls.
///
/// **`apply` stays the definition.** The memo answers the gate's threshold
/// tests from a lower bound where it passes and recomputes exactly where it
/// does not, so the two reach the same verdict on every state — and the state
/// harness computes both before every transition in the tree and panics on a
/// disagreement, rather than leaving the property to a probe that remembers to
/// look.
#[allow(clippy::too_many_arguments)]
pub fn apply_with_cache(
    state: &mut State,
    tx: Tx,
    tx_id: [u8; 32],
    not_after_epoch: u64,
    signers: &[Key],
    now_secs: u64,
    gate: &mut crate::bond::GateCache,
) -> Res<()> {
    state.begin_block(now_secs);
    if not_after_epoch < state.epoch {
        return Err(Error(ET_TX_EXPIRED));
    }
    if not_after_epoch > horizon(state.epoch, k::MAX_TX_LIFETIME_EPOCHS) {
        return Err(Error(ET_TX_WINDOW_TOO_LONG));
    }
    if state.is_applied(&tx_id, not_after_epoch) {
        return Err(Error(ET_TX_REPLAY));
    }
    // **The gate is the door, and a replay id is durable state.**
    //
    // `applied_by_expiry` is hashed into the state root
    // (`root.rs`) and retained for up to `MAX_TX_LIFETIME_EPOCHS`, so recording
    // one IS a write — and `bond_gate`'s own note says what a write must never
    // be: *"submit transitions designed to fail, consume a consensus round and
    // a durable write for each, and pay nothing"*. Recording before the gate
    // was exactly that, because the two branches that refuse there charge
    // nothing on the way out. Measured: a member at their ceiling wrote **200**
    // ids with **0.00** encumbered, and two fresh keys signing an `Accept`
    // between themselves wrote **100** for a `Due::Unpayable` refusal — no
    // account, no standing, no headroom.
    //
    // So an envelope refused at the gate leaves nothing behind: it was never
    // admitted. What that gives up is the uniformity the old note argued for,
    // and the argument does not survive being measured — a gate refusal is
    // about the SUBMITTER's write budget rather than about the transition's
    // terms, so it taxes an honest pair (who must re-collect signatures after a
    // moment at their ceiling) while bounding an attacker not at all, since
    // fresh keys mint fresh envelopes for free. The deferred replay the policy
    // exists to close is a rejection at DISPATCH — the transition attempted
    // against the ledger's economic state and refused by it — and every one of
    // those still spends its id, which is what `adversarial.rs` pins.
    let (charged, sponsor) = bond_gate(state, &tx, signers, gate)?;
    let permissionless = crate::bond::is_permissionless(&tx);
    let subject = crate::bond::self_act_subject(&tx);
    state.record_applied(tx_id, not_after_epoch);
    let outcome = dispatch(state, tx, signers, sponsor);
    // An envelope that failed because it was not AUTHORISED never spends the
    // id, and the bond charged for it is returned. `tx_digest` covers
    // `(tx, nonce, not_after_epoch)` and deliberately not `signers`, so an
    // envelope with a co-signature stripped out is a different wire message
    // with the SAME id — and the pending-signature pool gossips
    // partially-signed envelopes network-wide by design, so the under-signed
    // copy exists before the complete one. Burning the id on it let anyone
    // who saw a request permanently kill the genuine transaction for free,
    // and bill its bond to a party who never submitted anything.
    //
    // **Without `refund`, the second half of that sentence is prose.** With
    // the id released and the BOND not: measured, an `Accept` replayed with
    // the creditor's signature stripped moves the debtor's encumbrance
    // 0.00 → 20.00 and leaves it there, so the griefing the paragraph
    // describes is half-live — repeat it and the victim's headroom is gone,
    // and then their own honest traffic starts arming
    // `bond_denied_this_epoch` for the forfeiture crank. `refund` is the
    // other half. **Check that a claim's MEASUREMENT exists, not that its
    // wording matches.**
    //
    // This does not reopen the deferred replay the record-before-dispatch
    // policy closes. That attack holds a transaction which fails for an
    // ECONOMIC reason today and replays it when it would succeed; those
    // rejections still spend the id, exactly as before. What is released here
    // is only the case where the presented signatures did not authorise the
    // transition at all — an envelope that authorised nothing, and whose
    // retry is the genuinely-signed transaction the parties intended.
    if outcome == Err(Error(ET_MEM_NOT_SIGNER)) {
        state.forget_applied(&tx_id, not_after_epoch);
        refund(state, charged);
    } else if outcome.is_err() && charged == Charged::Free {
        // **A refused free class is priced, or it is forgotten.** A zero on
        // the schedule is justified by the transition destroying its own
        // precondition, and a refusal destroys nothing — so a free class that
        // fails at dispatch was, until here, an unbounded durable write: fifty
        // `Exit`s by a member who owes something wrote fifty ids, and a
        // keyless `RotateFinalize` wrote fifty more, each hashed into a replay
        // bucket the root re-encodes whole (110 ms per block per million ids).
        //
        // A crank carries no signature to defer and mutated nothing to protect
        // (`bond::is_permissionless`), so its id goes. A signed free class
        // keeps the deferred-replay defence — `Exit` and a lowering
        // `DeclareSupply` fail now and succeed later, and a replayed one is an
        // involuntary act — but the write is paid for: one allowance slot of
        // the first signer in canonical order who has one. Where nobody
        // present has a slot, nothing is kept, because a durable write nobody
        // paid for is the channel this branch closes; what that gives up is
        // the deferred-replay defence for a member with no allowance, whose
        // replayed `Exit` is their own signed intent arriving late.
        if permissionless || !price_refusal(state, subject, signers) {
            state.forget_applied(&tx_id, not_after_epoch);
        }
    }
    outcome
}

/// Spend one allowance slot for a free-class envelope the ledger refused —
/// the first signer in canonical order with a slot left. `false` when nobody
/// present has one, in which case the caller keeps nothing.
fn price_refusal(state: &mut State, subject: Option<MemberId>, signers: &[Key]) -> bool {
    // The row's own seat slot, by the rule the gate spends it under: the
    // transition is about this row and nobody else, the row's own key signed,
    // and it has no allowance of its own. Nobody burns another member's slot,
    // and a refused first write is not retried free.
    if let Some(id) = subject {
        let own = state.members.get(&id).is_some_and(|m| {
            m.seat_slot && matches!(m.status, MemberStatus::Active) && signers.iter().any(|s| m.has_key(s))
        });
        if own && crate::bond::free_remaining(state, id) == 0 {
            if let Some(m) = state.members.get_mut(&id) {
                m.seat_slot = false;
                return true;
            }
        }
    }
    let payer = crate::bond::payers(state, signers)
        .into_iter()
        .find(|&id| crate::bond::free_remaining(state, id) > 0);
    match payer.and_then(|id| state.members.get_mut(&id)) {
        Some(m) => {
            m.bond_free_used += 1;
            true
        }
        None => false,
    }
}

/// Does every id this transaction names exist on the ledger yet?
///
/// Issued means below the next id to be handed out; a retired row's id is
/// issued and fails at dispatch as it always did. An envelope naming an id from
/// the future cannot succeed now, so the node's ingress asks this and keeps it
/// out of the mempool rather than spending a block slot on a refusal
/// (`serve::core::NodeCore::submit`). It is deliberately NOT a rule `apply`
/// enforces ahead of the replay record: the deferred-replay policy is uniform
/// — a refused signed envelope spends its id where somebody can pay for it —
/// and a second answer for one shape of refusal would be a second policy.
pub fn names_issued_ids(state: &State, tx: &Tx) -> Res<()> {
    let member = |id: MemberId| if id < state.next_member { Ok(()) } else { Err(Error(ET_MEM_UNKNOWN)) };
    let party = |p: &Party| match p {
        Party::Member(id) => member(*id),
        Party::Key(_) => Ok(()),
    };
    let contract = |id: ContractId| if id < state.next_contract { Ok(()) } else { Err(Error(ET_CTR_UNKNOWN)) };
    let proposal = |id: ProposalId| if id < state.next_proposal { Ok(()) } else { Err(Error(ET_GOV_UNKNOWN_PROPOSAL)) };
    match tx {
        Tx::RegisterGuardians { member: m, guardians, .. } => {
            member(*m)?;
            guardians.iter().try_for_each(|&g| member(g))
        }
        Tx::RotateRequest { member: m, .. }
        | Tx::RotateVeto { member: m }
        | Tx::RotateFinalize { member: m }
        | Tx::SetConsensusKey { member: m, .. }
        | Tx::Exit { member: m }
        | Tx::DeclareSupply { member: m, .. }
        | Tx::ForfeitBonds { member: m } => member(*m),
        Tx::ListBeneficiaries { supporter, entries } => {
            member(*supporter)?;
            entries.iter().try_for_each(|&(b, _)| member(b))
        }
        Tx::ApproveSupporter { beneficiary, supporter, .. } => {
            member(*beneficiary)?;
            member(*supporter)
        }
        Tx::Sale { seller, buyer, .. } => {
            party(seller)?;
            party(buyer)
        }
        Tx::Accept { debtor, creditor, arb, .. } => {
            party(debtor)?;
            party(creditor)?;
            arb.iter().flat_map(|a| a.arbiters.iter()).try_for_each(|&a| member(a))
        }
        Tx::Transfer { contract: c, new_debtor } => {
            contract(*c)?;
            member(*new_debtor)
        }
        Tx::Settle { contract: c, .. }
        | Tx::Extend { contract: c, .. }
        | Tx::MarkExpired { contract: c }
        | Tx::Cure { contract: c, .. } => contract(*c),
        Tx::ArbAttest { contract: c, arbiter, .. } => {
            contract(*c)?;
            member(*arbiter)
        }
        Tx::Propose { author, kind } => {
            member(*author)?;
            match kind {
                ProposalKind::Suspend { member: m }
                | ProposalKind::Unsuspend { member: m }
                | ProposalKind::ValidatorPower { member: m, .. } => member(*m),
                _ => Ok(()),
            }
        }
        Tx::Assent { member: m, proposal: p } => {
            member(*m)?;
            proposal(*p)
        }
    }
}

/// What `bond_gate` actually charged, so an envelope that turns out to have
/// authorised nothing can be un-charged EXACTLY rather than approximately —
/// and so a refused free class can be told apart from a refused priced one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Charged {
    /// A zero-priced class: the gate charged nothing and asked nobody to pay.
    Free,
    /// One slot of this member's free allowance.
    Allowance(MemberId),
    /// The one self-act this member's seat paid for.
    Slot(MemberId),
    /// This much of this member's headroom, encumbered until `release_at`.
    Bond { payer: MemberId, release_at: u64, amount: u64 },
}

/// Undo one `bond_gate` charge. The release epoch is carried rather than
/// recomputed, because a bond is keyed by the epoch it returns in and nothing
/// guarantees the caller is still reading the same clock.
fn refund(state: &mut State, charged: Charged) {
    match charged {
        Charged::Free => {}
        Charged::Allowance(payer) => {
            if let Some(m) = state.members.get_mut(&payer) {
                m.bond_free_used = m.bond_free_used.saturating_sub(1);
            }
        }
        Charged::Slot(payer) => {
            if let Some(m) = state.members.get_mut(&payer) {
                m.seat_slot = true;
            }
        }
        Charged::Bond { payer, release_at, amount } => {
            if let Some(m) = state.members.get_mut(&payer) {
                if let Some(v) = m.bonds.get_mut(&release_at) {
                    *v = v.saturating_sub(amount);
                    if *v == 0 {
                        m.bonds.remove(&release_at);
                    }
                }
            }
        }
    }
}

/// `sponsor` is the member whose seat reach the gate charged, for the one
/// transition class that can seat a row. `None` everywhere else — and a `None`
/// reaching `seat` with a fresh party to write is a refusal, not a free row.
fn dispatch(state: &mut State, tx: Tx, signers: &[Key], sponsor: Option<MemberId>) -> Res<()> {
    match tx {
        Tx::RegisterGuardians { member, guardians, threshold, veto_window_epochs } => {
            register_guardians(state, member, guardians, threshold, veto_window_epochs, signers)
        }
        Tx::RotateRequest { member, new_keys } => rotate_request(state, member, new_keys, signers),
        Tx::RotateVeto { member } => rotate_veto(state, member, signers),
        Tx::RotateFinalize { member } => rotate_finalize(state, member),
        Tx::SetConsensusKey { member, key } => set_consensus_key(state, member, key, signers),
        Tx::DeclareSupply { member, supply } => declare_supply(state, member, supply, signers),
        Tx::Accept { debtor, creditor, amount, maturity_epochs, arb } => {
            accept(state, debtor, creditor, amount, maturity_epochs, arb, signers, sponsor)
        }
        Tx::Transfer { contract, new_debtor } => transfer(state, contract, new_debtor, signers),
        Tx::Settle { contract, amount } => settle(state, contract, amount, signers),
        Tx::Extend { contract, new_maturity_epoch } => extend(state, contract, new_maturity_epoch, signers),
        Tx::MarkExpired { contract } => mark_expired(state, contract),
        Tx::Cure { contract, amount } => cure(state, contract, amount, signers),
        Tx::ListBeneficiaries { supporter, entries } => list_beneficiaries(state, supporter, entries, signers),
        Tx::ApproveSupporter { beneficiary, supporter, approved } => {
            approve_supporter(state, beneficiary, supporter, approved, signers)
        }
        Tx::Sale { seller, buyer, amount, maturity_epochs } => {
            crate::cascade::sale(state, seller, buyer, amount, maturity_epochs, signers, sponsor)
        }
        Tx::ArbAttest { contract, arbiter, amount } => arb_attest(state, contract, arbiter, amount, signers),
        Tx::Exit { member } => exit(state, member, signers),
        Tx::Propose { author, kind } => propose(state, author, kind, signers),
        Tx::Assent { member, proposal } => assent(state, member, proposal, signers),
        Tx::ForfeitBonds { member } => forfeit_bonds(state, member),
    }
}

/// A party to a trade, resolved against current state but not yet written.
///
/// The two-step — resolve, validate everything, THEN seat — is what keeps the
/// rule honest. `dispatch` has no rollback, so a party seated at the moment it
/// was resolved would leave an account row behind every `Accept` that then
/// failed on its amount, its maturity or its arbitration terms. A row must
/// appear only alongside a write that somebody paid for AND that succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// An account that already exists.
    Existing(MemberId),
    /// A key with no account yet: `seat` gives it one.
    Fresh(Key),
}

/// Resolve a named party without writing anything.
///
/// A key that already belongs to somebody resolves to them, so naming a key is
/// never a way to displace an existing account — which is also why an id and a
/// key are interchangeable for a member who has one. A key that belongs to
/// nobody must have SIGNED: that is the same key-control proof `OpenAccount`
/// took, arriving through the signature `Accept` and `Sale` already required
/// from both sides, so nothing about consent has been relaxed to get here.
///
/// A key sitting in a member's `pending_rotation` is spoken for and is refused
/// rather than seated. `key_is_claimed` is the check that sees it: seating one
/// would leave a single key in two members' `keys` after the rotation
/// finalized, one of them unresolvable by `member_of_key` for the rest of its
/// life.
pub(crate) fn resolve(state: &State, party: &Party, signers: &[Key]) -> Res<Resolved> {
    match party {
        Party::Member(id) => {
            member(state, *id)?;
            Ok(Resolved::Existing(*id))
        }
        Party::Key(k) => match state.member_of_key(k) {
            Some(id) => Ok(Resolved::Existing(id)),
            None if key_is_claimed(state, k) => Err(Error(ET_ADM_DUP_KEY)),
            None if !signers.contains(k) => Err(Error(ET_MEM_NOT_SIGNER)),
            None => Ok(Resolved::Fresh(*k)),
        },
    }
}

/// Consent and status for a party that already exists. A `Fresh` one signed to
/// get through `resolve` and is `Active` by construction, so there is nothing
/// left to ask it.
pub(crate) fn require_party(state: &State, party: Resolved, signers: &[Key]) -> Res<()> {
    if let Resolved::Existing(id) = party {
        require_signed(state, id, signers)?;
        require_active(state, id)?;
    }
    Ok(())
}

/// Give a trade's two resolved parties their ids, creating an account for each
/// that this trade is the first of. Call only once the transition can no longer
/// be refused.
///
/// Creation is still not an admission and still grants nothing: the account has
/// no incident stakes, hence a capacity of zero, hence can owe nothing the
/// community underwrites. What it costs is the sponsor's — one bond unit of
/// their reach on the write layer, taken HERE, where the row is written, and
/// nowhere else, so a refused trade seats nobody and holds nothing.
///
/// **The reservation comes first.** The gate asked `can_seat_minor` on this
/// same state, and it asked the same SEQUENCE of holds this takes — one
/// reservation per row on the residual the previous one left, never one flow
/// of two units, which a maximum flow could route where two holds cannot — so
/// the `None` branch is unreachable, and is kept because a gate and a write
/// that disagree must refuse rather than write an unpriced row.
/// `tests/seats.rs` holds the two equal over a sweep of shapes, the branching
/// one included.
pub(crate) fn seat_pair(
    state: &mut State,
    first: Resolved,
    second: Resolved,
    sponsor: Option<MemberId>,
) -> Res<(MemberId, MemberId)> {
    // Both reservations before either row, for the reason the two-step above
    // exists one level up: `dispatch` has no rollback, so seating one party and
    // then finding the second unaffordable would leave a row behind a
    // transaction that failed — and a seat spent on it.
    let mut held = Vec::new();
    for party in [first, second] {
        if matches!(party, Resolved::Fresh(_)) {
            let Some(sponsor) = sponsor else { return Err(Error(ET_BOND_NO_PAYER)) };
            match state.reserve_seat(sponsor) {
                Some(h) => held.push((sponsor, h)),
                None => {
                    // Give back what this call took, and only that: the seats
                    // already standing are other rows' and are never released.
                    for (_, h) in &held {
                        state.release_seat(h);
                    }
                    return Err(Error(ET_BOND_SEAT_UNBACKED));
                }
            }
        }
    }
    let mut taken = held.into_iter();
    let mut write = |state: &mut State, party: Resolved| match party {
        Resolved::Existing(id) => id,
        Resolved::Fresh(k) => {
            let (sponsor, held) = taken.next().expect("one reservation per fresh party, taken above");
            let id = state.new_account(vec![k]);
            let row = state.members.get_mut(&id).expect("the row `new_account` just wrote");
            row.seat = Some(Seat { sponsor, held });
            // One self-act the seat paid for, so the newcomer registers
            // guardians once without a co-signer. Bounded by seats — a stock.
            row.seat_slot = true;
            id
        }
    };
    let a = write(state, first);
    let b = write(state, second);
    Ok((a, b))
}

/// Ceiling on the keys one account may claim.
///
/// Not a security bound — Sybil resistance is arithmetic, not a quota (see
/// `State::capacity_of`) — but a storage one, and this is the only
/// path that can grow a key list at all: an account is born from `seat` holding
/// exactly one key, and `RotateRequest` is the one transition that writes
/// another.
///
/// **It has to be enforced on the transition that writes an arbitrary
/// vector**, which is this one. A bound checked anywhere else leaves
/// `rotate_request` free to put 100,000 keys into `pending_rotation` and then
/// into `key_index` for a headroom spend of ZERO, because the guardians' free
/// allowance covers the one bond — a bond is charged per transaction and says
/// nothing about the size of its payload.
pub(crate) const MAX_KEYS_PER_ACCOUNT: usize = 8;

/// Forfeit the encumbered bonds of a member in sustained exhaustion.
///
/// Permissionless and state-checked, like `MarkExpired`: no signature is
/// required and no discretion exists, so the sanction can neither be forged
/// against a member who never saturated the gate nor suppressed by whoever
/// would otherwise have had to call for it.
///
/// The forfeited amount moves to `forfeit_reserve` rather than to any party,
/// so nobody is owed it and no position anywhere improves at the moment of
/// forfeiture — which is precisely why the mechanism collects no rent.
///
/// **What it is worth, decided: the encumbrance never comes back.**
/// `forfeit_reserve` was the loss pool's first-loss layer, and when the pool was
/// retired (§Recourse) it became a number nothing read — while this function handed
/// the headroom straight back with `m.bonds.clear()`, so the "penalty" was a
/// release. Measured: an abuser forfeited 2500, its headroom went 0 → 2231, and
/// it immediately wrote 22 more transitions.
///
/// `bond_headroom` now subtracts `forfeit_reserve`, which gives the field a
/// consumer again and makes the sanction enforceable by arithmetic: a
/// reservation that never returns. The bonds themselves are still cleared here,
/// because carrying them in `bonds` would put them back on the release schedule
/// they are being taken off — the amount moves from a dated encumbrance to an
/// undated one. Nothing is minted, so `prop:no-rent` holds, and the way back is
/// the model's own: earn more standing and `conferrable` grows past it.
fn forfeit_bonds(state: &mut State, target: MemberId) -> Res<()> {
    let m = member(state, target)?;
    if m.bond_saturated_epochs < state.params.bond_forfeit_epochs {
        return Err(Error(ET_BOND_NOT_SATURATED));
    }
    let held = m.bond_enc();
    if held <= state.params.dust_minor() {
        return Err(Error(ET_BOND_NOTHING_HELD));
    }
    {
        let m = member_mut(state, target)?;
        m.bonds.clear();
        // The counter resets with the forfeiture: the sanction is once per
        // sustained episode, not once per epoch for as long as the episode
        // lasts. Without this reset a single saturated member could be
        // forfeited repeatedly on every subsequent block.
        m.bond_saturated_epochs = 0;
        m.bond_denied_this_epoch = false;
    }
    *state.forfeit_reserve.entry(target).or_insert(0) += held;
    Ok(())
}

/// Run the permissionless cranks over everything that has fallen due, once
/// per epoch boundary, from inside the epoch machinery itself.
///
/// **Nobody runs a permissionless crank.** That was the defect: `MarkExpired`
/// and `ForfeitBonds` are signerless by design, so no party is responsible for
/// calling them, and an uncranked default was simply unrecorded — the debt
/// stood, but the sanction never landed and the creditor never got their
/// remedy. Worse than a gap, it was a *discretion*: whoever noticed a default
/// first chose whether and when it counted.
///
/// Running it here rather than from a node's daemon is what removes that
/// discretion entirely. Every validator executes the same block stream over
/// the same state, so the sweep is a function of the ledger — it cannot be
/// forged against a member who is not due, cannot be suppressed by a proposer
/// declining to include a transaction, and cannot be timed. The transitions
/// stay in the alphabet and stay callable by anyone; what changes is that
/// calling them is now a way to be early rather than the only way it happens.
///
/// Both cranks are already idempotent and already pure state checks, which is
/// what makes them safe to run this way: `mark_expired` refuses anything not
/// Active and past maturity, `forfeit_bonds` refuses anything not in sustained
/// exhaustion, and each resets the condition it fires on. Their errors are
/// dropped rather than propagated — this runs inside `begin_block`, which is
/// infallible by design, and "this contract was not due after all" is the
/// expected outcome for most of what a sweep looks at.
///
/// Cost is one pass over the contract book per epoch boundary, ordered by
/// contract id (a `BTreeMap`), which is negligible beside the flow queries the
/// same block already runs. A maturity index would make it proportional to
/// what is DUE rather than to what exists, and is the right change when the
/// book outgrows the scan; it is not one yet.
/// **Net rings of defaulted obligations by their minimum, with no signature.**
///
/// A ring is A owing B owing C owing A. Netting the smallest of the three
/// relieves each party of exactly as much debt as claim, so nobody loses —
/// and the paper already states the rule for the bilateral case a sale runs
/// (`cascade::net_mutual`); this is the same act over a longer cycle, and the
/// same code (`cascade::discharge_hop`).
///
/// **Why no consent is needed, and why it is needed for nothing shorter.**
/// Every hop is a DEFAULT, so every party is due now: nobody is being paid
/// early, nobody loses time value, and each is relieved of a debt they did not
/// pay against a claim they could not collect. That argument does not survive
/// dropping the default condition — netting a live obligation would hand its
/// creditor an early payment they never agreed to take — which is why
/// `eligible` insists on it rather than treating it as a heuristic.
///
/// A claim under an arbitration window is the panel's and not the sweep's, and
/// a party who is not Active cannot originate or discharge, so both are
/// excluded. An INSURED hop is eligible: netting cures it and `rehold`
/// releases exactly what it held, which is the one thing a default does not do
/// on its own.
///
/// Discovery is a function of state: eligible hops in ascending contract id,
/// the walk taking the smallest id at each step, bounded at
/// `NETTING_MAX_RING` hops and `NETTING_MAX_RINGS_PER_EPOCH` rings. Zero-priced
/// and permissionless, so it must destroy its own precondition — and it does:
/// every ring netted closes at least one hop, so no sweep can find the same
/// ring twice.
fn net_rings(state: &mut State) {
    for _ in 0..k::NETTING_MAX_RINGS_PER_EPOCH {
        let Some(ring) = find_ring(state) else { break };
        let Some(take) = ring.iter().map(|cid| state.contracts[cid].outstanding).min() else { break };
        for cid in ring {
            // A hop that will not discharge leaves the rest of the ring
            // half-netted, which is still a legal state — each hop is its own
            // discharge and the conservation sum holds per row — and the audit
            // on the commit path is what says otherwise.
            let _ = crate::cascade::discharge_hop(state, cid, take);
        }
    }
}

/// Is this contract a hop the sweep may net?
fn eligible_hop(state: &State, c: &Contract) -> bool {
    let active = |id: MemberId| state.members.get(&id).is_some_and(|m| m.status == MemberStatus::Active);
    c.status == ContractStatus::Expired
        && c.outstanding > state.params.dust_minor()
        && c.arb.is_none()
        && active(c.debtor)
        && active(c.creditor)
}

/// The lowest ring the eligible book contains, or `None`.
///
/// A bounded depth-first walk from each eligible hop in ascending contract id,
/// taking the smallest id at every step and never revisiting a member — so the
/// cycle it returns is simple, and which cycle it is depends on the state and
/// on nothing else.
fn find_ring(state: &State) -> Option<Vec<ContractId>> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut out: BTreeMap<MemberId, Vec<ContractId>> = BTreeMap::new();
    for c in state.contracts.values() {
        if eligible_hop(state, c) {
            out.entry(c.debtor).or_default().push(c.id);
        }
    }
    for hops in out.values() {
        debug_assert!(hops.windows(2).all(|w| w[0] < w[1]), "the book iterates in ascending id");
    }
    let mut starts: Vec<ContractId> = out.values().flatten().copied().collect();
    starts.sort_unstable();
    for cid in starts {
        let c = &state.contracts[&cid];
        // A member cannot owe themselves — the audit refuses it — but a walk
        // that assumed so would loop rather than refuse.
        if c.creditor == c.debtor {
            continue;
        }
        let mut path = vec![cid];
        let mut seen = BTreeSet::from([c.debtor, c.creditor]);
        if walk_ring(state, &out, c.debtor, c.creditor, &mut path, &mut seen) {
            return Some(path);
        }
    }
    None
}

/// One step of `find_ring`'s walk. Recursive, and bounded by
/// `NETTING_MAX_RING`: a constant depth is not the unbounded recursion the
/// kernel's own search refuses.
fn walk_ring(
    state: &State,
    out: &std::collections::BTreeMap<MemberId, Vec<ContractId>>,
    start: MemberId,
    at: MemberId,
    path: &mut Vec<ContractId>,
    seen: &mut std::collections::BTreeSet<MemberId>,
) -> bool {
    let Some(hops) = out.get(&at) else { return false };
    for &cid in hops {
        let next = state.contracts[&cid].creditor;
        if next == start {
            path.push(cid);
            return true;
        }
        if path.len() + 1 >= k::NETTING_MAX_RING || seen.contains(&next) {
            continue;
        }
        path.push(cid);
        seen.insert(next);
        if walk_ring(state, out, start, next, path, seen) {
            return true;
        }
        seen.remove(&next);
        path.pop();
    }
    false
}

pub(crate) fn sweep_cranks(state: &mut State) {
    let due: Vec<ContractId> = state
        .contracts
        .values()
        .filter(|c| c.status == ContractStatus::Active)
        .filter(|c| state.epoch > c.maturity_epoch && c.outstanding > state.params.dust_minor())
        .map(|c| c.id)
        .collect();
    for id in due {
        let _ = mark_expired(state, id);
    }
    // **Rings of defaults, netted by their minimum.** After `mark_expired`,
    // because every hop has to be in default; before the forfeitures, because
    // netting lowers `open_default` and a member the ring relieved should not
    // be sanctioned on a figure the same sweep is about to correct.
    net_rings(state);
    let saturated: Vec<MemberId> = state
        .members
        .values()
        .filter(|m| m.bond_saturated_epochs >= state.params.bond_forfeit_epochs)
        .filter(|m| m.bond_enc() > state.params.dust_minor())
        .map(|m| m.id)
        .collect();
    for id in saturated {
        let _ = forfeit_bonds(state, id);
    }
    // Arbitration windows that have closed. Every attestation the window
    // received is in the median, so this is the only place an award is minted
    // — see `arb_award`.
    let closed: Vec<ContractId> = state
        .contracts
        .values()
        .filter(|c| !c.arb_awarded)
        .filter(|c| {
            c.arb
                .as_ref()
                .is_some_and(|a| state.epoch > horizon(c.created_epoch, a.window_epochs))
        })
        .map(|c| c.id)
        .collect();
    for id in closed {
        let _ = arb_award(state, id);
    }
    prune_closed_rows(state);
    retire_empty_rows(state);
}

/// Drop the rows that have stopped being able to say anything.
///
/// **A write budget bounds a RATE, and a row is a STOCK.** The allowance
/// refills every epoch, so what it buys over time is unbounded, while every
/// obligation and every proposal it buys is permanent: hashed into the state
/// root on every block, walked by every sweep, served by every view. Without a
/// retirement rule the ledger grows for ever at whatever rate the community
/// trades at, and there is no rate limit that fixes that — a limit bounds the
/// rate and the problem is the integral.
///
/// **Only rows that are CLOSED and past their windows.** An `Expired`
/// obligation is live: it is a default, the audit counts it, and its hold is
/// still reserved (a default releases no flow). A `Settled`, `Transferred` or
/// `Cured` one owes nothing, holds nothing, and answers only "this was paid" —
/// which the block history records and the stake the settlement conferred
/// already reflects. Retention runs from creation and never ends before the
/// arbitration window both parties agreed, because that window is the one
/// period in which the row itself is still evidence somebody may act on.
///
/// Ids are never reused (`next_contract`, `next_proposal` only ever rise), so a
/// pruned row cannot be confused with a later one, and an inclusion proof taken
/// while it was live still verifies against the root it was taken against — a
/// leaf salt binds the snapshot it belongs to.
fn prune_closed_rows(state: &mut State) {
    let epoch = state.epoch;
    state.contracts.retain(|_, c| {
        if matches!(c.status, ContractStatus::Active | ContractStatus::Expired) {
            return true;
        }
        let window = c.arb.as_ref().map(|a| a.window_epochs).unwrap_or(0);
        let keep_until = horizon(c.created_epoch, k::CLOSED_RETENTION_EPOCHS.max(window));
        epoch <= keep_until
    });
    state
        .proposals
        .retain(|_, p| epoch <= horizon(p.opened_epoch, k::PROPOSAL_RETENTION_EPOCHS));
}

/// Drop the rows that hold nothing, owe nothing and are named by nothing —
/// after `prune_closed_rows`, since a closed obligation names its parties for
/// as long as it is kept.
///
/// **A row is a stock and its seat is the one bond that does not return; an
/// empty row is not a stock.** The conditions are the whole of what a row can
/// hold or be held by, read in one pass each over the graph, the book, the
/// panels, the proposals, the guardian rolls and the seats, so the sweep costs
/// the size of the ledger once a boundary and never the members times the
/// edges. Every
/// one of them is something only the member, a counterparty or a ceremony can
/// put there, which is what makes retirement a rule and not a lever:
/// `k::ROW_RETENTION_EPOCHS` says the rest. A suspended row is never retired
/// — a sanction is not nothing.
fn retire_empty_rows(state: &mut State) {
    let epoch = state.epoch;
    let mut named: BTreeSet<MemberId> = BTreeSet::new();
    for &(c, d) in state.edges.keys() {
        named.insert(c as MemberId);
        named.insert(d as MemberId);
    }
    for c in state.contracts.values() {
        named.insert(c.debtor);
        named.insert(c.creditor);
        if let Some(a) = &c.arb {
            named.extend(a.arbiters.iter().copied());
            // The parties the panel binds, which a subrogated row's own no
            // longer name: an award is minted between them at the window's
            // close, and a party retired before then would be a claim on a
            // row that is gone.
            named.insert(a.debtor);
            named.insert(a.creditor);
        }
    }
    for p in state.proposals.values() {
        named.insert(p.author);
        named.extend(p.assents.iter().copied());
        match p.kind {
            ProposalKind::Suspend { member }
            | ProposalKind::Unsuspend { member }
            | ProposalKind::ValidatorPower { member, .. } => {
                named.insert(member);
            }
            _ => {}
        }
    }
    for m in state.members.values() {
        if let Some(g) = &m.guardian {
            named.extend(g.guardians.iter().copied());
        }
        // A row that sponsors a live seat is not empty: the seat names its
        // sponsor for the life of the row it bought, and invariant 7 asks that
        // a sponsor be a member. Retiring the sponsor first would orphan the
        // seat and halt every node at this very sweep.
        if let Some(seat) = &m.seat {
            named.insert(seat.sponsor);
        }
    }
    let empty: Vec<MemberId> = state
        .members
        .values()
        .filter(|m| matches!(m.status, MemberStatus::Active | MemberStatus::Exited))
        .filter(|m| epoch > horizon(m.joined_epoch, k::ROW_RETENTION_EPOCHS))
        .filter(|m| !named.contains(&m.id))
        .filter(|m| m.debt_out == 0 && m.rep.open_default == 0 && m.bond_enc() == 0 && m.pending_rotation.is_none())
        .filter(|m| {
            !state.forfeit_reserve.contains_key(&m.id)
                && !state.underwriters.contains_key(&m.id)
                && !state.validators.contains_key(&m.id)
        })
        .map(|m| m.id)
        .collect();
    for id in empty {
        retire_row(state, id);
    }
}

/// Retire one row: unwind its cascade listings both ways as `exit` does, give
/// its seat back to the sponsor's reach, free its keys for a later seating,
/// and drop it. The id is never reused — `next_member` only rises — so a seat
/// another row holds on an arc that names this index stays a valid hold.
fn retire_row(state: &mut State, id: MemberId) {
    let Some(m) = state.members.get(&id).cloned() else { return };
    for b in m.beneficiaries.keys() {
        if let Ok(bm) = member_mut(state, *b) {
            bm.supporters_of.remove(&id);
            bm.approved_supporters.remove(&id);
        }
    }
    for l in &m.supporters_of {
        if let Ok(lm) = member_mut(state, *l) {
            lm.beneficiaries.remove(&id);
        }
    }
    if let Some(seat) = &m.seat {
        state.release_seat(&seat.held);
    }
    for key in &m.keys {
        state.key_index.remove(key);
    }
    state.members.remove(&id);
}

// ------------------------------------------------------------ bond gate --

/// Charge this transition's operation bond, or refuse it.
///
/// Runs after the envelope checks and BEFORE dispatch, and — like
/// `record_applied` above — charges UNCONDITIONALLY once the gate is cleared,
/// whether or not the dispatched transition then succeeds. That is the half
/// that does the anti-spam work. A bond refunded on failure would leave the
/// cheapest attack in the alphabet completely unpriced: submit transitions
/// designed to fail, consume a consensus round and a durable write for each,
/// and pay nothing. Honest clients are unaffected, because a transition that
/// fails validation was a client bug either way and the bond returns on
/// schedule regardless.
///
/// **A refusal here leaves nothing behind, because nothing was admitted.**
/// Uniformity with every other rejection would argue for burning the id anyway;
/// the two branches below charge nothing on the way out, so recording first
/// would make a gate refusal the one durable write in the alphabet that nobody
/// pays for — which is the attack the note above says must be impossible.
/// Ordering is what settles it: gate, then record, then dispatch. A rejection
/// at DISPATCH still spends its id, which is where the deferred-replay policy
/// actually bites.
///
/// The decision itself lives in `bond::due`, shared verbatim with the
/// advisory screens at mempool admission and block proposal. Only the
/// mutations — spending an allowance slot, encumbering a bond, recording a
/// denial — happen here, because only here is authoritative. What was charged
/// is returned to the caller so that `refund` can undo it exactly, for the one
/// case where dispatch then finds the envelope authorised nothing at all.
fn bond_gate(
    state: &mut State,
    tx: &Tx,
    signers: &[Key],
    gate: &mut crate::bond::GateCache,
) -> Res<(Charged, Option<MemberId>)> {
    match crate::bond::due_with_cache(state, tx, signers, gate) {
        // A zero-bond class: the permissionless cranks and the recovery path.
        crate::bond::Due::Free => Ok((Charged::Free, None)),
        // The allowance carries a member whose HEADROOM is spent, not a member
        // with nothing behind them: `bond::due` grants it only where
        // `conferrable > dust`, because a per-key allowance is the
        // free-signature bound's own defect one layer down. What carries a
        // newcomer is the counterparty — their first trade is billed to the
        // established member who chose to make it, which is the same member
        // already taking the first uninsured risk.
        crate::bond::Due::Allowance(payer) => {
            member_mut(state, payer)?.bond_free_used += 1;
            Ok((Charged::Allowance(payer), None))
        }
        // The row's own seat slot: spent here whether or not dispatch then
        // accepts the transition, exactly as an allowance slot is — a refused
        // first write is not retried free.
        crate::bond::Due::Slot(payer) => {
            member_mut(state, payer)?.seat_slot = false;
            Ok((Charged::Slot(payer), None))
        }
        // **A row is a stock and the work is a rate, so a seating trade is
        // charged twice from two budgets.** Order is the whole of it: status,
        // then the work bond exactly as `Due::Bond` charges it, then the seat
        // reach — which arms NOTHING and charges NOTHING when it refuses,
        // because a member who has brought in as many people as their backing
        // carries has not abused anything, and arming the saturation counter
        // there would let the epoch sweep forfeit their bonds for it.
        crate::bond::Due::Seat { payer, bond, rows } => {
            if !matches!(member(state, payer)?.status, MemberStatus::Active) {
                return Err(Error(ET_BOND_STATUS));
            }
            if bond > state.bond_headroom_minor(payer) {
                member_mut(state, payer)?.bond_denied_this_epoch = true;
                return Err(Error(ET_BOND_EXHAUSTED));
            }
            if !state.can_seat_minor(payer, rows as u64) {
                return Err(Error(ET_BOND_SEAT_UNBACKED));
            }
            let release_at = state.epoch.saturating_add(state.params.bond_release_epochs.max(1));
            *member_mut(state, payer)?.bonds.entry(release_at).or_insert(0) += bond;
            Ok((Charged::Bond { payer, release_at, amount: bond }, Some(payer)))
        }
        crate::bond::Due::Bond(payer, bond) => {
            // **A budget zeroed by STATUS is not a budget that was spent**, and
            // the two must not share a refusal. `bond_headroom` returns 0.00
            // for any member that is not `Active`, so a suspended member's
            // every priced transition landed on `ET-BND-001` below — telling
            // them to wait for a release that cannot come, because nothing was
            // ever charged — and, because that branch arms the saturation
            // counter, the epoch sweep then forfeited the bonds they were
            // holding from BEFORE the suspension. Measured: 20.00 encumbered,
            // gone permanently into `forfeit_reserve` in three epochs, for
            // nothing but trying to use their own wallet.
            //
            // The counter exists to catch a member who kept pushing past a
            // limit they could have respected. A status is not that limit, and
            // a sanction the community voted for must not quietly grow a second
            // one nobody proposed. Refused here, before the counter, with the
            // reason that is actually true.
            if !matches!(member(state, payer)?.status, MemberStatus::Active) {
                return Err(Error(ET_BOND_STATUS));
            }
            if bond > state.bond_headroom_minor(payer) {
                // Record the denial for the saturation counter that
                // `begin_block` folds at the epoch boundary. Written even
                // though we are about to return an error, because the whole
                // forfeiture path depends on knowing the member kept pushing
                // after being refused.
                member_mut(state, payer)?.bond_denied_this_epoch = true;
                return Err(Error(ET_BOND_EXHAUSTED));
            }
            let release_at = state.epoch.saturating_add(state.params.bond_release_epochs.max(1));
            *member_mut(state, payer)?.bonds.entry(release_at).or_insert(0) += bond;
            Ok((Charged::Bond { payer, release_at, amount: bond }, None))
        }
        // A priced class nobody present can pay for: a refusal rather than a
        // free pass, because a trade may name a party by key, and admitting an
        // unpayable transition would let a ring of free keys write accounts
        // into the ledger for nothing.
        crate::bond::Due::Unpayable => Err(Error(ET_BOND_NO_PAYER)),
    }
}

// ---------------------------------------------------------------- helpers --

/// What `debtor`'s cached debt would become if they took on `amount` — or a
/// refusal, where the sum is one a `u64` cannot hold.
///
/// **Asked before the first write on every path that mints a claim**, because
/// `dispatch` has no rollback: an overflow discovered after a reservation has
/// been taken or an original discharged is a refusal that has already moved
/// the book. It is the ingress ceiling that makes this unreachable in
/// practice — it takes 8,192 ceiling-sized obligations on one debtor to arrive
/// here, so the amount bound is the fix and this is what keeps the failure a
/// refusal rather than a panic in one build and a silent wrap in the other.
pub(crate) fn debt_after(state: &State, debtor: MemberId, amount: u64) -> Res<u64> {
    member(state, debtor)?
        .debt_out
        .checked_add(amount)
        .ok_or(Error(ET_CTR_BAD_AMOUNT))
}

pub(crate) fn member(state: &State, id: MemberId) -> Res<&Member> {
    state.members.get(&id).ok_or(Error(ET_MEM_UNKNOWN))
}

pub(crate) fn member_mut(state: &mut State, id: MemberId) -> Res<&mut Member> {
    state.members.get_mut(&id).ok_or(Error(ET_MEM_UNKNOWN))
}

pub(crate) fn contract(state: &State, id: ContractId) -> Res<Contract> {
    state.contracts.get(&id).cloned().ok_or(Error(ET_CTR_UNKNOWN))
}

pub(crate) fn contract_mut(state: &mut State, id: ContractId) -> Res<&mut Contract> {
    state.contracts.get_mut(&id).ok_or(Error(ET_CTR_UNKNOWN))
}

/// The epoch `delta` epochs after `base`, saturating.
///
/// Every deadline in the state machine is `state.epoch + something`, and each
/// `something` reaches this from a different place — a transaction field, a
/// governed parameter, a guardian config. Saturating here is the structural
/// backstop: the callers that take a value from a transaction bound it against
/// `k::MAX_HORIZON_EPOCHS` first and report a named error, which is the check
/// that carries the meaning; this one exists so that no arithmetic in this
/// file can panic or wrap even if some future caller forgets, or if a value
/// arrives from a genesis file this node did not author. A saturated deadline
/// is `u64::MAX` — unreachable, hence fail-closed for a window that must
/// elapse — never a deadline in the past.
pub(crate) fn horizon(base: u64, delta: u64) -> u64 {
    base.saturating_add(delta)
}

/// Is this key already spoken for — by a member, or by a rotation waiting out
/// its veto window?
///
/// `key_index` alone is not the answer, because `rotate_request` stores the
/// incoming keys in `pending_rotation` (public, replicated state) and does not
/// index them until `rotate_finalize`. A key sitting in that gap could be
/// claimed by an ordinary admission, and the later finalize would then
/// overwrite the index entry — leaving one key in two members' `keys`, one of
/// them unresolvable by `member_of_key` for the rest of its life.
pub(crate) fn key_is_claimed(state: &State, key: &Key) -> bool {
    // A registered consensus key is claimed too. `set_consensus_key` refuses a
    // member key; without this half a rotation or a seating could take a
    // validator's consensus key as a member key, and one key was two
    // identities — the separation enforced in one direction only.
    state.key_index.contains_key(key)
        || state.members.values().any(|m| {
            m.consensus_key == Some(*key) || m.pending_rotation.as_ref().is_some_and(|p| p.new_keys.contains(key))
        })
}

pub(crate) fn require_signed(state: &State, id: MemberId, signers: &[Key]) -> Res<()> {
    if signers.iter().any(|s| member(state, id).map(|m| m.has_key(s)).unwrap_or(false)) {
        Ok(())
    } else {
        member(state, id)?;
        Err(Error(ET_MEM_NOT_SIGNER))
    }
}

/// The member's own signature, or a threshold of their guardians' — the
/// authority a CREDITOR's discharge may carry.
///
/// Every discharge needs the creditor's signature, and a creditor who has
/// lost their keys or stopped answering left their debtor with no way to
/// pay: the obligation matured, defaulted, and an uninsured default
/// substitutes nothing, so the debtor held an open default for ever — no
/// allowance, no exit, no cure. Guardians already hold the power to rotate
/// the creditor's key and then sign as them, so letting a threshold of them
/// sign a discharge in the creditor's stead adds no trust the creditor had
/// not already placed. It is the creditor's side only: a debtor's guardians
/// signing a payment would be a debtor declaring their own payment.
pub(crate) fn signed_or_guardians(state: &State, id: MemberId, signers: &[Key]) -> Res<()> {
    if require_signed(state, id, signers).is_ok() {
        return Ok(());
    }
    if let Some(cfg) = &member(state, id)?.guardian {
        let signing = cfg
            .guardians
            .iter()
            .filter(|&&g| require_signed(state, g, signers).is_ok())
            .count() as u32;
        if signing >= cfg.threshold {
            return Ok(());
        }
    }
    Err(Error(ET_MEM_NOT_SIGNER))
}

/// Did some ACTIVE validator sign this transaction?
///
/// The charter's own signature, on-ledger: validators are the charter
/// institutions (the paper's §Recourse), so this is how an operation
/// authorised by the membrane rather than by a peer is expressed. Used by the
/// two admission-path branches that have no sponsor to check — never as a
/// general "validators may do anything" shortcut, which is why it is
/// consulted only where a sponsor set is empty.
///
/// Record a discharge as stake — the ONLY way the stake graph grows.
///
/// ```text
/// stake(c,d) = max(stake(c,d), min(repaid, conferrable(c)))
/// ```
///
/// `repaid` is what THIS obligation has repaid in total — its original less
/// what is still outstanding — and not the installment that arrived. The
/// paper's definition is stated over the obligation's amount when it settles;
/// read per installment instead, a member who honoured 1,000 in two halves
/// held a stake of 500 beside one who paid at once and held 1,000, for the
/// same evidence. The peak over cumulative repayment equals the definition at
/// full settlement and counts installments on the way, and is still a peak
/// under the same cap, so the wash bound is untouched.
///
/// Three properties carry the security and all three are in that one line.
///
/// **Directed.** The edge runs creditor → debtor, so standing is evidence of
/// having OWED and paid. Held symmetrically, extending credit would raise the
/// LENDER's own limit — a member could grow their capacity by lending, which
/// is backwards. It is also why selling earns nothing: 2000 settled sales to
/// twenty well-backed members leave the seller at zero, and one honoured
/// purchase gives them standing at once.
///
/// **A peak, not a sum.** Running the same cycle between one pair a thousand
/// times raises it exactly as high as running it once, so a wash loop cannot
/// accumulate and no detector is needed for one.
///
/// **Capped by what the creditor may confer.** Between two accounts that may
/// confer nothing, every settlement stakes zero however much volume passes.
/// This is why the measure is stake rather than volume: settlement takes two
/// signatures and no delivery, so volume is free to fabricate, while a stake
/// is capped by a quantity the fabricator had to earn.
pub(crate) fn discharge_credit(state: &mut State, debtor: MemberId, creditor: MemberId, amount: u64) -> Res<()> {
    if amount <= state.params.dust_minor() {
        return Ok(());
    }
    // What the creditor may confer as it stands now: their declared supply if
    // they are an underwriter, otherwise their own capacity. Read here rather
    // than pinned at acceptance so the rule lives in one place; the two differ
    // only if the creditor's own backing moved while the obligation was
    // outstanding, and then the present figure is the honest one.
    //
    // **"Now" is after the reservation has followed the debt down.** Every
    // caller runs `rehold` first, because a capacity is read on the RESIDUAL
    // graph and the obligation being paid holds a reservation on it: where
    // that reservation ran through the creditor's own backing arcs, the
    // creditor's conferrable read before the release is the shadow of their
    // own loan. Measured — one underwriter backing A for 100, A backing X and
    // Y for 100 each, Y backing S for 100: X lends S 100 insured, the
    // reservation runs U→A→Y→S and saturates U→A, which is X's only backing;
    // S pays, and X's stake on S reads 0.00 with the release after the stake
    // and 100.00 with it before. A creditor who honoured a loan and was paid
    // conferred nothing for it, by the accident of which path the solver
    // took. `tests/model.rs` holds the order.
    let conferrable = state.conferrable_minor(creditor);
    state.record_stake(creditor, debtor, amount, conferrable);
    Ok(())
}

/// Re-hold exactly what an insured obligation still owes, after its
/// outstanding amount has moved.
///
/// The hold SHRINKS where it is, by route (`loss::shrink`): the share just
/// paid is released from the arcs that were holding it, and nothing else
/// moves. Not scaled arc by arc — scaling is inexact under integer rounding,
/// so `Σ committed` would drift from the book and invariant 2 asks for
/// equality, and a scaled-down flow is no longer a flow. And not re-solved: a
/// re-solve is a full network build per partial payment, and a settle is free
/// and bounded only by its amount, so every partial payment of one minor unit
/// cost every validator a max-flow — 2.3 ms at 10,000 accounts against a
/// microsecond for the shrink, four thousand to one, for the price of one
/// allowance slot. First-come conservatism is the rule the insured tier
/// already runs on — a reservation never reroutes an earlier one — and a
/// partial payment does not get to either.
///
/// The one reservation this still takes is for a row that holds nothing and
/// owes something: the cascade's remainder path, which empties a routed
/// original's hold before the buyer's reservation is taken and asks afresh for
/// what stays with the seller. That reservation cannot fail, and the reason is
/// structural rather than hopeful: decay floors every edge at its live
/// reservation and §Stability floors every supply at its committed flow. The
/// uninsured fallback is written only because a validator may not panic on any
/// input.
pub(crate) fn rehold_public(state: &mut State, contract_id: ContractId) -> Res<()> {
    rehold(state, contract_id)
}

fn rehold(state: &mut State, contract_id: ContractId) -> Res<()> {
    let c = contract(state, contract_id)?;
    if !c.insured {
        return Ok(());
    }
    let live = matches!(c.status, ContractStatus::Active | ContractStatus::Expired);
    let want = if live { c.outstanding } else { 0 };
    // **A hold shrinks where it is; it is never re-solved.** For a DEFAULTED
    // claim that is §Recourse's load-bearing line — "the supply arc stays
    // committed until the defaulter repays the underwriter" — and re-solving
    // broke it: the solver answers with whatever path the residual offers, so
    // a defaulter who honoured one trade with a third party handed it a cheaper
    // arc and the throttle landed on an underwriter who was never substituted
    // (committed `{U_b: 300}` → `{U_a: 200}` on a cure of 100). For a LIVE claim
    // it is the cost: a re-solve is a network build per partial payment, free
    // to the payer and paid by every validator (`rehold_public` has the
    // figures). `loss::shrink` says the rest; a row closing gives everything
    // back through the same division, with nothing kept.
    let total = c.held.amount();
    if total > 0 {
        let keep = want.min(total);
        if keep == total {
            return Ok(());
        }
        let (still, back) = crate::loss::shrink(&c.held, c.debtor as usize, keep, total);
        state.release_capacity(&back);
        contract_mut(state, contract_id)?.held = still;
        return Ok(());
    }
    // Nothing held and something owed: the cascade's remainder path.
    if want > state.params.dust_minor() {
        let held = state.reserve_capacity_minor(c.debtor, want);
        let cm = contract_mut(state, contract_id)?;
        match held {
            Some(h) => cm.held = h,
            None => {
                cm.held = Default::default();
                cm.insured = false;
            }
        }
    }
    Ok(())
}

pub(crate) fn require_active(state: &State, id: MemberId) -> Res<()> {
    match member(state, id)?.status {
        MemberStatus::Active => Ok(()),
        _ => Err(Error(ET_MEM_NOT_ACTIVE)),
    }
}

// ------------------------------------------------------------- membership --

fn register_guardians(
    state: &mut State,
    member_id: MemberId,
    guardians: Vec<MemberId>,
    threshold: u32,
    veto_window_epochs: u64,
    signers: &[Key],
) -> Res<()> {
    require_signed(state, member_id, signers)?;
    if threshold < k::GUARDIAN_MIN || (guardians.len() as u32) < threshold {
        return Err(Error(ET_ROT_THRESHOLD));
    }
    // The window is the whole of the veto's protection: it is the time the
    // account holder has to notice a rotation they did not ask for and refuse
    // it. `veto_window_epochs: 0` made `RotateVeto` structurally unreachable —
    // request and finalize in one block — turning a threshold of compromised
    // guardians into an immediate takeover. `k::VETO_WINDOW_EPOCHS` existed
    // for exactly this and was referenced by nothing; it is the floor, and
    // `MAX_HORIZON_EPOCHS` is the ceiling that keeps the deadline arithmetic
    // in range and stops a member locking their own rotation out forever.
    if !(k::VETO_WINDOW_EPOCHS..=k::MAX_HORIZON_EPOCHS).contains(&veto_window_epochs) {
        return Err(Error(ET_ROT_BAD_WINDOW));
    }
    for &g in &guardians {
        member(state, g)?;
        if g == member_id {
            return Err(Error(ET_ROT_THRESHOLD));
        }
    }
    let m = member_mut(state, member_id)?;
    m.guardian = Some(GuardianConfig { guardians: guardians.into_iter().collect(), threshold, veto_window_epochs });
    // A rotation is authorised by a specific guardian set under a specific
    // window; replacing that set withdraws the authority the request was made
    // under, so the request does not survive it. Leaving it in place inverted
    // the victim's only defence: revoking compromised guardians and installing
    // trusted ones with a shorter window left the attacker's request standing
    // AND measured it against the NEW, shorter window — making the takeover
    // finalizable sooner than if the victim had done nothing.
    m.pending_rotation = None;
    Ok(())
}

fn rotate_request(state: &mut State, member_id: MemberId, new_keys: Vec<Key>, signers: &[Key]) -> Res<()> {
    let cfg = member(state, member_id)?.guardian.clone().ok_or(Error(ET_ROT_NO_GUARDIANS))?;
    // The storage bound — see `MAX_KEYS_PER_ACCOUNT`. A rotation writes its
    // whole key list into replicated state for a single bond, so without this
    // the ledger's key index is unbounded in a transition priced once.
    if new_keys.is_empty() || new_keys.len() > MAX_KEYS_PER_ACCOUNT {
        return Err(Error(ET_ADM_BAD_KEYS));
    }
    if new_keys.iter().any(|nk| key_is_claimed(state, nk)) {
        return Err(Error(ET_ADM_DUP_KEY));
    }
    let signing_guardians = cfg
        .guardians
        .iter()
        .filter(|&&g| require_signed(state, g, signers).is_ok())
        .count() as u32;
    if signing_guardians < cfg.threshold {
        return Err(Error(ET_ROT_THRESHOLD));
    }
    let epoch = state.epoch;
    member_mut(state, member_id)?.pending_rotation = Some(PendingRotation { new_keys, opened_epoch: epoch });
    Ok(())
}

/// The account holder's stop. It DELETES the request rather than flagging it:
/// a vetoed request has done everything it can ever do, and a request left
/// standing with a flag on it was a precondition a second veto could consume
/// again — free, and a durable replay id each time. The second veto is now
/// `ET-ROT-003`, which is what makes "every free class refuses its own second
/// call" a true sentence for this one.
fn rotate_veto(state: &mut State, member_id: MemberId, signers: &[Key]) -> Res<()> {
    require_signed(state, member_id, signers)?;
    let m = member_mut(state, member_id)?;
    if m.pending_rotation.is_none() {
        return Err(Error(ET_ROT_NO_REQUEST));
    }
    m.pending_rotation = None;
    Ok(())
}

fn rotate_finalize(state: &mut State, member_id: MemberId) -> Res<()> {
    let m = member(state, member_id)?;
    let cfg = m.guardian.clone().ok_or(Error(ET_ROT_NO_GUARDIANS))?;
    let p = m.pending_rotation.clone().ok_or(Error(ET_ROT_NO_REQUEST))?;
    if state.epoch < horizon(p.opened_epoch, cfg.veto_window_epochs) {
        return Err(Error(ET_ROT_WINDOW_OPEN));
    }
    let old_keys = member(state, member_id)?.keys.clone();
    for old in old_keys {
        state.key_index.remove(&old);
    }
    for nk in &p.new_keys {
        state.key_index.insert(*nk, member_id);
    }
    let m = member_mut(state, member_id)?;
    m.keys = p.new_keys;
    m.pending_rotation = None;
    Ok(())
}

/// Register, rotate or retire the key this member's validator signs consensus
/// with. **It never becomes a `keys` entry, so it can never sign a
/// transaction**, and it is not in `key_index`.
///
/// One key for both roles is a validator host compromise handing the attacker
/// every `Accept`, `Settle` and `DeclareSupply` that key can sign — and a
/// recovery phrase that, restored into the wallet, makes a handset a hot
/// consensus key.
///
/// **Uniqueness is checked against both namespaces**, because a key that is
/// also a member key would put an economic signature back on the server this
/// separation exists to keep one off — and a key two members share is one
/// identity the validator set cannot resolve.
///
/// Retiring it (`None`) is refused while the member still holds voting power:
/// the set has to be able to name a key for every validator in it, and
/// `EdetValidatorSet::build` fails closed rather than shrinking the quorum.
fn set_consensus_key(state: &mut State, member_id: MemberId, key: Option<Key>, signers: &[Key]) -> Res<()> {
    require_active(state, member_id)?;
    require_signed(state, member_id, signers)?;
    match key {
        Some(k) => {
            if state.key_index.contains_key(&k) {
                return Err(Error(ET_VAL_KEY_IN_USE));
            }
            if state.members.values().any(|m| m.id != member_id && m.consensus_key == Some(k)) {
                return Err(Error(ET_VAL_KEY_IN_USE));
            }
            member_mut(state, member_id)?.consensus_key = Some(k);
        }
        None => {
            if state.validators.contains_key(&member_id) {
                return Err(Error(ET_VAL_NO_CONSENSUS_KEY));
            }
            member_mut(state, member_id)?.consensus_key = None;
        }
    }
    Ok(())
}

// ----------------------------------------------------------- underwriting --

/// Resize or leave the underwriter role. **It cannot be taken through this
/// door, and a declaration cannot be raised through it either.**
///
/// **A raise is refused outright** (`ET-UWR-ABOVE-CAPACITY`). Capping a
/// declaration by the declarer's own CAPACITY reads correctly one party at a
/// time and is a supply of hollow insurance in aggregate: capacity is conferred
/// by the community, so a member declares against it, the declaration becomes a
/// source arc feeding the next member's capacity, and that becomes their
/// declaration. Twelve joiners wash-backing each other behind a seed of 100
/// reach a declared 204,900 — and borrow 204,800 from honest creditors,
/// **labelled insured by the ledger**, with every invariant satisfied. The
/// aggregate consolation that suggests itself — "what those twelve can owe
/// together stays at 100" — is a statement about the DECLARERS as a set, and
/// the credit goes to the sybils they back, who are not in it.
/// `tests/adversarial.rs` measures both halves.
///
/// So supply now has exactly one source: a ceremony. Genesis
/// (`State::add_underwriter`) and `seed::enact` (§Governance's endorsed
/// amendment, rate-bounded and assented) are the whole of it, and the role
/// stays OPEN through the second of those rather than through this
/// transition. What a member may do here is withdraw — restating an unchanged
/// supply is still legal, since `want == current` is not a raise.
///
/// **Lowering is floored at the flow already committed through them** (§Stability). A
/// withdrawal is decay applied to a source arc and takes the same floor: the
/// debt did not shrink because the underwriter changed their mind. Measured
/// without it, one of six underwriters leaving a fully drawn community gives
/// capacity 12,500 against 15,000 outstanding — the cut bound broken outright.
fn declare_supply(state: &mut State, member_id: MemberId, supply: f64, signers: &[Key]) -> Res<()> {
    require_signed(state, member_id, signers)?;
    // Lowering is the recovery path, and it belongs to a suspended member too.
    // Gated on `Active`, a suspended underwriter could neither lower their
    // declaration nor leave (`exit` refuses a declared supply) nor vote, while
    // their supply went on insuring and every default through it landed on
    // them — a sanction that made its target a permanent involuntary insurer,
    // and a lever for whoever holds the ordinary bar. A raise is refused for
    // everybody below; an exited row has left and is refused here.
    if member(state, member_id)?.status == MemberStatus::Exited {
        return Err(Error(ET_MEM_NOT_ACTIVE));
    }
    if !supply.is_finite() || supply < 0.0 {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    let want = State::to_minor(supply);
    let current = state.underwriters.get(&member_id).copied().unwrap_or(0);
    if want > current {
        return Err(Error(ET_UWR_ABOVE_CAPACITY));
    } else if want < state.supply_floor(member_id) {
        return Err(Error(ET_UWR_BELOW_COMMITTED));
    }
    if want == 0 {
        state.underwriters.remove(&member_id);
    } else {
        state.underwriters.insert(member_id, want);
    }
    Ok(())
}

// -------------------------------------------------------------- contracts --

/// Book a fresh Active obligation on `debtor`, reserving the flow that
/// justifies it.
///
/// Capacity bounds what the community UNDERWRITES, not what a member may
/// choose to risk, so a reservation that cannot be found is not an error. The
/// obligation is simply **uninsured**: it holds nothing, triggers no community
/// recourse on default, and the creditor bears it alone. Both parties signed,
/// so both consented to that.
///
/// This is what dissolves the bootstrap. A community whose accounts all start
/// at zero would deadlock if capacity were a permission — the first trade
/// could never happen. It is not a permission. First trades are uninsured,
/// they settle, they create stakes, and capacity is their residue.
pub(crate) fn book(
    state: &mut State,
    debtor: MemberId,
    creditor: MemberId,
    amount: u64,
    maturity_epoch: u64,
    arb: Option<ArbTerms>,
) -> Res<ContractId> {
    // Before `reserve_capacity_minor`, which WRITES: a refusal after it would
    // leave flow held for an obligation that does not exist. `open_obligation`
    // asks again for the paths that reach it with a reservation already in
    // hand.
    debt_after(state, debtor, amount)?;
    // **Insured only within the horizon the electorate stands behind**,
    // measured from this acceptance (`Params::insured_horizon_epochs`). Past
    // it the claim is booked uninsured — both signatures are already on it,
    // and the creditor's is the one that matters — and keeping a longer claim
    // insured is settling and re-accepting it, which re-prices it on the
    // current cut. That is the one place an underwriter's exposure in TIME is
    // bounded; the amount was bounded already.
    let held = if maturity_epoch <= horizon(state.epoch, state.params.insured_horizon_epochs()) {
        state.reserve_capacity_minor(debtor, amount)
    } else {
        None
    };
    open_obligation(state, debtor, creditor, amount, maturity_epoch, arb, held, state.epoch)
}

/// Is this claim's date still inside the insured horizon measured from its
/// acceptance? The question every debtor swap asks before it reserves for a
/// successor, and `extend` asks before it keeps a hold.
pub(crate) fn within_insured_horizon(state: &State, c: &Contract) -> bool {
    c.maturity_epoch <= horizon(c.accepted_epoch, state.params.insured_horizon_epochs())
}

/// Book a fresh Active obligation on `debtor` (contract row + caches) against
/// a reservation already taken — `None` for an uninsured obligation.
///
/// Separate from `book` because a conditional leg reserves at PROPOSAL time
/// and carries that reservation to fulfilment: the flow a pending conditional
/// holds is the flow the obligation ends up holding, not a fresh one. Taking
/// it twice would double-count, and taking it late would lose the race the
/// reservation exists to win.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_obligation(
    state: &mut State,
    debtor: MemberId,
    creditor: MemberId,
    amount: u64,
    maturity_epoch: u64,
    arb: Option<ArbTerms>,
    held: Option<edet_kernel::flow::Held>,
    accepted_epoch: u64,
) -> Res<ContractId> {
    // Before the row, never after it: a refusal past the insert would leave a
    // contract on the book with nothing behind it in `debt_out`, which is a
    // conservation violation minted by the refusal itself.
    let debt_out = debt_after(state, debtor, amount)?;
    let id = state.next_contract;
    state.next_contract += 1;
    state.contracts.insert(
        id,
        Contract {
            id,
            debtor,
            creditor,
            outstanding: amount,
            original: amount,
            maturity_epoch,
            status: ContractStatus::Active,
            created_epoch: state.epoch,
            accepted_epoch,
            insured: held.is_some(),
            held: held.unwrap_or_default(),
            arb,
            arb_attestations: Default::default(),
            arb_awarded: false,
        },
    );
    let d = member_mut(state, debtor)?;
    d.debt_out = debt_out;
    d.rep.d_in += State::from_minor(amount);
    Ok(id)
}

/// Book an obligation.
///
/// There is no capacity GATE here, and that is the point. Capacity bounds what
/// the community underwrites, not what a member may choose to risk: within the
/// debtor's capacity the obligation reserves flow and the recourse machinery
/// stands behind it, beyond it the creditor may still lend and bears it alone.
/// Both parties signed, so both consented to which of the two they got.
#[allow(clippy::too_many_arguments)]
fn accept(
    state: &mut State,
    debtor: Party,
    creditor: Party,
    amount: f64,
    maturity_epochs: u64,
    arb: Option<ArbTermsWire>,
    signers: &[Key],
    sponsor: Option<MemberId>,
) -> Res<()> {
    let debtor = resolve(state, &debtor, signers)?;
    let creditor = resolve(state, &creditor, signers)?;
    if debtor == creditor {
        return Err(Error(ET_CTR_SELF_DEAL));
    }
    // Origination, on both sides. Suspension revokes it — extending credit is
    // originating just as much as accepting it — while an account nobody has
    // ever backed is refused by nothing at all, because it can owe nothing the
    // community underwrites and needs no permission to owe the rest. Which is
    // exactly why a party can be created here: the newcomer is refused by
    // nothing, so there is nothing for anyone to approve.
    require_party(state, debtor, signers)?;
    require_party(state, creditor, signers)?;
    let arb = check_arb(state, arb, debtor, creditor)?;
    if amount <= state.params.dust || !State::amount_representable(amount) {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    // THE BOUNDARY. Past here nothing in this transition is an `f64`: the
    // ledger stores minor units and every comparison downstream is integer.
    // Both ends of it are refused above rather than clamped here: `to_minor`
    // floors a NaN to zero and saturates a `1e300` to `u64::MAX`, and neither
    // is a rounding of what two parties signed.
    let amount = State::to_minor(amount);
    if maturity_epochs < state.params.min_maturity_epochs {
        return Err(Error(ET_CTR_MATURITY_TOO_SHORT));
    }
    if maturity_epochs > k::MAX_HORIZON_EPOCHS {
        return Err(Error(ET_CTR_MATURITY_TOO_LONG));
    }
    let maturity_epoch = horizon(state.epoch, maturity_epochs);
    // Past every refusal, so this is where a row may appear. `book` below
    // cannot fail against ids that exist, which these now do — and the panel's
    // terms can now name them.
    let (debtor, creditor) = seat_pair(state, debtor, creditor, sponsor)?;
    let arb = arb.map(|(t, cap)| t.into_stored(cap, debtor, creditor, amount));
    book(state, debtor, creditor, amount, maturity_epoch, arb)?;
    Ok(())
}

/// Validate consented arbitration terms. Pinned at acceptance and immutable
/// after: a panel both parties named, capped and time-boxed before the
/// obligation existed, which is what makes an award an authorisation by the
/// party who loses if it is wrong rather than by a third party who does not.
fn check_arb(
    state: &State,
    arb: Option<ArbTermsWire>,
    debtor: Resolved,
    creditor: Resolved,
) -> Res<Option<(ArbTermsWire, u64)>> {
    let Some(t) = arb else { return Ok(None) };
    // A party still being seated cannot be on the panel: every arbiter must
    // already be a member (below), and a `Fresh` party is not one yet.
    let is_party = |id: &MemberId| [debtor, creditor].contains(&Resolved::Existing(*id));
    if t.quorum == 0
        || (t.arbiters.len() as u32) < t.quorum
        || (t.arbiters.len() as u32) > k::N_ARB
        || t.window_epochs == 0
        || t.window_epochs > k::MAX_HORIZON_EPOCHS
        || !State::amount_representable(t.award_cap)
        || t.arbiters.iter().any(is_party)
    {
        return Err(Error(ET_ARB_BAD_TERMS));
    }
    for &a in &t.arbiters {
        member(state, a)?;
    }
    // Converted here and nowhere else: the refusals above are the ones
    // `to_minor` cannot make, because it clamps a NaN and a negative to zero
    // and saturates anything past the ceiling, rather than rejecting any of
    // them. A malformed ceiling is a malformed TERM, which is why it is
    // refused beside the panel and the window rather than on its own code.
    // The parties are bound into the stored terms by `accept`, once they have
    // ids — a `Fresh` party has none here.
    let cap = State::to_minor(t.award_cap);
    Ok(Some((t, cap)))
}

// ---------------------------------------------------------- the alphabet --

/// Move the debtor of a claim: the old debtor discharges, the successor is
/// booked against the new debtor's own standing.
fn move_debtor(
    state: &mut State,
    c: &Contract,
    new_debtor: MemberId,
    held: Option<edet_kernel::flow::Held>,
) -> Res<()> {
    let amount = c.outstanding;
    // **A debtor swap is not a settlement, so it writes no stake.** §Standing grows
    // the graph only at settlement, and the creditor's signature is what
    // authorises an edge — an insured→insured `Transfer` asks them for nothing
    // (see `transfer` below), so a stake written here is one no creditor ever
    // placed. The linearity corollary assumes ∂S is written by outsiders; this was
    // insiders writing it.
    //
    // Measured at the quantifier, with a stake written here: C lends D 300
    // once, and the claim is passed around a ring of k members, two signatures
    // per hop. Every hop writes `stake(C, old debtor)` at `conferrable(C)`, so
    // `Σ stake(C, ·)` comes out LINEAR in k — 600 at k=2, 3000 at k=10 — from
    // one 300 acceptance C signed once. The cut still bounds the coalition's
    // aggregate at C's own inflow, which is why no invariant fires; what a cut
    // does not bound is each member's OWN standing, and `conferrable` is what
    // `bond_headroom`, the cascade's drain cap and the establishment floor all
    // read. The successor's eventual settlement writes `stake(C, successor)`,
    // which is the edge C did accept.
    // Before the old claim is discharged, because this transition writes
    // before it books: a successor refused after the release would leave the
    // original extinguished and nothing standing in its place.
    debt_after(state, new_debtor, amount)?;
    state.release_capacity(&c.held);
    {
        let old = member_mut(state, c.debtor)?;
        old.debt_out = old.debt_out.saturating_sub(amount);
    }
    {
        let cm = contract_mut(state, c.id)?;
        cm.status = ContractStatus::Transferred;
        cm.outstanding = 0;
        cm.held = Default::default();
        cm.insured = false;
    }
    // The successor inherits the ORIGINAL maturity. A transfer moves who owes,
    // not when it is due, and re-dating it here would let two colluding
    // members hand a debt back and forth to push its maturity out of reach for
    // ever — `MarkExpired` never fires, no default is recorded, no pool claim
    // is ever reachable. `Extend` is the transition that moves a maturity, and
    // it needs the creditor's signature because moving it is their concession.
    // And the successor inherits the ORIGINAL acceptance as its horizon base,
    // for the same reason: measured from the swap, two members could refresh
    // the horizon by passing the claim back and forth and extend it insured
    // for ever.
    let held = match held {
        Some(h) => Some(h),
        None if within_insured_horizon(state, c) => state.reserve_capacity_minor(new_debtor, amount),
        None => None,
    };
    open_obligation(state, new_debtor, c.creditor, amount, c.maturity_epoch, None, held, c.accepted_epoch)?;
    Ok(())
}

fn transfer(state: &mut State, contract_id: ContractId, new_debtor: MemberId, signers: &[Key]) -> Res<()> {
    let c = contract(state, contract_id)?;
    if c.status != ContractStatus::Active {
        return Err(Error(ET_CTR_BAD_STATE));
    }
    if new_debtor == c.debtor || new_debtor == c.creditor {
        return Err(Error(ET_CTR_SELF_DEAL));
    }
    require_signed(state, c.debtor, signers)?;
    require_signed(state, new_debtor, signers)?;
    require_active(state, new_debtor)?;
    // **The creditor signs only when the successor would NOT be insured.**
    //
    // A transfer discharges the old debtor, and every discharge must be
    // authorised by the party who loses if it is wrong. But the creditor only
    // loses something when the claim stops being one the community stands
    // behind. If the successor is insured, the community's recourse follows the
    // claim to its new debtor, and who that debtor is has become the
    // community's question rather than the creditor's — asking them to sign
    // would be asking consent for a change that, in the protocol's own terms,
    // is not one. §Model says the creditor "consented to that", and this is what
    // their consent is actually about.
    //
    // So the common case — the correspondent seam, where P is a member with
    // real standing here — needs two signatures, not three. The case that costs
    // the creditor their recourse needs all three, and cannot be done behind
    // their back. Which case it is, is a fact about the graph rather than a
    // choice anybody makes.
    if !within_insured_horizon(state, &c)
        || !state.fits_capacity_minor_after_release(new_debtor, c.outstanding, &c.held)
    {
        require_signed(state, c.creditor, signers)?;
    }
    move_debtor(state, &c, new_debtor, None)
}

/// Discharge an obligation, in whole or in part.
///
/// Two signatures, and the creditor's is the load-bearing one: a discharge
/// must be authorised by the party who loses if it is wrong. Measured, when it
/// was not: a discharge authorised by anything else hands one account 100% of
/// the community's ceiling.
///
/// The reservation is given back by `rehold` — exactly what was taken, never a
/// proportion of it.
///
/// **The cache follows the BOOK, not the payment**, and the two are not the
/// same number. A row closes when at most `dust` is left, so paying 9.99
/// against 10.00 moves the book by the whole 10.00 while only 9.99 was paid;
/// moving `debt_out` by the amount instead leaves the forgiven cent in the
/// cache, and §Verification's conservation check compares cache against book
/// for EXACT equality on the COMMIT path — so it does not corrupt the ledger
/// quietly, it returns `InvariantViolated` on every honest node at once and
/// halts the chain. Two honest signatures and ordinary traffic: enumerated
/// over every two-decimal amount from 0.02 to 1000.00, **81.6% of "pay all but
/// one cent" settlements** land at or under dust.
///
/// The same rule binds `cure`, `net_mutual` and `clear_member_debts`, which
/// close rows the same way. What the DEBTOR is credited with — the §Standing stake,
/// and the velocity counter — stays the amount they actually paid; only the
/// cache of what they owe follows the book, because that is the only one an
/// invariant compares against it.
fn settle(state: &mut State, contract_id: ContractId, amount: f64, signers: &[Key]) -> Res<()> {
    let c = contract(state, contract_id)?;
    if c.status != ContractStatus::Active {
        return Err(Error(ET_CTR_BAD_STATE));
    }
    require_signed(state, c.debtor, signers)?;
    signed_or_guardians(state, c.creditor, signers)?;
    if !amount.is_finite() || amount <= 0.0 {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    // THE BOUNDARY. The over-payment test is exact on both sides once the
    // amount is on the ledger's own grid — no epsilon, because there is no
    // representation gap left for one to cover.
    let amount = State::to_minor(amount);
    if amount == 0 || amount > c.outstanding || below_installment_floor(&c, amount) {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    let dust = state.params.dust_minor();
    let cleared = {
        let cm = contract_mut(state, contract_id)?;
        cm.outstanding = cm.outstanding.saturating_sub(amount);
        if cm.outstanding <= dust {
            cm.status = ContractStatus::Settled;
            cm.outstanding = 0;
        }
        c.outstanding - cm.outstanding
    };
    // The reservation follows the debt down BEFORE the stake is written —
    // `discharge_credit` says why the order is load-bearing. The stake reads
    // what this obligation has repaid in total, not this installment.
    rehold(state, contract_id)?;
    discharge_credit(state, c.debtor, c.creditor, c.original.saturating_sub(c.outstanding - cleared))?;
    let d = member_mut(state, c.debtor)?;
    d.debt_out = d.debt_out.saturating_sub(cleared);
    d.rep.d_out += State::from_minor(amount);
    Ok(())
}

/// Is this partial payment below the installment floor? A payment is at least
/// a `MAX_INSTALLMENTS`th of the original unless it closes the row.
///
/// A settle is free and was bounded by the obligation's AMOUNT — and an
/// uninsured amount is bounded by nothing but the ingress ceiling, so one
/// allowance slot bought `2^51` free durable writes of one minor unit each
/// (measured: 3,000 in a row, allowance and bonds unmoved), and on an insured
/// row each one cost every validator a re-hold. Priced by class instead, a
/// member at their ceiling curing in parts would be refused, which is the
/// absorbing default the schedule exists to prevent. A floor on the SHARE
/// bounds the count without touching the recovery path.
fn below_installment_floor(c: &Contract, amount: u64) -> bool {
    amount < c.outstanding && (amount as u128) * (k::MAX_INSTALLMENTS as u128) < c.original as u128
}

fn extend(state: &mut State, contract_id: ContractId, new_maturity_epoch: u64, signers: &[Key]) -> Res<()> {
    let c = contract(state, contract_id)?;
    if c.status != ContractStatus::Active {
        return Err(Error(ET_CTR_BAD_STATE));
    }
    require_signed(state, c.debtor, signers)?;
    require_signed(state, c.creditor, signers)?;
    if new_maturity_epoch <= c.maturity_epoch {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    if new_maturity_epoch > horizon(state.epoch, k::MAX_HORIZON_EPOCHS) {
        return Err(Error(ET_CTR_MATURITY_TOO_LONG));
    }
    // **Past the insured horizon the extension drops the insurance.** The
    // horizon is measured from the ACCEPTANCE, never from now — measured from
    // now, a chain of extensions each inside it rolls an insured claim for
    // ever, and the underwriters whose supply it holds were never asked. The
    // creditor's signature is on this write, so the drop is the consent a
    // transfer to an uninsured successor already takes; the flow returns to
    // the graph, and the claim is the creditor's own from here.
    let drops_insurance =
        c.insured && new_maturity_epoch > horizon(c.accepted_epoch, state.params.insured_horizon_epochs());
    if drops_insurance {
        state.release_capacity(&c.held);
    }
    let cm = contract_mut(state, contract_id)?;
    cm.maturity_epoch = new_maturity_epoch;
    if drops_insurance {
        cm.held = Default::default();
        cm.insured = false;
    }
    Ok(())
}

/// The permissionless default crank, and the point where an underwriter's
/// loss stops being a bound and becomes a debt.
///
/// **A default does not release the flow it committed**, and that one line is
/// the whole sanction. The defaulter's standing stays consumed by the debt
/// they did not pay, so their capacity does not come back — which is why
/// stealing through a default costs the thief exactly what they hold, at
/// 1.00×, and cannot be repeated. 1000 attempts by a crooked correspondent
/// extract 2500, once.
///
/// It is also why a forged discharge would have been so much worse than a
/// default: a defaulter spends their standing once, while a forger spends
/// nothing and stays clean, so lifetime extraction would be bounded by elapsed
/// time rather than by capacity.
///
/// If the obligation was INSURED, this is also where the community pays. The
/// creditor's claim moves onto the underwriters whose arcs carried it, split
/// by exactly what each carried, and the defaulter's debt subrogates to them
/// — `crate::loss` is the whole of it, the paper's §Recourse the reasoning. An
/// uninsured obligation substitutes nothing: nobody stood behind it, and the
/// creditor bears it alone (§Recourse).
fn mark_expired(state: &mut State, contract_id: ContractId) -> Res<()> {
    let c = contract(state, contract_id)?;
    if c.status != ContractStatus::Active {
        return Err(Error(ET_CTR_BAD_STATE));
    }
    if state.epoch <= c.maturity_epoch || c.outstanding <= state.params.dust_minor() {
        return Err(Error(ET_CTR_NOT_DUE));
    }
    contract_mut(state, contract_id)?.status = ContractStatus::Expired;
    member_mut(state, c.debtor)?.rep.open_default += c.outstanding;
    crate::loss::substitute(state, contract_id)
}

/// Late discharge against a defaulted obligation.
///
/// The creditor signs, deliberately: a debtor must not be able to declare its
/// own payment. That is the same rule `settle` runs on, and it is the reason a
/// foreign root can never be a discharge — it would be the first authority in
/// the alphabet that is not the party at risk.
fn cure(state: &mut State, contract_id: ContractId, amount: f64, signers: &[Key]) -> Res<()> {
    let c = contract(state, contract_id)?;
    if c.status != ContractStatus::Expired {
        return Err(Error(ET_CTR_BAD_STATE));
    }
    require_signed(state, c.debtor, signers)?;
    signed_or_guardians(state, c.creditor, signers)?;
    if !amount.is_finite() || amount <= 0.0 {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    // THE BOUNDARY, as in `settle`, and the same installment floor.
    let amount = State::to_minor(amount);
    if amount == 0 || amount > c.outstanding || below_installment_floor(&c, amount) {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    let dust = state.params.dust_minor();
    // The book move, not the payment — `settle` says why, and a cure closes a
    // row at dust exactly as a settlement does.
    let cleared = {
        let cm = contract_mut(state, contract_id)?;
        cm.outstanding = cm.outstanding.saturating_sub(amount);
        if cm.outstanding <= dust {
            cm.status = ContractStatus::Cured;
            cm.outstanding = 0;
        }
        c.outstanding - cm.outstanding
    };
    // Curing releases in proportion to what was actually paid, and only then.
    // The flow a default committed stays committed until the debt behind it
    // does not. Released before the stake is written, as in `settle`, and the
    // stake reads the obligation's cumulative repayment, as there.
    rehold(state, contract_id)?;
    discharge_credit(state, c.debtor, c.creditor, c.original.saturating_sub(c.outstanding - cleared))?;
    let d = member_mut(state, c.debtor)?;
    // The default is over when the row is: a claim the book no longer carries
    // is not an open default, whether the last cent was paid or forgiven.
    d.rep.open_default = d.rep.open_default.saturating_sub(cleared);
    d.debt_out = d.debt_out.saturating_sub(cleared);
    d.rep.d_out += State::from_minor(amount);
    Ok(())
}

// -------------------------------------------------- cascade + arbitration --

fn list_beneficiaries(
    state: &mut State,
    supporter: MemberId,
    entries: Vec<(MemberId, f64)>,
    signers: &[Key],
) -> Res<()> {
    require_active(state, supporter)?;
    require_signed(state, supporter, signers)?;
    if entries.len() > 64 {
        return Err(Error(ET_CAS_TOO_MANY));
    }
    // Self-listing is legal: the supporter's own entry is their share of
    // each sale that clears their own debts (the v0.5 breakdown shape).
    for &(b, w) in &entries {
        if !w.is_finite() || w <= 0.0 {
            return Err(Error(ET_CAS_BAD_WEIGHT));
        }
        member(state, b)?;
    }
    // Remove stale reverse edges, then install the new listing. The self
    // entry never enters the reverse index: it is a share, not a support
    // relationship, and needs no moderation approval.
    let old: Vec<MemberId> = member(state, supporter)?.beneficiaries.keys().copied().collect();
    for b in old {
        if b == supporter {
            continue;
        }
        if let Ok(bm) = member_mut(state, b) {
            bm.supporters_of.remove(&supporter);
        }
    }
    let mut list = std::collections::BTreeMap::new();
    for &(b, w) in &entries {
        *list.entry(b).or_insert(0.0) += w;
        if b != supporter {
            member_mut(state, b)?.supporters_of.insert(supporter);
        }
    }
    member_mut(state, supporter)?.beneficiaries = list;
    Ok(())
}

fn approve_supporter(
    state: &mut State,
    beneficiary: MemberId,
    supporter: MemberId,
    approved: bool,
    signers: &[Key],
) -> Res<()> {
    require_signed(state, beneficiary, signers)?;
    if approved {
        if !member(state, beneficiary)?.supporters_of.contains(&supporter) {
            return Err(Error(ET_CAS_NOT_LISTED));
        }
        member_mut(state, beneficiary)?.approved_supporters.insert(supporter);
    } else {
        member_mut(state, beneficiary)?.approved_supporters.remove(&supporter);
    }
    Ok(())
}

fn arb_attest(state: &mut State, contract_id: ContractId, arbiter: MemberId, amount: f64, signers: &[Key]) -> Res<()> {
    require_signed(state, arbiter, signers)?;
    let c = contract(state, contract_id)?;
    let terms = c.arb.clone().ok_or(Error(ET_ARB_NO_TERMS))?;
    if !terms.arbiters.contains(&arbiter) {
        return Err(Error(ET_ARB_NOT_PANEL));
    }
    // The window first, because it is the accurate reason: the sweep resolves
    // every panel whose window has closed, so an arbiter who is simply late
    // would otherwise be told an award exists when the quorum was never met.
    if state.epoch > horizon(c.created_epoch, terms.window_epochs) {
        return Err(Error(ET_ARB_WINDOW_CLOSED));
    }
    if c.arb_awarded {
        return Err(Error(ET_ARB_ALREADY_AWARDED));
    }
    if c.arb_attestations.contains_key(&arbiter) {
        return Err(Error(ET_ARB_ALREADY_ATTESTED));
    }
    // **The cheapest door there is.** An attestation is a free class, so this
    // bound is the only thing standing between one arbiter's signature and a
    // figure the sweep mints against on every node.
    if !State::amount_representable(amount) {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    // THE BOUNDARY: an attestation is stored on the ledger's grid, so the
    // median taken over the panel is integer arithmetic on integer inputs.
    contract_mut(state, contract_id)?
        .arb_attestations
        .insert(arbiter, State::to_minor(amount));
    Ok(())
}

/// **The award is the median of every attestation the window received**, minted
/// by the epoch sweep once the window has closed.
///
/// Minting at QUORUM instead reads correctly and decides the case by whoever
/// signs first: on a panel of sixteen with a quorum of nine, five arbiters
/// attesting the maximum before four honest ones attest zero take the median of
/// those nine — the maximum — and the remaining seven are refused as too late.
/// Five of sixteen decide, and the panel both parties agreed to is a formality
/// after the ninth signature. A median is a statement about a SET, and taking
/// it over whichever prefix arrived first measures the race rather than the
/// panel.
///
/// So attestations accumulate for the whole window, the quorum becomes the
/// floor for minting at all rather than the trigger, and nobody's signature is
/// worth more for being early. The cost is that a remedy is not available until
/// the window both parties consented to has run — which is what a window is.
///
/// Permissionless in the same sense `MarkExpired` is: the sweep runs it at
/// every epoch boundary, so nobody has to be watching for it.
fn arb_award(state: &mut State, contract_id: ContractId) -> Res<()> {
    let c = contract(state, contract_id)?;
    let terms = c.arb.clone().ok_or(Error(ET_ARB_NO_TERMS))?;
    if c.arb_awarded {
        return Err(Error(ET_ARB_ALREADY_AWARDED));
    }
    if state.epoch <= horizon(c.created_epoch, terms.window_epochs) {
        return Err(Error(ET_ARB_WINDOW_OPEN));
    }
    if (c.arb_attestations.len() as u32) < terms.quorum {
        // The panel did not reach the quorum both parties agreed to, so there
        // is nothing to mint and nothing to say. Marked awarded regardless, so
        // the sweep stops re-asking and a late attestation cannot revive it.
        contract_mut(state, contract_id)?.arb_awarded = true;
        return Ok(());
    }
    let mut vals: Vec<u64> = c.arb_attestations.values().copied().collect();
    vals.sort_unstable();
    let n = vals.len();
    // An even panel's median is the mean of the middle pair, ROUNDED DOWN.
    // Every node computes the same integer, which is the whole reason the
    // attestations are stored on the grid: a half-unit split by an `f64`
    // average would be a value the book cannot hold, and the two ways of
    // holding it — the award row and the loser's `debt_out` — could round it
    // differently. Down rather than up because an award is minted against a
    // party who did not consent to this figure, only to the panel.
    // Summed in `u128`: the pair being averaged are two amounts, and the sum
    // of two amounts is not one. Both are bounded by `MAX_AMOUNT_MINOR`, so
    // the `u64` this returns to is exact — the wider type is what makes the
    // intermediate exact as well, in the one arithmetic on this path that no
    // party bonded for.
    let median = if n % 2 == 1 { vals[n / 2] } else { ((vals[n / 2 - 1] as u128 + vals[n / 2] as u128) / 2) as u64 };
    // Bounded by the amount the panel was consented for — read off the terms,
    // because the row's own `original` is the first underwriter's share once
    // the claim has been subrogated.
    let award = median.min(terms.award_cap).min(terms.amount);
    contract_mut(state, contract_id)?.arb_awarded = true;
    if award <= state.params.dust_minor() {
        return Ok(());
    }
    // **What an award is, stated where it is minted.** It is not a remedy "on
    // a disputed default", not "from debtor to creditor", and not "subject to
    // the debtor's capacity gate": an award runs from the original CREDITOR to
    // the original DEBTOR, which makes it **the buyer's remedy for
    // non-delivery** — the seller took on an obligation to deliver,
    // the panel both parties named at acceptance found they did not, and the
    // buyer gets a claim back. That is why there is deliberately no status check:
    // a buyer who has PAID IN FULL and received nothing is exactly the case that
    // needs it, and gating on the row still being open would close the remedy at
    // the moment it becomes due.
    //
    // It is bounded three ways, all consented at acceptance: the median of the
    // panel's attestations, the `award_cap` both parties agreed, and the original
    // amount itself — measured, a panel attesting 9999 each against a cap of 5000
    // on an original of 100 awards 100. The window runs from creation.
    //
    // Minted regardless of headroom, and UNINSURED, so what it is worth is
    // exactly what §Recourse says any uninsured claim is worth. It does NOT
    // "consume the loser's capacity": an uninsured obligation reserves nothing,
    // so it does not touch the capacity path at all — measured, the loser's
    // capacity is unchanged at 2500 across an award of 90. What it does touch
    // is the loser's
    // WRITE headroom, which `bond_headroom` nets `debt_out` out of: 2500 → 2410
    // on the same scene. Beyond that it is an ordinary claim that nets against
    // any future trade the loser does with the winner, and nothing more. That is
    // a real remedy in a mutual-credit community and it is not a lien.
    let maturity_epoch = horizon(state.epoch, state.params.min_maturity_epochs);
    // Between the parties the PANEL binds, which are the parties at
    // acceptance: the row's own may have moved to an underwriter at
    // substitution, and the remedy for non-delivery was never theirs to owe.
    // Before the award row, for the same reason `open_obligation` does it: a
    // refusal past the insert would leave a claim on the book with no cached
    // debt behind it.
    let debt_out = debt_after(state, terms.creditor, award)?;
    let cid = state.next_contract;
    state.next_contract += 1;
    state.contracts.insert(
        cid,
        Contract {
            id: cid,
            debtor: terms.creditor,
            creditor: terms.debtor,
            outstanding: award,
            original: award,
            maturity_epoch,
            status: ContractStatus::Active,
            created_epoch: state.epoch,
            accepted_epoch: state.epoch,
            insured: false,
            held: Default::default(),
            arb: None,
            arb_attestations: Default::default(),
            arb_awarded: false,
        },
    );
    let loser = member_mut(state, terms.creditor)?;
    loser.debt_out = debt_out;
    loser.rep.d_in += State::from_minor(award);
    Ok(())
}

// -------------------------------------------------------------- lifecycle --

fn exit(state: &mut State, member_id: MemberId, signers: &[Key]) -> Res<()> {
    require_signed(state, member_id, signers)?;
    // **`Exit` is priced at zero because it closes what it touches, and the
    // second one closes nothing.** Every zero on the bond schedule has to
    // survive "how many of these can one member force in an epoch?", and
    // without this line the answer here was *unlimited*: measured, 500 of 500
    // repeated `Exit`s accepted on an already-`Exited` row with the free
    // allowance at zero and `bond_headroom` at 0.00, each recording a replay id
    // in `applied_by_expiry` — which `root.rs` hashes into the state root, so this
    // was unpriced growth of the replicated state by a member with no write
    // budget at all. `bond::admits` passed them too, so the mempool screen was
    // not the bound either.
    //
    // It is `DeclareSupply`'s defect one door over, and `Exit` was the only
    // free class carrying it: every other one refuses its own second call
    // (`RotateVeto` ET-ROT-003, `RotateFinalize` ET-ROT-001, `MarkExpired`
    // ET-CTR-002, `ForfeitBonds` ET-BND-002). Making the transition destroy its
    // own precondition is what makes the schedule's stated justification true,
    // rather than capping a count the justification never claimed to bound.
    //
    // `Suspended` is deliberately NOT refused here. Winding down under sanction
    // is the recovery path, and it is free for the reason the module note in
    // `bond` gives: a status a member cannot leave turns a recoverable failure
    // into an absorbing one. What a suspended member may do on the way out is
    // `settle`, `cure`, `transfer` and this.
    if member(state, member_id)?.status == MemberStatus::Exited {
        return Err(Error(ET_MEM_NOT_ACTIVE));
    }
    let m = member(state, member_id)?;
    if m.debt_out > state.params.dust_minor() || m.rep.open_default > state.params.dust_minor() {
        return Err(Error(ET_LIF_OUTSTANDING_DEBT));
    }
    if m.bond_enc() > state.params.dust_minor() {
        return Err(Error(ET_LIF_OUTSTANDING_BONDS));
    }
    // Leaving the community and leaving the UNDERWRITER role are two acts, and
    // the second has a floor the first must not be able to jump. A member
    // walking out while credit still stands on their supply would break the
    // cut bound by exactly the §Stability route, through a transition that never
    // mentions supply at all.
    if state.underwriters.contains_key(&member_id) {
        return Err(Error(ET_UWR_STILL_DECLARED));
    }
    // `exit` is UNILATERAL — no governance, no other party's signature —
    // which is exactly why it needs its own floor check rather than relying
    // on whatever governance does. Checked here, before any mutation begins
    // (this function's own validate-then-mutate discipline), so a refused
    // exit leaves state untouched rather than half-unwound.
    if state.validators.contains_key(&member_id) && (state.validators.len() as u64) <= state.params.min_validators {
        return Err(Error(ET_VAL_LAST_VALIDATOR));
    }
    // Unwind cascade edges in both directions.
    let listed: Vec<MemberId> = member(state, member_id)?.beneficiaries.keys().copied().collect();
    for b in listed {
        if let Ok(bm) = member_mut(state, b) {
            bm.supporters_of.remove(&member_id);
            bm.approved_supporters.remove(&member_id);
        }
    }
    let listers: Vec<MemberId> = member(state, member_id)?.supporters_of.iter().copied().collect();
    for l in listers {
        if let Ok(lm) = member_mut(state, l) {
            lm.beneficiaries.remove(&member_id);
        }
    }
    state.validators.remove(&member_id);
    // The stake graph is deliberately NOT swept. An edge records what a
    // creditor placed behind somebody, and an exiting member's departure does
    // not un-place it; decay is what retires standing, on the one schedule
    // that applies to everybody. Sweeping here would also hand any member a
    // way to erase the evidence of who backed them, on demand.
    let m = member_mut(state, member_id)?;
    m.beneficiaries.clear();
    m.supporters_of.clear();
    m.approved_supporters.clear();
    m.status = MemberStatus::Exited;
    Ok(())
}

// ------------------------------------------------------------- governance --

/// The share of the community's **external seed** that has assented (§Governance):
///
/// ```text
///     Σ external supply of the assenters
///     ──────────────────────────────────
///          Σ external supply
/// ```
///
/// **The ceremony governs.** External supply is what arrived through genesis
/// or through a §Governance amendment the community endorsed — a commitment by
/// somebody with something to lose outside the ledger. It is the only quantity
/// in this model that a signature cannot manufacture, which is the whole of
/// §Security applied to votes instead of to credit. Free keys are mute, internal
/// declarations are mute, and the electorate grows by exactly the door that
/// already exists for growing the seed.
///
/// **This replaced a cut, and the cut was capturable and wrong in two
/// independent ways** (§Governance). It read
/// `capacity(assenters) + Σ supply the assenters declared` over the declared
/// total, and:
///
/// - Every declared supply entered the numerator TWICE at face value — as a
///   source arc feeding the tail's capacity, and as the tail's own supply — so
///   a chain of accomplices, each fake-backed by all the previous ones and each
///   declaring the maximum, doubled the tail's weight per link while the
///   coalition's real capacity never moved. Measured: against a 15,000 seed,
///   one honest 300 trade and seven accomplices put 9,600 + 9,600 over a
///   declared 34,200 — 0.56, and the tail alone took the validator set.
/// - A cut hands full weight to whoever the seed REACHES rather than to
///   whoever put it up. Measured: one founder declaring the whole 2500 and
///   backing one member for all of it has capacity 0 and the member they
///   backed enacts alone. That is not a capture, it is the measure meaning
///   the wrong thing, and it is why no capacity-based weight was salvageable
///   for any tier.
///
/// **One measure, every kind.** One proposal was to split — the
/// constitutional kinds on the seed, the "advisory" constants left on the cut
/// — and that split does not survive contact with `ParamKey`. `StakeDecay` at
/// either end of its safe range is a credit freeze, `BondFraction` at its
/// ceiling is censorship by arithmetic, and `SeedRate` governs the franchise
/// itself now that the seed is the electorate. What was left as genuinely
/// advisory was one key. A second weight for one key would install "one act,
/// two code paths, opposite rules" — this model's most expensive shape — by
/// design, and would keep alive a measure the scene above shows to be wrong.
///
/// **What members keep** is the proposal: `propose` still asks only for the
/// establishment floor, so any member with something to lose can put a change
/// on the record. Members propose; the ceremony enacts.
///
/// Both operands are integer minor units, so the ratio is exact on both sides
/// of the comparison — the old form divided an `f64` capacity by a converted
/// total, which is the `f64`/minor boundary the halts were at.
fn governance_mass(state: &State, assenting: &[MemberId]) -> f64 {
    let external = state.external_minor();
    if external == 0 {
        // A community with no external seed has no denominator, and no
        // proposal of any kind can be enacted — including the amendment that
        // would give it one. Zero is absorbing here exactly as it is for
        // credit (§Adoption), and for the same reason: a repeatable ceremony still
        // needs a seed to be a fraction OF.
        return 0.0;
    }
    // Deduplicated because `assent` appends the caller to the recorded set
    // without first checking whether they are already in it, so a re-assent
    // would otherwise vote twice.
    let voters: std::collections::BTreeSet<MemberId> = assenting.iter().copied().collect();
    let mine: u64 = voters.iter().filter_map(|id| state.underwriters.get(id)).sum();
    mine as f64 / external as f64
}

/// Who may propose or assent a kind: an active member — and, for `Unsuspend`
/// alone, a suspended one.
///
/// Suspension silences a voter without removing their weight from the seed,
/// so a coalition holding the ordinary bar could suspend everyone else,
/// withdraw, and leave an electorate in which nothing could ever be enacted
/// again — no reinstatement, no amendment, no parameter. The paper's
/// argument that a constant is recoverable through its door assumes the
/// electorate survives its own sanctions. Letting the sanctioned propose and
/// vote on reinstatement, and on nothing else, undoes the deadlock without
/// moving anybody's weight: a majority of the seed cannot be held suspended,
/// which is what a majority of the seed means, and a minority still cannot
/// reinstate itself.
fn may_govern(state: &State, id: MemberId, kind: &ProposalKind) -> Res<()> {
    match (member(state, id)?.status, kind) {
        (MemberStatus::Active, _) | (MemberStatus::Suspended, ProposalKind::Unsuspend { .. }) => Ok(()),
        _ => Err(Error(ET_MEM_NOT_ACTIVE)),
    }
}

/// Would the bond unit still be at least one minor unit under these values?
///
/// Zero is absorbing everywhere else in the ledger and here it would open: a
/// unit that rounds to nothing makes every bonded class free and every seat
/// free (`can_seat_minor` answers yes to any count when the unit is zero). It
/// takes about a hundredfold of cumulative downward re-denomination with
/// `BondFraction` at its floor, and the constitutional table cannot see it
/// coming because it is written in genesis units. So the two acts that move
/// the unit are refused at the value that would kill it.
fn unit_survives(bond_fraction: f64, v_base: f64) -> bool {
    State::to_minor(bond_fraction * v_base) >= 1
}

fn propose(state: &mut State, author: MemberId, kind: ProposalKind, signers: &[Key]) -> Res<()> {
    require_signed(state, author, signers)?;
    may_govern(state, author, &kind)?;
    // The establishment floor is now the same "has something to lose" test the
    // write surface uses: your own capacity, or your declared supply if you
    // underwrite. Free keys are excluded by arithmetic rather than by a quota
    // — the same mechanism that excludes them from credit — because a proposal
    // from an account nobody backs is a free signature, and free signatures
    // decide nothing here.
    //
    // A seed amendment is the one exemption, and it is exactly the case the
    // floor would otherwise close on: §Standing names members who bring backing from
    // OUTSIDE as *the* growth path, and such a member has nothing on-ledger to
    // be established by — a co-op founding on the ledger in year three has
    // precisely zero. Nothing is granted by proposing, so the exemption grants
    // nothing either: it takes Θ of the external seed to enact, and the write
    // itself is still bonded, which means a newcomer's amendment is co-signed
    // and paid for by an established member — the same cost §Recourse already names
    // for a newcomer's first trade, and the same shape.
    if !matches!(kind, ProposalKind::SeedAmendment { .. }) && state.conferrable(author) <= state.params.dust {
        return Err(Error(ET_GOV_NOT_ESTABLISHED));
    }
    match &kind {
        ProposalKind::ParamChange { key, value } => {
            let (lo, hi) = crate::params::Params::safe_range(*key);
            if !(*value >= lo && *value <= hi) {
                return Err(Error(ET_GOV_OUT_OF_RANGE));
            }
            if *key == ParamKey::BondFraction && !unit_survives(*value, state.params.v_base) {
                return Err(Error(ET_GOV_OUT_OF_RANGE));
            }
        }
        ProposalKind::Redenominate { num, den } => {
            if *num == 0 || *den == 0 {
                return Err(Error(ET_GOV_BAND));
            }
            let pi = *num as f64 / *den as f64;
            if libm::fabs(libm::log(pi)) > state.params.redenom_band_ln {
                return Err(Error(ET_GOV_BAND));
            }
            if !unit_survives(state.params.bond_fraction, state.params.v_base * pi) {
                return Err(Error(ET_GOV_BAND));
            }
        }
        ProposalKind::Suspend { member: m } | ProposalKind::Unsuspend { member: m } => {
            member(state, *m)?;
        }
        ProposalKind::ValidatorPower { member: m, power } => {
            // Consensus sums the set's powers into one `u64`
            // (`engine_context::total_voting_power`), so an unbounded power is
            // not just a governance concern: a large enough value plus any
            // second validator overflows that sum — a panic on every node in
            // debug, and in release a WRAP to a small total, against which a
            // single vote satisfies quorum. Bounded per validator, and the
            // whole set re-checked below, so no reachable set can overflow.
            if *power > k::MAX_VALIDATOR_POWER {
                return Err(Error(ET_VAL_POWER_TOO_HIGH));
            }
            if *power > 0 {
                match member(state, *m)?.status {
                    MemberStatus::Active => {}
                    _ => return Err(Error(ET_VAL_NOT_ELIGIBLE)),
                }
                // The operator has to have registered a consensus key before
                // the community can vote them power: a validator whose signing
                // key the ledger cannot name is one no certificate verifies
                // against. Re-checked at enactment for the same reason the
                // power ceiling is — the set has moved since.
                if member(state, *m)?.consensus_key.is_none() {
                    return Err(Error(ET_VAL_NO_CONSENSUS_KEY));
                }
            } else {
                member(state, *m)?;
            }
        }
        ProposalKind::SeedAmendment { amount } => {
            // The static half only. The RATE bound is deliberately not checked
            // here — assent can arrive many epochs later, by which time both
            // the tracked seed and the epoch's remaining headroom have moved,
            // so a check here would be a promise about a quantity this
            // transition does not decide. It is enforced where it binds, at
            // enactment, exactly as `ValidatorPower` re-checks the validator
            // set it will actually join.
            if *amount <= 0.0 || !State::amount_representable(*amount) || State::to_minor(*amount) == 0 {
                return Err(Error(ET_CTR_BAD_AMOUNT));
            }
        }
    }
    let id = state.next_proposal;
    state.next_proposal += 1;
    state.proposals.insert(
        id,
        Proposal { id, kind, author, assents: Default::default(), enacted: false, opened_epoch: state.epoch },
    );
    Ok(())
}

/// The share of the external seed this proposal needs, which is not one number.
///
/// **A change to who ORDERS the ledger takes two thirds; everything else takes
/// a half.** At one threshold for every kind, whoever holds half the seed can
/// remove every other validator down to the ledger's floor and suspend anyone
/// who objects, alone and in one epoch. A parameter moved too far is moved back
/// through the same door — every value inside a constitutional range is one the
/// ledger keeps working at — while a validator set is not recoverable that way,
/// because the coalition holding it decides which blocks exist, including the
/// ones that would undo it.
///
/// **Suspension is in the higher class exactly when its target holds voting
/// power**, and that has to be read off the state rather than off the kind:
/// `Suspend` removes a validator (`enact`'s own `MIN_VALIDATORS` check is
/// there because it does), so a rule keyed on the kind alone would leave the
/// same door open one name over. The reading is a pure function of committed
/// state, so every node agrees on which bar applies; a target who gains or
/// loses power while a proposal is open moves the bar with them, which is the
/// bar tracking what the act would actually do.
pub fn adoption_threshold(state: &State, kind: &ProposalKind) -> f64 {
    let touches_the_order = match kind {
        ProposalKind::ValidatorPower { .. } => true,
        ProposalKind::Suspend { member } | ProposalKind::Unsuspend { member } => state.validators.contains_key(member),
        _ => false,
    };
    if touches_the_order {
        state.params.theta_adopt_validator
    } else {
        state.params.theta_adopt
    }
}

fn assent(state: &mut State, member_id: MemberId, proposal_id: ProposalId, signers: &[Key]) -> Res<()> {
    require_signed(state, member_id, signers)?;
    let p = state.proposals.get(&proposal_id).ok_or(Error(ET_GOV_UNKNOWN_PROPOSAL))?;
    may_govern(state, member_id, &p.kind)?;
    if p.enacted {
        return Err(Error(ET_GOV_UNKNOWN_PROPOSAL));
    }
    // The second call refuses. An assent already recorded was admitted again
    // and again — free, and each admission a durable replay id — because the
    // set it inserts into absorbed the duplicate silently.
    if p.assents.contains(&member_id) {
        return Err(Error(ET_GOV_ALREADY_ASSENTED));
    }
    let kind = p.kind.clone();
    let author = p.author;
    // The franchise, and it is the whole of §Governance's answer to the capture:
    // assent weight is a share of the EXTERNAL seed (§Governance), so a member who holds
    // none of it carries none of the vote. Refused rather than recorded at
    // zero, for the same reason the author-conflict below is refused rather
    // than discounted — a vote the ledger shows and does not use is a
    // preference, and ledger state carries what must be ENFORCED.
    //
    // Nothing is lost by it: `propose` still asks only for the establishment
    // floor, so any member with something to lose can put a change on the
    // record. Members propose; the ceremony enacts.
    if state.underwriters.get(&member_id).copied().unwrap_or(0) == 0 {
        return Err(Error(ET_GOV_NO_MANDATE));
    }
    // A `SeedAmendment` names no beneficiary because its beneficiary is its
    // author (§Governance), so an author assenting is a member voting on their own
    // supply. Refused rather than silently discounted: a recorded assent that
    // did not count would be a vote the ledger shows and does not use.
    //
    // The exclusion is load-bearing under the governance weight this ledger
    // actually uses, and MORE so since that weight became the external seed
    // (§Governance): an amendment adds external supply to its author, and external
    // supply is now the whole of the vote — so an author assenting their own
    // amendment votes with precisely the quantity the amendment enlarges, and
    // each raise carries the next one more easily. (§Governance's own note that
    // "today's governance formula excludes underwriters entirely, which
    // happens to cover it" described a formula this ledger no longer has, and
    // the formula it now has covers it even less.)
    if matches!(kind, ProposalKind::SeedAmendment { .. }) && member_id == author {
        return Err(Error(ET_SEED_CONFLICTED));
    }
    // The cooldown is checked BEFORE the assent is recorded, because `apply`
    // has no rollback: a handler that mutates and then returns `Err` leaves
    // the mutation on the ledger while the transaction is reported failed.
    // Recording first meant a member told their assent had been refused
    // (`ET-GOV-004`) had it banked anyway, and a later assent could enact the
    // change on a support set including one nobody consented to leaving there.
    // Every rejection in this function now happens before the first write.
    // Would this assent carry the proposal over Θ_adopt? Computed WITHOUT
    // recording it, because the answer decides whether recording is allowed.
    let assenting: Vec<MemberId> = p.assents.iter().copied().chain(std::iter::once(member_id)).collect();
    let enacts = governance_mass(state, &assenting) >= adoption_threshold(state, &kind);
    let cooling = match &kind {
        ProposalKind::ParamChange { key, .. } => state
            .params
            .last_amend_epoch
            .get(key)
            .is_some_and(|&last| state.epoch < horizon(last, state.params.gov_cooldown_epochs)),
        ProposalKind::Redenominate { .. } => state
            .params
            .last_redenom_epoch
            .is_some_and(|last| state.epoch < horizon(last, state.params.gov_cooldown_epochs)),
        _ => false,
    };
    if enacts && cooling {
        return Err(Error(ET_GOV_COOLDOWN));
    }
    // The unit guard again, at enactment and before the first write: the
    // values it reads may have moved since the proposal was written.
    if enacts {
        match &kind {
            ProposalKind::ParamChange { key: ParamKey::BondFraction, value }
                if !unit_survives(*value, state.params.v_base) =>
            {
                return Err(Error(ET_GOV_OUT_OF_RANGE));
            }
            ProposalKind::Redenominate { num, den }
                if !unit_survives(state.params.bond_fraction, state.params.v_base * (*num as f64 / *den as f64)) =>
            {
                return Err(Error(ET_GOV_BAND));
            }
            _ => {}
        }
    }
    // The amendment's own admissibility, in the same place and for the same
    // reason as the cooldown: every rejection in this function happens before
    // the first write, because a handler that mutates and then fails leaves
    // the mutation on the ledger while the transaction is reported failed.
    if enacts {
        if let ProposalKind::SeedAmendment { amount } = kind {
            crate::seed::check(state, author, amount)?;
        }
    }
    state
        .proposals
        .get_mut(&proposal_id)
        .ok_or(Error(ET_GOV_UNKNOWN_PROPOSAL))?
        .assents
        .insert(member_id);
    if !enacts {
        return Ok(());
    }
    match kind {
        ProposalKind::ParamChange { key, value } => {
            state.params.set(key, value);
            let epoch = state.epoch;
            state.params.last_amend_epoch.insert(key, epoch);
        }
        ProposalKind::Redenominate { num, den } => {
            let pi = num as f64 / den as f64;
            state.rescale(pi);
            state.params.last_redenom_epoch = Some(state.epoch);
        }
        ProposalKind::Suspend { member: target } => {
            // Same floor as `exit` and `ValidatorPower { power: 0 }`
            // below — checked before `mm.status` is touched, so a refused
            // suspension leaves the member's status untouched too, not just
            // the validator set.
            if state.validators.contains_key(&target) && (state.validators.len() as u64) <= state.params.min_validators
            {
                return Err(Error(ET_VAL_LAST_VALIDATOR));
            }
            let mm = member_mut(state, target)?;
            if matches!(mm.status, MemberStatus::Active) {
                mm.status = MemberStatus::Suspended;
            }
            state.validators.remove(&target);
        }
        ProposalKind::Unsuspend { member: target } => {
            let mm = member_mut(state, target)?;
            if mm.status == MemberStatus::Suspended {
                // `Active` is the resting state of every account that has not
                // been suspended or left. There is no ladder to return to.
                mm.status = MemberStatus::Active;
            }
        }
        ProposalKind::ValidatorPower { member: target, power } => {
            if power > 0 {
                match member(state, target)?.status {
                    MemberStatus::Active => {
                        // Re-checked at enactment, not only at proposal: the
                        // set has moved since, and it is the SET's total that
                        // consensus sums. Refused before any mutation.
                        let others: u64 =
                            state.validators.iter().filter(|(&id, _)| id != target).map(|(_, &p)| p).sum();
                        if power > k::MAX_VALIDATOR_POWER || others.checked_add(power).is_none() {
                            return Err(Error(ET_VAL_POWER_TOO_HIGH));
                        }
                        if member(state, target)?.consensus_key.is_none() {
                            return Err(Error(ET_VAL_NO_CONSENSUS_KEY));
                        }
                        state.validators.insert(target, power);
                    }
                    _ => return Err(Error(ET_VAL_NOT_ELIGIBLE)),
                }
            } else {
                // `power: 0` is this proposal kind's removal path — the
                // same floor `exit` and `Suspend` enforce.
                if state.validators.contains_key(&target)
                    && (state.validators.len() as u64) <= state.params.min_validators
                {
                    return Err(Error(ET_VAL_LAST_VALIDATOR));
                }
                state.validators.remove(&target);
            }
        }
        ProposalKind::SeedAmendment { amount } => crate::seed::enact(state, author, amount)?,
    }
    if let Some(p) = state.proposals.get_mut(&proposal_id) {
        p.enacted = true;
    }
    Ok(())
}

// --------------------------------------------- the deterministic signer rule --

/// **Do these signatures authorise anything at all?** A pure function of
/// committed state, computed without a single flow query, so it is safe to make
/// a BLOCK VALIDITY rule out of.
///
/// An envelope that authorises nothing is refused at dispatch with
/// `ET-MEM-NOT_SIGNER`, and `apply` then REFUNDS its bond and forgets its id —
/// deliberately, because burning the id would let anyone who saw a request in
/// the pending pool strip a signature off it and permanently kill the genuine
/// transaction for free. What that leaves is a transaction that costs its
/// submitter nothing and can be re-applied without limit: measured, 1,000
/// applications of one unauthorised envelope under one id spend zero allowance,
/// encumber zero bonds and record zero ids. And every one of them still costs
/// the node that applies it a `seed_reach` max-flow per signer, because the
/// bond gate runs before dispatch.
///
/// So such an envelope must never reach a block. This is the test, applied in
/// three places for three different reasons:
///
///  - at the ingress (`serve::core::Node::submit`), so it never enters a
///    mempool, which is what keeps an HONEST proposer from building a block
///    every other validator would then have to refuse;
///  - in the pre-vote screen, so a Byzantine proposer's block is voted Invalid
///    rather than committed;
///  - at commit, because a certificate can arrive by sync rather than by vote.
///
/// **It is deliberately conservative.** A `false` here must mean the transition
/// is CERTAIN to fail at dispatch, because refusing a block is severe: anything
/// this cannot decide cheaply and exactly is left to `apply`. So the creditor's
/// conditional signature on a `Transfer` is not checked (deciding it needs a
/// max-flow), an unknown member or contract asks for nothing (those fail
/// cheaply, and demanding a signature for a row that does not exist would
/// refuse blocks over a typo), and the permissionless cranks require nobody.
///
/// Determinism, which is what makes it usable as a validity rule at all: every
/// voter evaluates it against the same parent state, because the screen already
/// refuses a block whose `app_hash` does not match the state it holds.
pub fn authorises(state: &State, tx: &Tx, signers: &[Key]) -> bool {
    let signed = |id: MemberId| -> bool {
        match state.members.get(&id) {
            // An unknown member asks for nothing: `apply` refuses it with
            // `ET-MEM-001` before it reads anything expensive.
            None => true,
            Some(m) => signers.iter().any(|s| m.has_key(s)),
        }
    };
    let party_signed = |p: &Party| -> bool {
        match p {
            Party::Member(id) => signed(*id),
            Party::Key(k) => match state.member_of_key(k) {
                Some(id) => signed(id),
                // A party named by a key nobody holds is the newcomer whose
                // first trade seats them, and `resolve` already demands that
                // key's own signature.
                None => signers.contains(k),
            },
        }
    };
    let parties_of = |contract: ContractId| -> Option<(MemberId, MemberId)> {
        state.contracts.get(&contract).map(|c| (c.debtor, c.creditor))
    };
    match tx {
        // The permissionless cranks: nobody is named, and `bond::due` answers
        // `Free` from the schedule before it asks who would pay.
        Tx::MarkExpired { .. } | Tx::ForfeitBonds { .. } | Tx::RotateFinalize { .. } => true,
        // The member's own acts.
        Tx::RegisterGuardians { member, .. }
        | Tx::RotateVeto { member }
        | Tx::SetConsensusKey { member, .. }
        | Tx::Exit { member }
        | Tx::DeclareSupply { member, .. }
        | Tx::Assent { member, .. } => signed(*member),
        Tx::ListBeneficiaries { supporter, .. } => signed(*supporter),
        Tx::ApproveSupporter { beneficiary, .. } => signed(*beneficiary),
        Tx::ArbAttest { arbiter, .. } => signed(*arbiter),
        Tx::Propose { author, .. } => signed(*author),
        // Key recovery: a THRESHOLD of the member's own guardians, which is
        // countable here exactly as `rotate_request` counts it. A member with
        // no guardian config asks for nothing — that call fails cheaply on the
        // missing config.
        Tx::RotateRequest { member, .. } => match state.members.get(member).and_then(|m| m.guardian.as_ref()) {
            None => true,
            Some(cfg) => cfg.guardians.iter().filter(|&&g| signed(g)).count() as u32 >= cfg.threshold,
        },
        // Both sides of a trade, either of which may be named by key.
        Tx::Accept { debtor, creditor, .. } => party_signed(debtor) && party_signed(creditor),
        Tx::Sale { seller, buyer, .. } => party_signed(seller) && party_signed(buyer),
        // Both parties to an existing obligation. The creditor's signature on a
        // `Transfer` is conditional on the successor's insurance, which is a
        // max-flow question, so only the two unconditional signatures are here.
        Tx::Extend { contract, .. } => match parties_of(*contract) {
            None => true,
            Some((debtor, creditor)) => signed(debtor) && signed(creditor),
        },
        // A discharge: the debtor, and the creditor or a threshold of the
        // creditor's guardians — the same count `rotate_request` takes, mirrored
        // here exactly as `signed_or_guardians` applies it.
        Tx::Settle { contract, .. } | Tx::Cure { contract, .. } => match parties_of(*contract) {
            None => true,
            Some((debtor, creditor)) => {
                let guardians = state
                    .members
                    .get(&creditor)
                    .and_then(|m| m.guardian.as_ref())
                    .is_some_and(|cfg| {
                        cfg.guardians
                            .iter()
                            .filter(|&&g| state.members.contains_key(&g) && signed(g))
                            .count() as u32
                            >= cfg.threshold
                    });
                signed(debtor) && (signed(creditor) || guardians)
            }
        },
        Tx::Transfer { contract, new_debtor } => match parties_of(*contract) {
            None => signed(*new_debtor),
            Some((debtor, _)) => signed(debtor) && signed(*new_debtor),
        },
    }
}

#[cfg(test)]
mod min_validators_tests {
    use super::*;

    /// `exit` is unilateral (no governance, no other signer) — the
    /// sharpest of the three removal paths, since a single member can drive
    /// it alone. Refused rather than silently applied when the exiting
    /// member is the last validator standing.
    #[test]
    fn exit_refuses_to_remove_the_last_validator() {
        let mut st = State::default();
        let key = [7u8; 32];
        // Ordering the chain and funding it are two roles, and this test needs
        // the validator one: an underwriter is refused an exit earlier, by the
        // supply floor, before the validator floor is ever reached.
        st.add_underwriter(vec![[9u8; 32]], 2500.0).expect("founding underwriter");
        let id = st.new_account(vec![key]);
        st.set_consensus_key(id, {
            let mut k = [0xC0u8; 32];
            k[0] = id as u8;
            k
        })
        .expect("consensus key");
        st.set_genesis_validator(id, 1).expect("genesis validator");

        let err = exit(&mut st, id, &[key]).expect_err("PROVEN: draining the last validator must be refused");
        assert_eq!(err, Error(ET_VAL_LAST_VALIDATOR));
        assert!(st.validators.contains_key(&id), "the set must stay untouched, not emptied then rejected");
    }

    /// The other floor, and the one that is new: leaving the community and
    /// leaving the UNDERWRITER role are two acts, and the second has a floor
    /// the first must not be able to jump. A member walking out while credit
    /// stands on their supply would break the cut bound by exactly the §Stability
    /// route, through a transition that never mentions supply at all.
    #[test]
    fn exit_refuses_a_member_still_carrying_a_declared_supply() {
        let mut st = State::default();
        let key = [7u8; 32];
        let id = st.add_underwriter(vec![key], 2500.0).expect("founding underwriter");
        let err = exit(&mut st, id, &[key]).expect_err("PROVEN: an underwriter may not simply leave");
        assert_eq!(err, Error(ET_UWR_STILL_DECLARED));
        assert_eq!(st.underwriters.get(&id), Some(&250_000), "the supply must stay, not be dropped then refused");

        // Withdrawing first is the way out, and it is legal because nothing
        // is drawn through them.
        assert_eq!(st.supply_floor(id), 0);
        declare_supply(&mut st, id, 0.0, &[key]).expect("nothing is committed, so the floor is zero");
        assert!(!st.underwriters.contains_key(&id));
    }
}
