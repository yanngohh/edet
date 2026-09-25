/**
 * Pay at maturity: the debtor's half of a settlement, opened the day before,
 * once, and never while one already waits in the pool.
 */
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('../actors', async () => {
    const { writable } = await import('svelte/store');
    return {
        currentActorId: writable<number | null>(1),
        nicknames: writable<Record<number, string>>({}),
        holdsSeed: (id: number) => id === 1,
        heldSeed: () => new Uint8Array(32),
        setNickname: () => {},
    };
});
vi.mock('../submit', () => ({ send: vi.fn(async () => true) }));

import { send } from '../submit';
import { contractsList, firstLoadDone, networkView, nodeUp, pendingView } from '../node';
import {
    autopay,
    resetAutopayForTests,
    setAutopay,
    settlementPending,
    shouldPayNow,
    startAutopay,
    stopAutopay,
} from '../autopay';
import type { ContractView } from '../api';

const contract = (id: number, over: Partial<ContractView> = {}): ContractView => ({
    id,
    debtor: 1,
    creditor: 2,
    outstanding: 40,
    original: 40,
    status: 'active',
    maturity_epoch: 30,
    created_epoch: 0,
    accepted_epoch: 0,
    insured: false,
    ...over,
});

const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    contractsList.set([]);
    networkView.set(null);
    pendingView.set(null);
    firstLoadDone.set(true);
    nodeUp.set(true);
    resetAutopayForTests();
});
afterEach(() => {
    stopAutopay();
    resetAutopayForTests();
});

describe('the scheduling rule', () => {
    it('fires the day before maturity and on the due day, never earlier or later', () => {
        const c = contract(1, { maturity_epoch: 30 });
        expect(shouldPayNow(c, 28)).toBe(false);
        expect(shouldPayNow(c, 29)).toBe(true);
        expect(shouldPayNow(c, 30)).toBe(true);
        expect(shouldPayNow(c, 31)).toBe(false);
        expect(shouldPayNow(contract(2, { status: 'expired' }), 29)).toBe(false);
        expect(shouldPayNow(contract(3, { outstanding: 0 }), 29)).toBe(false);
    });

    it('sees a settlement already waiting, mine or theirs', () => {
        const entry = (contract: number) => ({ tx: { Settle: { contract, amount: 40 } } }) as any;
        expect(settlementPending({ awaiting_me: [], mine: [entry(1)] }, 1)).toBe(true);
        expect(settlementPending({ awaiting_me: [entry(1)], mine: [] }, 1)).toBe(true);
        expect(settlementPending({ awaiting_me: [], mine: [entry(2)] }, 1)).toBe(false);
        expect(settlementPending(null, 1)).toBe(false);
    });
});

describe('the instruction on the device', () => {
    it('opens one settlement for an armed debt, once per epoch, and not for an unarmed one', async () => {
        startAutopay();
        setAutopay(1, true);
        expect(get(autopay).has(1)).toBe(true);
        networkView.set({ epoch: 29 } as any);
        contractsList.set([contract(1), contract(2)]);
        await flush();
        expect(send).toHaveBeenCalledOnce();
        expect(vi.mocked(send).mock.calls[0][0].tx).toEqual({ Settle: { contract: 1, amount: 40 } });
        // The same epoch again: nothing more.
        contractsList.set([contract(1), contract(2)]);
        await flush();
        expect(send).toHaveBeenCalledOnce();
    });

    it('stays quiet while a settlement of that debt is already in the pool', async () => {
        startAutopay();
        setAutopay(1, true);
        pendingView.set({ awaiting_me: [], mine: [{ tx: { Settle: { contract: 1, amount: 40 } } } as any] });
        networkView.set({ epoch: 29 } as any);
        contractsList.set([contract(1)]);
        await flush();
        expect(send).not.toHaveBeenCalled();
    });

    it('survives a restart as a device preference', () => {
        startAutopay();
        setAutopay(7, true);
        stopAutopay();
        resetAutopayForTests();
        startAutopay();
        expect(get(autopay).has(7)).toBe(true);
        setAutopay(7, false);
        expect(get(autopay).has(7)).toBe(false);
    });
});
