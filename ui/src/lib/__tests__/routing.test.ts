import { describe, expect, it } from 'vitest';

import { decideRoute, declineMessage } from '../submit';
import { txSummary } from '../txSummary';
import { bytesToHex } from '../crypto';

describe('decideRoute', () => {
    it('goes direct when this device holds enough required seeds', () => {
        // Unilateral action, my own seed held.
        expect(decideRoute(1, 1, 1)).toBe('direct');
        // Permissionless crank (MarkExpired): nothing required.
        expect(decideRoute(0, 0, 0)).toBe('direct');
    });

    it('parks in the pending pool when counterparties must sign', () => {
        // Bilateral contract: I hold only my side.
        expect(decideRoute(2, 2, 1)).toBe('pending');
        // Guardian rotation, threshold 2 of 3, I am one guardian.
        expect(decideRoute(3, 2, 1)).toBe('pending');
    });

    it('refuses when no required signature can be produced here', () => {
        expect(decideRoute(2, 2, 0)).toBe('unsignable');
    });
});

describe('declineMessage', () => {
    it('is domain-separated from the transaction digest', () => {
        const digest = 'ab'.repeat(32);
        const msg = declineMessage(digest);
        expect(msg).toHaveLength(32);
        expect(bytesToHex(msg)).not.toBe(digest);
        // Deterministic.
        expect(bytesToHex(declineMessage(digest))).toBe(bytesToHex(msg));
    });
});

describe('txSummary', () => {
    const deps = {
        t: (_k: string, o?: { default?: string }) => o?.default ?? _k,
        nameOf: (id: number | null | undefined) => `M${id}`,
        fmt: (n: number) => n.toFixed(2),
        // An epoch is an absolute day, so the summary asks for the date rather
        // than printing a number nobody can place (`lib/epoch.ts`). Named here
        // so the assertion below reads what it is.
        dateOf: (epoch: number) => `D${epoch}`,
    };

    it('renders a bilateral contract in plain language', () => {
        const s = txSummary({ Accept: { debtor: 0, creditor: 1, amount: 30, maturity_epochs: 30, arb: null } }, deps);
        expect(s).toContain('M0');
        expect(s).toContain('M1');
        expect(s).toContain('30.00');
    });

    it('renders a guardian rotation', () => {
        const s = txSummary({ RotateRequest: { member: 4, new_keys: [[1]] } }, deps);
        expect(s).toContain('M4');
        expect(s.toLowerCase()).toContain('key');
    });

    it('falls back to the kind name for unknown transactions', () => {
        expect(txSummary({ SomethingNew: {} }, deps)).toBe('SomethingNew');
    });
});
