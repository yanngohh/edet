//! What an underwriter actually pays (the paper's §Recourse).
//!
//! The *bound* on an underwriter's loss was exact and measured long before
//! this file existed; what was missing was the settlement of it. In a system
//! with no external redemption, "you lose 500" has to name something concrete,
//! and one constraint shapes the whole answer:
//!
//! > **In a closed system, the only way anyone can pay is to become a debtor.**
//! > There is no asset to hand over. "You lose 500" can only mean "you now owe
//! > 500 of goods and services."
//!
//! So when an insured obligation defaults, two things happen at once and they
//! are the same event seen from either end:
//!
//! - **Substitution.** The creditor's claim moves onto the underwriters, split
//!   by exactly what each one's supply arc carried. Each becomes an ordinary
//!   debtor of the creditor, and discharges the way anyone discharges
//!   anything — delivering value, or netting it against their own trade.
//! - **Subrogation.** The defaulter's debt does not vanish; it now runs to the
//!   underwriters instead of to the creditor. If the defaulter ever `Cure`s,
//!   the underwriter recovers, because the creditor is already whole. This is
//!   classical insurance subrogation arrived at from inside the model, and it
//!   is why `Cure` needs no special case here: only a new creditor.
//!
//! **Authorization.** Every discharge is authorised by the party who loses if
//! it is wrong. The loser is the underwriter, and the underwriter authorised
//! it at `DeclareSupply` — which is the reframing the ceremony needed. A
//! supply declaration is not a number, it is *a signed, standing consent to
//! inherit the debts of those the community's stakes reach through you, up to
//! this amount*, and §Adoption can finally say that sentence to a founding
//! underwriter.
//!
//! **Where the flow goes, and why §Verification needs no new term.** §Recourse expected
//! invariant 2 to gain a second one — `Σ committed` = outstanding insured
//! *plus* absorbed-unpaid losses. It does not, because the committed flow
//! never leaves a live insured claim: the subrogated claim IS the original
//! obligation with its creditor moved, so it keeps the supply arc and the
//! defaulter's edges untouched. Three consequences fall out of that one
//! choice, and each is a property §Recourse asked for by another route:
//!
//! - **The defaulter's standing stays consumed.** A default does not release
//!   the flow it committed, and substitution does not release it either — so
//!   stealing still costs the thief exactly what they hold, once. Had the
//!   subrogated claim been released, the community paying a loss would have
//!   *restored* the member who caused it.
//! - **The underwriter's supply stays drawn** until the defaulter repays them,
//!   which for a real default is indefinitely. Unpaid absorbed losses throttle
//!   the declaration automatically, and §Stability's floor already stops a withdrawal
//!   out from under it. This is strictly stronger than releasing when the
//!   underwriter pays the creditor, which would let them absorb a loss, settle
//!   it, and immediately insure the same amount again.
//! - **The reservation caches never move.** The pieces partition the original
//!   `Held` exactly, arc by arc, so `reserved` and `committed` are unchanged
//!   by construction rather than recomputed and checked.
//!
//! **The substitution leg is uninsured, deliberately.** Insuring it would draw
//! on what *others* have put behind the underwriter — the loss cascading to
//! the next ring of underwriters, which is exactly the path contagion §Recourse
//! refuses. Losses land on source arcs, not on paths.
//!
//! **The bottom of the waterfall.** If the underwriter defaults on the
//! substituted obligation too, the creditor finally bears the loss — visibly
//! and attributably, because that second default is an ordinary `MarkExpired`
//! on an uninsured claim and substitutes nothing. Without an external asset
//! the system cannot do better and must not pretend to. What substitution buys
//! is that a phantom declaration now **fails in public**.
//!
//! **What it deliberately does not provide is compensation.** An underwriter
//! bears real expected loss and earns nothing; `prop:no-rent` keeps it that
//! way. Underwriting stays a club good.

use edet_kernel::flow;

use crate::apply::{contract, contract_mut, debt_after, horizon, member_mut};
use crate::errors::*;
use crate::state::State;
use crate::types::*;

