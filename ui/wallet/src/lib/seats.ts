/**
 * **Whether this member's standing carries one more account.**
 *
 * A trade that names a key with no row seats that row, and the seat is held
 * out of the SPONSOR's reach — one bond unit, for as long as the row exists.
 * The ledger refuses the trade at signing when the reach is short, but by
 * then the offer has gone out: in the civitas pilots six sponsors with no
 * reach opened 126 first-trade offers, and the newcomers tried to sign them
 * every day for a refusal that was never theirs to fix. So the question is
 * asked where the offer is composed, in the wallet's own words.
 */

import type { OperationBondView } from './api';

/** How many more newcomers the standing carries; null while it is unknown. */
export function seatsLeft(bond: OperationBondView | null | undefined): number | null {
    if (!bond || !(bond.unit > 0)) return null;
    return Math.floor(bond.seat_reach / bond.unit);
}

/**
 * Whether an offer that would seat a row may go out. Unknown standing is
 * not a refusal — the ledger has the last word either way — so only a
 * position that is KNOWN to carry nothing blocks.
 */
export function canSeat(bond: OperationBondView | null | undefined): boolean {
    const n = seatsLeft(bond);
    return n === null || n >= 1;
}
