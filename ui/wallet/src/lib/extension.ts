/**
 * What an extension of a debt's maturity looks like before the ledger has
 * moved it.
 *
 * `Extend` is signed by both parties, so from the moment one side asks until
 * the other signs, the record a card shows stays at the OLD maturity. Two
 * readings bridge that gap, and both are pure so a card can preview a
 * maturity without the ledger's help: the term the form currently holds, and
 * an entry in the pending pool naming this contract.
 */

import type { PendingEntryView, PendingView } from './api';

/** A maturity extension parked in the pending pool, as one card sees it. */
export interface PendingExtension {
    newMaturity: number;
    /** True when this device is the one asked to sign it; false when it
     *  waits on the other party. */
    awaitingMe: boolean;
}

/**
 * The earliest maturity the form offers.
 *
 * The ledger's floor is the record itself (`apply::extend` refuses a maturity
 * at or below the current one). The form's is one higher than the clock as
 * well: a maturity still in the past leaves the debt overdue and markable
 * exactly as it was, so it is a value nobody means, and the label would have
 * to read "ends −9 epochs from now" to be honest about it.
 */
export function extensionFloor(currentMaturity: number, epoch: number): number {
    return Math.max(currentMaturity, epoch) + 1;
}

/**
 * A candidate maturity read against the clock and the record: how many epochs
 * from now it ends, how many it adds to the current term, and whether the form
 * would send it.
 */
export function extensionTerms(
    candidate: number,
    currentMaturity: number,
    epoch: number,
): { left: number; added: number; valid: boolean } {
    return {
        left: candidate - epoch,
        added: candidate - currentMaturity,
        valid: Number.isInteger(candidate) && candidate >= extensionFloor(currentMaturity, epoch),
    };
}

function extendOf(tx: Record<string, unknown>): { contract: number; newMaturity: number } | null {
    const b = tx?.Extend as { contract?: unknown; new_maturity_epoch?: unknown } | undefined;
    if (!b || typeof b.contract !== 'number' || typeof b.new_maturity_epoch !== 'number') return null;
    return { contract: b.contract, newMaturity: b.new_maturity_epoch };
}

/**
 * The extension of `contractId` waiting in the pool, if any. An entry that
 * waits on THIS device comes first: it is the one with an action attached.
 */
export function pendingExtensionOf(view: PendingView | null | undefined, contractId: number): PendingExtension | null {
    if (!view) return null;
    const find = (list: PendingEntryView[]) => list.find((e) => extendOf(e.tx)?.contract === contractId);
    const asked = find(view.awaiting_me);
    if (asked) return { newMaturity: extendOf(asked.tx)!.newMaturity, awaitingMe: true };
    const sent = find(view.mine);
    if (sent) return { newMaturity: extendOf(sent.tx)!.newMaturity, awaitingMe: false };
    return null;
}
