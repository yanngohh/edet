import { describe, expect, it } from 'vitest';

import type { PendingEntryView, PendingView } from '../api';
import { extensionFloor, extensionTerms, pendingExtensionOf } from '../extension';

function entry(tx: Record<string, unknown>, initiator = 1): PendingEntryView {
    return {
        digest: JSON.stringify(tx),
        tx,
        nonce: [],
        not_after_epoch: 0,
        required: [],
        min_sigs: 2,
        initiator,
        created_secs: 0,
        signed_by: [],
    };
}

const extend = (contract: number, epoch: number) => entry({ Extend: { contract, new_maturity_epoch: epoch } });

describe('extensionFloor', () => {
    it('is strictly later than the record, which is where the ledger refuses', () => {
        expect(extensionFloor(20_686, 20_674)).toBe(20_687);
    });
    it('is strictly later than the clock once the record is overdue', () => {
        expect(extensionFloor(20_686, 20_700)).toBe(20_701);
    });
});

describe('extensionTerms', () => {
    it('reads a candidate against the clock and the record', () => {
        expect(extensionTerms(20_716, 20_686, 20_674)).toEqual({ left: 42, added: 30, valid: true });
    });
    it('refuses the record itself and anything below it', () => {
        expect(extensionTerms(20_686, 20_686, 20_674).valid).toBe(false);
        expect(extensionTerms(20_600, 20_686, 20_674).valid).toBe(false);
    });
    it('refuses a maturity that would still be in the past', () => {
        expect(extensionTerms(20_690, 20_686, 20_700).valid).toBe(false);
        expect(extensionTerms(20_701, 20_686, 20_700)).toEqual({ left: 1, added: 15, valid: true });
    });
    it('refuses what is not a whole epoch', () => {
        expect(extensionTerms(Number.NaN, 20_686, 20_674).valid).toBe(false);
        expect(extensionTerms(20_700.5, 20_686, 20_674).valid).toBe(false);
    });
});

describe('pendingExtensionOf', () => {
    const view: PendingView = {
        awaiting_me: [extend(7, 300), entry({ Settle: { contract: 9, amount: 1 } })],
        mine: [extend(9, 400), extend(7, 350)],
    };

    it('finds nothing without a view', () => {
        expect(pendingExtensionOf(null, 7)).toBeNull();
        expect(pendingExtensionOf(undefined, 7)).toBeNull();
    });
    it('reports an extension this device is asked to sign', () => {
        expect(pendingExtensionOf(view, 7)).toEqual({ newMaturity: 300, awaitingMe: true });
    });
    it('reports an extension waiting on the other party', () => {
        expect(pendingExtensionOf(view, 9)).toEqual({ newMaturity: 400, awaitingMe: false });
    });
    it('ignores other kinds of request on the same contract', () => {
        expect(pendingExtensionOf({ awaiting_me: [entry({ Settle: { contract: 9, amount: 1 } })], mine: [] }, 9)).toBeNull();
    });
    it('ignores extensions of other contracts', () => {
        expect(pendingExtensionOf(view, 8)).toBeNull();
    });
    it('ignores a malformed body rather than guessing a maturity', () => {
        expect(pendingExtensionOf({ awaiting_me: [entry({ Extend: { contract: 7 } })], mine: [] }, 7)).toBeNull();
    });
});
