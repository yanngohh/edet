//! Growing the seed (the paper's §Governance).
//!
//! **This is the only door a supply can grow through, and genesis is the only
//! other one.** `DeclareSupply` may lower a declaration and nothing else.
//!
//! `DeclareSupply` admitting a raise capped by the declarer's own CAPACITY
//! would keep a free key from declaring a billion — true one party at a time,
//! and hollow in aggregate, because capacity is what the community conferred.
//! What it buys a coalition: twelve joiners wash-backing each other behind a
//! seed of 100 declare 204,900 between them, then borrow 204,800 from honest
//! creditors with the ledger's own `insured` label on every contract, every
//! invariant satisfied. So there is no internal door, and this module carries
//! the whole of the community's ability to take on more underwriting.
//!
//! Which makes what follows load-bearing rather than a convenience. Without
//! it the ledger's external-commitment door would close at genesis,
//! permanently, for everyone — a founder whose capacity is near zero could not
//! RAISE their supply, a newcomer bringing backing from outside would have no
//! transition to arrive through, and a co-op that forms in year three could
//! never have underwriters at all, because zero is absorbing per region. Note
//! the asymmetry that would leave: genesis fixes two sets, and the validator
//! half is already amendable at runtime.
//!
//! **Verification is impossible, so this does not look for it.** Externality
//! is precisely the property no set of signatures can prove (§Security). Which
//! forces the honest reading of genesis: *it was never a verification, it was
//! a ceremony the ledger recorded.* Humans checked each other out of band and
//! the file was the record. The only coherent repair is to make the ceremony
//! repeatable — a transition, not a proof.
//!
//! So an amendment is `Propose { kind: SeedAmendment { amount } }` carried
//! through the existing governance machinery, and three properties make it
//! safe to have at all:
//!
//! - **The author IS the beneficiary**, structurally: the kind names nobody.
//!   A supply is a signed, standing consent to inherit debts (§Recourse), so an
//!   amendment that named somebody else would volunteer a member to
//!   underwrite — and every discharge in this alphabet is authorised by the
//!   party who loses if it is wrong. Here that party signed the proposal.
//! - **Assent is weighted by the external seed** (§Governance), so free keys are mute
//!   by the same arithmetic as everywhere else, and the beneficiary may not
//!   assent their own amendment (`ET-SED-002`) — an exclusion that matters
//!   more under that weight, not less, since an amendment adds external supply
//!   to its author and external supply is the whole of the vote.
//! - **The rate is bounded** at β × the seed already tracked, per epoch. The
//!   base was written as the EXTERNAL seed rather than the declared total, and
//!   that was the whole security content while the two could differ: a
//!   declared total inflated geometrically from inside the community — seed
//!   100, twelve joiners, 204,900 declared — and a rate computed on it would
//!   have converted that inflation into amendment headroom at 2049×. The two
//!   are one figure now, which retires the hazard rather than the rule.
//!
//! **What the bound is honestly worth.** A capacity-majority can still assent
//! to a phantom commitment; no rule prevents a community from lying to itself,
//! and none ever will. What the bound changes is the shape of the failure: an
//! explosion becomes a slow, public, attributable leak, during which the only
//! real victims — lenders, who must still choose to deliver goods against the
//! inflated figure — can simply stop extending credit. The same humans were
//! trusted absolutely at genesis; this adds no trust assumption, it makes the
//! existing one repeatable and auditable.
//!
//! **Why an amendment endorses something concrete.** It endorses a §Recourse
//! liability — "this person will inherit debts up to X" — which is
//! enforceable, and which fails in public if it was vapor. Amendments without
//! substitution would endorse nothing, which is why §Recourse was built first.
//!
//! **No vesting, and that is a decision rather than an omission.** §Governance
//! offered an optional vested arrival, so that a raise lands while the
//! community watches it against §Adoption's honest metrics. Nothing revokes a vested
//! tranche, so vesting is a schedule nobody has to re-affirm — while running
//! the ceremony again per tranche produces the same arrival rate out of a
//! fresh decision each time, taken with the new utilisation, refusal and loss
//! figures in hand. The rate bound already forces the arrival to be gradual;
//! a schedule on top of it would only remove the re-affirmation, which is the
//! part worth keeping.

use edet_kernel::constants as k;

