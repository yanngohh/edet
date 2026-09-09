/**
 * What a member is actually deciding when they approve a supporter.
 *
 * Calling the approval "your moderation gate against unwanted
 * coupling", which is the paper's old answer and was the wrong one. A drain
 * cannot cost the member drained toward any money — it discharges their
 * obligations and creates none — and it is not what keeps strangers out either,
 * since the drain cap is zero for a pair that has never settled anything
 * whoever approved whom.
 *
 * **What it costs is the standing those obligations would have conferred.**
 * Routing a claim onto the buyer is a debtor swap, and the creditor whose claim
 * moves never signs it, so it writes no stake edge in either direction
 * (the paper's §Standing; the rule is that a stake is only ever placed by a
 * transition the creditor signed). Measured in `crates/state/tests/cascade.rs`:
 * an obligation of 80 cleared by a supporter confers 0 where the debtor paying
 * the same 80 confers 80, and three loans of 60 leave a member at capacity 200
 * carried against 380 honoured — same debt outcome, half the standing.
 *
 * So the decision is **relief now against standing later**, and a member cannot
 * make it without seeing which of their own obligations are in reach. That is
 * the whole of this module: mirror the ledger's own selection rule so the page
 * can name them.
 */

import type { ContractView, MemberDetail, SupportEdgeView } from './api';

/** What one sale by a given supporter could take out of the member's book. */
export interface DrainReach {
    /** Whether this edge can route anything at all. */
    routed: boolean;
    /**
     * Whether the book this was computed over was visible to the reader at all.
     * `false` is "cannot say", never "nothing" — see `reachBySupporter`.
     */
    visible: boolean;
    /**
     * The most of the member's own debt a single sale by this supporter could
     * clear. An upper bound, and deliberately so: the actual share depends on
     * how large that sale is and how the supporter's weights waterfill, neither
     * of which exists yet at the moment the member is deciding.
     */
    clears: number;
    /** The obligations it would take, oldest first — the ones whose settlement
     *  would otherwise have written their creditor's stake. */
    contracts: number[];
}

/** No route and nothing hidden — the answer for an edge that carries nothing. */
export const EMPTY_REACH: DrainReach = { routed: false, visible: true, clears: 0, contracts: [] };
const EMPTY = EMPTY_REACH;
const HIDDEN: DrainReach = { routed: false, visible: false, clears: 0, contracts: [] };

/**
 * Mirror `cascade.rs::clear_member_debts`' selection: **Active** obligations
 * only, oldest first, partial allowed, up to the cap.
 *
 * Three deliberate differences from the ledger, each of which can only make
 * this an over-estimate and never an under-estimate — the safe direction for a
 * figure that describes what a member is giving up.
 *
 * - The ledger also skips obligations owed to the *buyer* of that sale, and no
 *   buyer exists yet.
 * - It bounds each one by what that buyer can carry insured, which is likewise
 *   unknown here.
 * - `drainCap` is the ceiling on the edge, not the share the waterfill will
 *   actually hand it.
 *
 * Expired rows are excluded because the ledger excludes them: a defaulted claim
 * is not routed, so the cascade cannot rescue a member who has already fallen
 * due. `undefined` (an unauthenticated read) is not zero and is not a
 * route: it is "nothing is known", so it reports no reach rather than a
 * reassuring one.
 */
export function drainReach(drainCap: number | null | undefined, owes: ContractView[]): DrainReach {
    if (typeof drainCap !== 'number' || !Number.isFinite(drainCap) || drainCap <= 0) return EMPTY;
    const live = owes
        .filter((c) => c.status === 'active' && c.outstanding > 0)
        .sort((a, b) => a.id - b.id); // ascending id = oldest first, as the ledger walks them
    let left = drainCap;
    const contracts: number[] = [];
    let clears = 0;
    for (const c of live) {
        if (left <= 0) break;
        const take = Math.min(c.outstanding, left);
        clears += take;
        left -= take;
        contracts.push(c.id);
    }
    return { routed: clears > 0, visible: true, clears, contracts };
}

/**
 * Which sentence the page should put under a supporter's row.
 *
 * Four different facts, and merging any two of them is how a page ends up
 * telling a member something the ledger does not say. `no-route` means the pair
 * has never settled anything, which is escaped by trading; `nothing-to-clear`
 * means the member has no live obligations, which is not a problem at all;
 * `unknown` means this reader cannot see the book (below); and only `trade-off`
 * is a decision.
 */
export type SupportPrompt = 'no-route' | 'nothing-to-clear' | 'unknown' | 'trade-off';

export function supportPrompt(reach: DrainReach, drainCap: number | null | undefined): SupportPrompt {
    if (typeof drainCap !== 'number' || !Number.isFinite(drainCap) || drainCap <= 0) return 'no-route';
    if (reach.routed) return 'trade-off';
    return reach.visible ? 'nothing-to-clear' : 'unknown';
}

/**
 * The reach of each supporter listed against this member, keyed by member id.
 *
 * **Takes the whole record, because an empty `owes` is two different facts**
 * The node serves `owes` filtered to the contracts the READER is a party
 * to, so a member reading their own detail gets their whole book while anybody
 * else gets an empty array — and an empty array read as "nothing to clear"
 * would print a reassurance the ledger never gave. `debt` is served to every
 * reader (bucketed for a non-party, but never absent and never zero when the
 * member owes something), so the two together tell an empty book from a hidden
 * one. That discriminator is the whole reason this takes a `MemberDetail`
 * rather than the two arrays.
 *
 * `dust` is what makes the discriminator safe rather than merely clever. A book
 * whose rows have all closed can leave a residue in `debt` that no row accounts
 * for — the close forgives dust and the cache follows the book — and reading
 * that residue as "the book is hidden" would print "not visible here" to a
 * member who simply owes nothing. Pass `params.dust`; the ledger's own gates
 * compare against it everywhere for the same reason.
 */
export function reachBySupporter(member: MemberDetail | null | undefined, dust = 0): Map<number, DrainReach> {
    const out = new Map<number, DrainReach>();
    if (!member) return out;
    const owes = member.owes ?? [];
    const visible = owes.length > 0 || !(member.debt > dust);
    for (const s of member.supporters ?? []) {
        out.set(s.member, visible ? drainReach(s.drain_cap, owes) : HIDDEN);
    }
    return out;
}
