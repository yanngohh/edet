/**
 * What the wallet should say about a member's standing and their write
 * allowance — decided once, and tested, because the component around it cannot
 * be.
 *
 * Two facts the wallet kept getting wrong, both by asking the wrong quantity:
 *
 *   * **The allowance cliff.** The free allowance goes only to accounts the
 *     community has put something behind (`conferrable > dust`), because a
 *     per-key allowance is the free-signature bound's own defect one layer
 *     down. So standing can fall through no act of the member's own — an
 *     underwriter reduces a supply, a stake decays — and take every free action
 *     with it. Nothing warned them, and worse, the `free_remaining` figure the
 *     node served did not know about the test either, so the wallet showed
 *     "32 of 32" in the good tone while every write came back `ET-BND-001`.
 *
 *   * **"You're new here."** The bootstrap banner keyed on `capacity`, and a
 *     founding underwriter's capacity is ZERO — a cut into an underwriter draws
 *     on the OTHER underwriters, and at genesis nobody has backed anybody. So
 *     the one member carrying the community was told nobody had backed them.
 *
 * Both are the same question, and the ledger now answers it directly:
 * `operation_bond.established`, never computed here as
 * `conferrable > dust`, which is what the gate compared when this file was
 * written — and the gate's reading is GROSS of live credit,
 * because the residual one made the qualification a statement about how busy a
 * member's backers were rather than about whether they had any (measured: a
 * full allowance to zero at 20% community utilisation, for a member who had
 * borrowed nothing). A client that re-derives the ledger's test is right until
 * the ledger's test moves, and this is the second time this file has been the
 * place that found out.
 */

import type { MemberDetail } from './api';

/** What the wallet should tell the member about writing. */
export type AllowanceState =
    /** Not known yet: no detail, or a read without full access. Say nothing. */
    | 'unknown'
    /** Nothing is behind them, so the allowance does not apply at all. */
    | 'no-standing'
    /** Backed, but carrying an open default — which is what a default costs,
     *  and it does not come back with the next epoch. Distinct from `spent`
     *  because "you have used every free action this epoch" would be a promise
     *  about tomorrow that the ledger will not keep: the allowance returns when
     *  the default is cured, not when the epoch turns. */
    | 'defaulted'
    /** Standing, but every free action for this epoch is spent. */
    | 'spent'
    /** Standing, and free actions remain. */
    | 'ok';

export function allowanceState(me: MemberDetail | null, dust: number): AllowanceState {
    // `operation_bond` is absent for a read without full access, and absent is
    // "unknown" rather than zero: displaying zero would warn about a state the
    // member is not in.
    if (!me || !me.operation_bond || me.operation_bond.established === undefined) return 'unknown';
    if (!me.operation_bond.established) return 'no-standing';
    if ((me.open_default ?? 0) > dust) return 'defaulted';
    return me.operation_bond.free_remaining === 0 ? 'spent' : 'ok';
}

/**
 * Is this account one the community has put nothing behind?
 *
 * True for a newcomer and for a member whose backing has lapsed — the wallet
 * says the same thing to both, because it cannot tell them apart from public
 * state and the operational advice is identical. False for a founding
 * underwriter, whose capacity is zero and whose declared supply is not; false
 * for a member whose backers are merely fully drawn, who has standing behind
 * them and no free credit; and false for a defaulter, who is told about the
 * default instead.
 */
export function hasNoStanding(me: MemberDetail | null, dust: number): boolean {
    return allowanceState(me, dust) === 'no-standing' && me?.status === 'active';
}
