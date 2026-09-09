/**
 * Who may do what on the governance page, decided once and in one place.
 *
 * **Members propose; the ceremony enacts** (the paper's §Governance). Assent weight
 * is a share of the community's EXTERNAL seed — genesis, plus every commitment
 * a later ceremony endorsed — so a member holding none of it carries none of
 * the vote, and the ledger REFUSES such an assent (`ET-GOV-007`) rather than
 * recording it at zero.
 *
 * A page that offered the button to every member and let that refusal
 * explain itself. That is the wrong way round: a control that is always refused
 * is a promise the chain does not keep, and it is the same shape as a view
 * describing a mechanism the chain does not run. What replaces it is not a
 * hidden button but a sentence saying who the electorate is and why.
 *
 * Pure, because the component around it cannot be tested and this can.
 */

import type { MemberDetail, ProposalView } from './api';

/** Why the Assent control is or is not offered on one proposal. */
export type AssentVerdict =
    /** Decided, or nobody is signed in: there is nothing to offer. */
    | 'none'
    /** Already assented — say so rather than offering it twice. */
    | 'assented'
    /** Not known yet: the member detail has not arrived. */
    | 'unknown'
    /** Outside the electorate: their backing came from inside the community. */
    | 'no-mandate'
    /** Their own seed amendment — a vote cast with the backing it grants. */
    | 'own-amendment'
    /** Offer it. */
    | 'offer';

/**
 * @param me the signed-in member's id, or null
 * @param detail the signed-in member's own detail view, or null while it loads
 * @param p the proposal
 * @param dust the ledger's dust threshold, so "declared nothing" and "declared
 *        a rounding error" are the same answer
 */
export function assentVerdict(
    me: number | null,
    detail: MemberDetail | null,
    p: ProposalView,
    dust: number,
): AssentVerdict {
    if (me === null || p.enacted) return 'none';
    if (p.assents.includes(me)) return 'assented';
    // `undefined` reads as "not known yet", never as "no". The detail view is
    // polled, and a refusal that flickered on every slow poll would be worse
    // than either answer — the same rule the acceptance engine applies to a
    // missing risk field.
    if (!detail) return 'unknown';
    if ((detail.supply?.declared ?? 0) <= dust) return 'no-mandate';
    // `ET-SED-002`: the author of a seed amendment IS its beneficiary, because
    // the kind names nobody. Their assent would be a vote on their own supply,
    // cast with the weight that supply is about to enlarge.
    if (p.kind.type === 'seed_amendment' && p.author === me) return 'own-amendment';
    return 'offer';
}