/// Substitute the underwriters into a defaulted obligation, and subrogate the
/// defaulter's debt to them.
///
/// A no-op for an obligation the community never stood behind: an uninsured
/// claim holds no supply arc, so there is nobody to substitute and the
/// creditor bears it alone (§Recourse). That is also what stops the recursion — the
/// substitution legs minted here are uninsured, so a default on one of them
/// reaches this function and returns immediately.
pub(crate) fn substitute(state: &mut State, contract_id: ContractId) -> Res<()> {
    let c = contract(state, contract_id)?;
    if !c.insured || c.held.supply.is_empty() {
        return Ok(());
    }
    // In underwriter order, which `flow::reserve` already guarantees — so the
    // split is a function of state rather than of insertion history, like
    // every other quantity in the model.
    let shares: Vec<(usize, u64)> = c.held.supply.clone();
    // A creditor who is themselves the only underwriter behind the claim
    // insured it themselves, and there is nothing to move. The same clause
    // that stops a coalition underwriting itself, one level down.
    if shares.iter().all(|&(u, _)| u as MemberId == c.creditor) {
        return Ok(());
    }
    let total: u64 = shares.iter().map(|&(_, f)| f).sum();
    if total == 0 {
        return Ok(());
    }
    // Defensive, and cheap next to what it prevents. The kernel already
    // guarantees a debtor never draws on their own supply arc — underwriters
    // inside the measured set supply it nothing, which is the same clause that
    // stops a coalition underwriting itself — so this cannot fire. If it ever
    // did, the split would mint a claim a member owed themselves, §Verification would
    // refuse it, and the audit runs on the COMMIT path: every honest node
    // would halt at once. A guard is the right shape for a failure whose blast
    // radius is the chain.
    if shares.iter().any(|&(u, _)| u as MemberId == c.debtor) {
        return Ok(());
    }
    let weights: Vec<u64> = shares.iter().map(|&(_, f)| f).collect();
    // **Split by ROUTE, so every piece is still a flow.** A proportional split
    // of each arc gives each underwriter a share of arcs its own supply never
    // reached, and the pieces stop being routes: the sums still reconstitute
    // the original arc for arc, so the audit stays green, but cure one piece
    // and what is left carries flow no source arrives at. Measured before this
    // moved: after a partial cure, 71,429 leaving an account with nothing
    // arriving at it. Routes divide without that, because a route can be cut to
    // any value and each piece is a route, and the division is exact --- it
    // partitions integers that are already there rather than rounding a ratio.
    //
    // The fallback is the old split, not a halt. A `Held` that will not
    // decompose is already broken, and the audit is what says so; refusing to
    // substitute here would leave the loss unallocated, which is worse.
    let edge_pieces = match flow::decompose(&c.held, c.debtor as usize) {
        Ok(paths) => shares
            .iter()
            .map(|&(u, _)| {
                let mine: Vec<flow::Path> = paths.iter().filter(|p| p.source == u).cloned().collect();
                flow::from_paths(&mine).edges
            })
            .collect(),
        Err(_) => split_edges(&c.held.edges, &weights, total),
    };

    // The amount each piece carries. `held.amount()` is `to_minor(outstanding)`
    // by construction, so the shares sum to at most the outstanding figure and
    // the sub-minor remainder — under one hundredth of a unit — goes to the
    // first piece. Without that the split would quietly destroy a fraction of
    // the debt and the conservation audit would say so.
    let mut amounts: Vec<u64> = weights.clone();
    let residual = c.outstanding.saturating_sub(amounts.iter().sum::<u64>());
    amounts[0] += residual;

    let maturity_epoch = horizon(state.epoch, state.params.min_maturity_epochs);
    for (i, &(u, f)) in shares.iter().enumerate() {
        let underwriter = u as MemberId;
        let held = flow::Held { edges: edge_pieces[i].clone(), supply: vec![(u, f)] };
        if i == 0 {
            // The original row becomes the first underwriter's claim, so the
            // obligation keeps its own history — its id, its creation epoch,
            // the maturity it was booked under.
            let cm = contract_mut(state, contract_id)?;
            cm.creditor = underwriter;
            cm.outstanding = amounts[0];
            cm.original = amounts[0];
            cm.held = held;
            // The consented arbitration channel STAYS, attestations and all.
            // It names the original parties on its own terms (`ArbTerms::
            // debtor`, `creditor`, `amount`), so the change of creditor here
            // does not reach it: the award is still the buyer's remedy against
            // the seller for non-delivery, minted between them and bounded by
            // the original amount, and never against U, who was never asked.
            // Dropping it here — because the row's creditor had changed —
            // closed the remedy in exactly the case arbitration exists for: a
            // buyer who withheld payment for non-delivery defaulted, and lost
            // both their standing and their panel.
            //
            // **No `pool_covered = false` belongs here.** A loss may be
            // mutualized once, and a flag cleared at this point would be
            // trying to stop a second layer paying a creditor the underwriters
            // had just made whole — a job it cannot do, because both of the
            // early returns above leave before it. A claim insured solely by
            // its creditor's own arc takes that exit and stays covered: U
            // (2500) lending D 300 on U's own arc has the 300 written off, D
            // given back its whole standing, and an honest covenanter drained
            // to 200.60 — the defaulter released and the members who paid
            // holding nothing. **The tier is the whole of the rule**, and a
            // tier cannot be enforced by a flag cleared inside one branch of
            // something else. There is no layer under the uninsured tier
            // (§Recourse), which closes it at the tier.
        } else {
            subrogated_claim(state, &c, underwriter, amounts[i], held)?;
        }
        // The creditor's claim lands on the underwriter as an ordinary debt.
        // Not when they are one and the same: a creditor cannot owe themselves
        // (§Verification refuses it), and a self-insured share is simply borne.
        if underwriter != c.creditor {
            substitution_leg(state, underwriter, c.creditor, amounts[i], maturity_epoch)?;
        }
    }
    Ok(())
}