use crate::errors::*;
use crate::state::State;
use crate::types::*;

/// The external seed the epoch's rate is measured against: what was tracked
/// when the epoch opened.
///
/// Derived rather than stored, and the derivation is the point. Amendments
/// this epoch raised the roll by exactly `seed_amended_this_epoch`, so
/// subtracting it recovers the opening figure with one counter instead of a
/// snapshot that could drift from the thing it snapshots. A withdrawal during
/// the epoch lowers it further, which is the safe direction: the base can be
/// under-stated, never over.
fn rate_base(state: &State) -> u64 {
    state.external_minor().saturating_sub(state.seed_amended_this_epoch)
}

/// What one more epoch of amendments may still admit, in minor units.
fn remaining(state: &State) -> u64 {
    let ceiling = (rate_base(state) as f64 * state.params.seed_rate_bounded()).floor().max(0.0) as u64;
    ceiling.saturating_sub(state.seed_amended_this_epoch)
}

/// What this epoch may still admit, in denomination units — one of the figures
/// a community needs in order to plan a ceremony, and the honest answer to
/// "can this amendment enact now?".
pub fn headroom(state: &State) -> f64 {
    State::from_minor(remaining(state))
}

/// Would this amendment be admitted right now? Asked BEFORE anything is
/// written, because `apply` has no rollback: a handler that mutates and then
/// returns `Err` leaves the mutation on the ledger while the transaction is
/// reported failed.
///
/// **Zero stays absorbing, by two independent routes.** A ledger whose
/// external seed is nothing has a rate base of nothing, so no amendment can
/// ever be admitted — and its governance mass is zero for want of a
/// denominator, so no proposal of any kind can be enacted either. §Security says
/// no quantity computed from ledger events can seed insured credit; a
/// repeatable ceremony does not weaken that, because the ceremony is not a
/// ledger event about the ledger's own history. What it needs is a seed to be
/// a fraction OF, and a community that never had one still has none.
pub(crate) fn check(state: &State, author: MemberId, amount: f64) -> Res<()> {
    let m = state.members.get(&author).ok_or(Error(ET_MEM_UNKNOWN))?;
    // Re-checked at enactment and not only at proposal: assent can arrive
    // epochs later, and a member who has since been suspended or left has had
    // exactly the standing this amendment would give them taken away.
    if !matches!(m.status, MemberStatus::Active) {
        return Err(Error(ET_MEM_NOT_ACTIVE));
    }
    // Defence in depth: `propose` already refuses an amount the boundary
    // cannot name, and this runs again at enactment because that is where the
    // rate below binds, so it asks the same question about the same figure.
    let want = State::to_minor(amount);
    if amount <= 0.0 || !State::amount_representable(amount) || want == 0 {
        return Err(Error(ET_CTR_BAD_AMOUNT));
    }
    if want > remaining(state) {
        return Err(Error(ET_SEED_RATE));
    }
    // **The rate bounds how fast the roll grows and says nothing about where
    // it stops.** Compounding at β reaches any figure given epochs — about two
    // hundred of them from a large genesis — and a roll past
    // `MAX_AMOUNT_MINOR` is one the boundary can no longer name, which is the
    // same refusal every other ingress makes about one amount, asked of the
    // sum they add up to.
    if state.external_minor().saturating_add(want) > k::MAX_AMOUNT_MINOR {
        return Err(Error(ET_SEED_RATE));
    }
    Ok(())
}

/// Admit an endorsed external commitment: the author's declared supply rises
/// by `amount`.
///
/// **This and genesis are the only two places a supply may rise.**
/// `DeclareSupply` can lower one and nothing else: a declaration made against
/// capacity the community itself conferred is hollow insurance, and mints the
/// real label at 2000x the seed. So the underwriter roll IS the external
/// record, and there is no second map to keep beside it.
///
/// Only ever called after `check` on the same state, so every rejection has
/// already happened and this cannot fail on anything but a lookup.
pub(crate) fn enact(state: &mut State, author: MemberId, amount: f64) -> Res<()> {
    check(state, author, amount)?;
    let want = State::to_minor(amount);
    *state.underwriters.entry(author).or_insert(0) += want;
    state.seed_amended_this_epoch = state.seed_amended_this_epoch.saturating_add(want);
    Ok(())
}