/// One more subrogated claim: the same debtor, the same default, a new
/// creditor, and the slice of the original augmentation that this
/// underwriter's arc carried.
fn subrogated_claim(
    state: &mut State,
    original: &Contract,
    underwriter: MemberId,
    amount: u64,
    held: flow::Held,
) -> Res<()> {
    let id = state.next_contract;
    state.next_contract += 1;
    state.contracts.insert(
        id,
        Contract {
            id,
            debtor: original.debtor,
            creditor: underwriter,
            outstanding: amount,
            original: amount,
            maturity_epoch: original.maturity_epoch,
            // Still in default. Substitution settles who is owed, never
            // whether the debt was paid — and `Cure` is the way out of it,
            // for the same debtor, under the same rule, toward whoever holds
            // the claim now.
            status: ContractStatus::Expired,
            created_epoch: original.created_epoch,
            accepted_epoch: original.accepted_epoch,
            insured: true,
            held,
            arb: None,
            arb_attestations: Default::default(),
            arb_awarded: false,
        },
    );
    // `debt_out` is deliberately NOT touched: the defaulter owed this before
    // the split and owes exactly the same after it. The pieces partition the
    // original amount, so the cached total and the book move together by
    // moving not at all.
    Ok(())
}

/// The underwriter's own new liability: they now owe the creditor what their
/// arc carried.
///
/// **Uninsured**, and that is the load-bearing word. An insured leg would draw
/// on what others have put behind this underwriter — the loss walking one hop
/// further out through the graph, which is the path contagion the model
/// refuses. The sanction lives at the source arc, and this is the source arc's
/// bill.
fn substitution_leg(
    state: &mut State,
    underwriter: MemberId,
    creditor: MemberId,
    amount: u64,
    maturity_epoch: u64,
) -> Res<()> {
    // Zero, not dust. Everywhere else in the alphabet a dust-sized amount is
    // economically nothing and is dropped on the floor; here the same share is
    // the creditor's claim, and the piece of the defaulted obligation matching
    // it has ALREADY moved to this underwriter. Dropping it would not round a
    // quantity down, it would make the creditor whole for less than they lost
    // — once per underwriter on the split, silently.
    if amount == 0 {
        return Ok(());
    }
    // Before the row, as everywhere else a claim is minted: `dispatch` has no
    // rollback, so a checked add evaluated after the insert would leave a leg
    // on the book with nothing behind it in the underwriter's cached debt.
    let debt_out = debt_after(state, underwriter, amount)?;
    let id = state.next_contract;
    state.next_contract += 1;
    state.contracts.insert(
        id,
        Contract {
            id,
            debtor: underwriter,
            creditor,
            outstanding: amount,
            original: amount,
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
    let m = member_mut(state, underwriter)?;
    m.debt_out = debt_out;
    m.rep.d_in += State::from_minor(amount);
    Ok(())
}

/// Shrink a defaulted claim's hold to `keep` of its `total`, arc by arc, and
/// return `(what still stands, what is given back)`.
///
/// **This is §Recourse's load-bearing line, made true.** "The supply arc stays
/// committed until the defaulter repays the underwriter" assumes the hold that
/// remains after a partial repayment sits where the old one sat. Re-solving it
/// does not: `flow::reserve` answers with whatever augmenting path the residual
/// offers, in underwriter id order, so a defaulter who honours one ordinary
/// trade with a third party gives the solver a cheaper arc to find and the
/// throttle moves onto an underwriter who was never substituted. Measured
/// before this existed: D insured on U_b alone, default, one honoured uninsured
/// purchase from U_a, cure 100 → **committed `{U_b: 300}` → `{U_a: 200}`** —
/// U_b free to insure 300 again having absorbed a loss it has not paid, and
/// U_a's supply frozen indefinitely by a claim owed to somebody else.
///
/// So a repayment releases exactly the share it paid, on the arcs that were
/// holding it, and nothing else moves. The two pieces sum to the original arc
/// for arc, which is what lets the caller give back the second one and leave
/// `reserved` and `committed` exact.
///
/// The two sides are allocated differently on purpose. The **edges** take the
/// same per-arc largest-remainder split the subrogation uses, with the shares
/// being "still owed" and "just paid". The **supply** side is allocated
/// GLOBALLY, because it is the quantity §Verification invariant 2 compares against the
/// book — the kept shares must sum to `keep` exactly, and a per-entry split
/// does not: two entries of 1 keeping 1 of 2 would each round their own share
/// up and keep 2.
pub(crate) fn shrink(held: &flow::Held, debtor: usize, keep: u64, total: u64) -> (flow::Held, flow::Held) {
    let weights: Vec<u64> = held.supply.iter().map(|&(_, a)| a).collect();
    // The supply side is allocated globally and that does not change: it is the
    // quantity §Verification invariant 2 compares against the book, and it is
    // what keeps a partial repayment from moving a loss off the underwriter
    // that absorbed it.
    let kept = allocate(&weights, keep, total);

    // The EDGE side splits by ROUTE, never per arc. A per-arc proportional
    // split does not preserve a route: the two pieces sum back to the original
    // arc for arc while neither is a flow. Cutting the routes keeps both halves
    // flows, and the arc totals are then sums of integers rather than rounded
    // ratios, so they cannot drift from `reserved`.
    if let Ok(paths) = flow::decompose(held, debtor) {
        let mut still_paths: Vec<flow::Path> = Vec::new();
        let mut back_paths: Vec<flow::Path> = Vec::new();
        for (i, &(u, _)) in held.supply.iter().enumerate() {
            let mine: Vec<&flow::Path> = paths.iter().filter(|p| p.source == u).collect();
            let vals: Vec<u64> = mine.iter().map(|p| p.value).collect();
            let mine_total: u64 = vals.iter().sum();
            // Each underwriter's own kept share, spread over its own routes.
            let take = allocate(&vals, kept[i].min(mine_total), mine_total);
            for (j, pth) in mine.iter().enumerate() {
                if take[j] > 0 {
                    still_paths.push(flow::Path { source: pth.source, arcs: pth.arcs.clone(), value: take[j] });
                }
                if pth.value > take[j] {
                    back_paths.push(flow::Path {
                        source: pth.source,
                        arcs: pth.arcs.clone(),
                        value: pth.value - take[j],
                    });
                }
            }
        }
        return (flow::from_paths(&still_paths), flow::from_paths(&back_paths));
    }

    // Same fallback as `substitute`: a hold that will not decompose is already
    // broken, and halting a discharge is not the way to report it.
    let pieces = split_edges(&held.edges, &[keep, total - keep], total);
    let mut still = flow::Held { edges: pieces[0].clone(), supply: Vec::new() };
    let mut back = flow::Held { edges: pieces[1].clone(), supply: Vec::new() };
    for (i, &(u, had)) in held.supply.iter().enumerate() {
        if kept[i] > 0 {
            still.supply.push((u, kept[i]));
        }
        if had > kept[i] {
            back.supply.push((u, had - kept[i]));
        }
    }
    (still, back)
}

/// Allocate `keep` across `amounts` in proportion to them — largest remainder,
/// ties by index — so the shares sum to exactly `keep` whenever
/// `keep <= total = Σ amounts`.
fn allocate(amounts: &[u64], keep: u64, total: u64) -> Vec<u64> {
    let mut out = vec![0u64; amounts.len()];
    if total == 0 {
        return out;
    }
    let mut assigned = 0u64;
    let mut remainders: Vec<(u128, usize)> = Vec::with_capacity(amounts.len());
    for (i, &a) in amounts.iter().enumerate() {
        // `u128`: a share times an amount is a product of two minor-unit
        // quantities and would wrap a `u64` well inside the range a real ledger
        // reaches.
        let num = a as u128 * keep as u128;
        let q = (num / total as u128) as u64;
        out[i] = q;
        assigned += q;
        remainders.push((num % total as u128, i));
    }
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut left = keep.saturating_sub(assigned);
    for &(_, i) in &remainders {
        if left == 0 {
            break;
        }
        // Never above what that entry actually held: the shares are a partition
        // of the old hold, not a fresh reservation.
        if out[i] < amounts[i] {
            out[i] += 1;
            left -= 1;
        }
    }
    out
}

/// Split each held edge across the underwriters in proportion to what their
/// arcs carried, exactly.
///
/// Largest remainder, ties by index, so every unit of every arc lands
/// somewhere and the pieces sum to the original arc for arc. Exactness is the
/// whole requirement: `reserved` and `committed` are the cached sum of every
/// live obligation's `Held`, and §Verification invariant 2 compares them for equality
/// rather than closeness — a split that lost a minor unit would make the two
/// views of one fact disagree with nothing having gone wrong.
///
/// A piece is *not* an augmenting path in its own right, and does not need to
/// be. Every invariant over `Held` is additive across obligations that share a
/// debtor — which these do, all of them — so the sums the audit checks are
/// the sums it checked before. What a piece is, is the share of one
/// augmentation that one underwriter's arc carried, which is exactly what a
/// subrogated claim should hold.
fn split_edges(edges: &[((usize, usize), u64)], weights: &[u64], total: u64) -> Vec<Vec<((usize, usize), u64)>> {
    let k = weights.len();
    let mut out: Vec<Vec<((usize, usize), u64)>> = vec![Vec::new(); k];
    for &(key, amount) in edges {
        let mut alloc = vec![0u64; k];
        let mut assigned: u64 = 0;
        // `u128` throughout: an amount times a weight is a product of two
        // minor-unit quantities and would wrap a `u64` well inside the range
        // a real ledger reaches.
        let mut remainders: Vec<(u128, usize)> = Vec::with_capacity(k);
        for (i, &w) in weights.iter().enumerate() {
            let num = amount as u128 * w as u128;
            let q = (num / total as u128) as u64;
            alloc[i] = q;
            assigned += q;
            remainders.push((num % total as u128, i));
        }
        remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut left = amount.saturating_sub(assigned);
        for &(_, i) in &remainders {
            if left == 0 {
                break;
            }
            alloc[i] += 1;
            left -= 1;
        }
        for (i, a) in alloc.into_iter().enumerate() {
            if a > 0 {
                out[i].push((key, a));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one property the split must have: every unit lands somewhere, and
    /// the pieces sum to the original arc for arc. Checked over uneven weights
    /// and amounts that do not divide, because those are the cases where a
    /// naive proportional split silently loses a unit.
    #[test]
    fn the_edge_split_is_exact_however_it_divides() {
        for weights in [vec![1u64, 2, 7], vec![3, 3], vec![1, 1, 1, 1], vec![999_983, 17]] {
            let total: u64 = weights.iter().sum();
            for amount in [1u64, 2, 7, 100, 12_345, 999_999] {
                let edges = vec![((0usize, 1usize), amount), ((1, 2), amount + 3)];
                let pieces = split_edges(&edges, &weights, total);
                assert_eq!(pieces.len(), weights.len());
                for (key, want) in [((0usize, 1usize), amount), ((1, 2), amount + 3)] {
                    let got: u64 = pieces
                        .iter()
                        .flat_map(|p| p.iter())
                        .filter(|&&(k, _)| k == key)
                        .map(|&(_, a)| a)
                        .sum();
                    assert_eq!(got, want, "weights {weights:?}, amount {amount}: arc {key:?} lost or gained");
                }
            }
        }
    }

    /// `allocate` must sum to exactly `keep`, and never hand an entry more than
    /// it held. Both matter: the first is what §Verification invariant 2 compares against
    /// the book, and the second is what makes the shares a partition of an
    /// existing hold rather than a fresh reservation.
    ///
    /// The awkward case is the one a per-entry split gets wrong — two entries of
    /// 1 keeping 1 of 2 would each round its own share up and keep 2.
    #[test]
    fn the_supply_allocation_sums_to_exactly_what_is_kept() {
        for amounts in [vec![1u64, 1], vec![1, 2, 7], vec![3, 3], vec![999_983, 17], vec![5], vec![2, 2, 2, 2]] {
            let total: u64 = amounts.iter().sum();
            for keep in 0..=total.min(64) {
                let got = allocate(&amounts, keep, total);
                assert_eq!(got.iter().sum::<u64>(), keep, "amounts {amounts:?}, keep {keep}: {got:?}");
                for (i, &g) in got.iter().enumerate() {
                    assert!(g <= amounts[i], "amounts {amounts:?}, keep {keep}: entry {i} grew to {g}");
                }
            }
            assert_eq!(allocate(&amounts, total, total), amounts, "keeping everything moves nothing");
        }
    }

    /// The two sides of a shrink partition the old hold exactly: what stays plus
    /// what is given back is what was held, arc for arc and underwriter for
    /// underwriter. That is what lets the caller release the second piece and
    /// leave `reserved` and `committed` exact.
    #[test]
    fn a_shrink_partitions_the_hold_it_came_from() {
        let held = flow::Held {
            edges: vec![((0, 1), 1000), ((1, 3), 1000), ((0, 2), 1001), ((2, 3), 1001)],
            supply: vec![(0, 2001)],
        };
        let total = held.amount();
        for keep in 0..=total {
            let (still, back) = shrink(&held, 3, keep, total);
            assert_eq!(still.amount(), keep, "keep {keep}");
            assert_eq!(back.amount(), total - keep, "keep {keep}");
            for &(k, had) in &held.edges {
                let get = |h: &flow::Held| h.edges.iter().filter(|&&(x, _)| x == k).map(|&(_, a)| a).sum::<u64>();
                assert_eq!(get(&still) + get(&back), had, "keep {keep}: arc {k:?} lost or gained");
            }
        }
    }

    /// A single underwriter takes the whole arc, with no rounding anywhere —
    /// the common case in a small community, and the one where an off-by-one
    /// would be least visible.
    #[test]
    fn a_single_underwriter_takes_the_whole_augmentation() {
        let edges = vec![((0usize, 1usize), 250_000u64), ((1, 2), 250_000)];
        let pieces = split_edges(&edges, &[250_000], 250_000);
        assert_eq!(pieces, vec![edges]);
    }
}
