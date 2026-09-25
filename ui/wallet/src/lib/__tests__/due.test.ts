/**
 * A debt falling due is said once per contract per state, and the block on
 * My Contracts reads the same list. Every default whose debtor held the cash
 * in the civitas pilots was a debtor nobody had told, so the rule under test
 * is that the wallet tells them — and does not go on telling them.
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
vi.mock('../background', () => ({
    notifyIfAway: vi.fn(async () => {}),
    CHANNEL_WAITING: 'edet-waiting',
    CHANNEL_PAYMENTS: 'edet-payments',
}));

import { addMessages, init } from 'svelte-i18n';
import { CHANNEL_WAITING, notifyIfAway } from '../background';
import { contractsList, networkView } from '../node';
import {
    DUE_SOON_EPOCHS,
    dueNotices,
    dueNow,
    noticeKey,
    resetDueForTests,
    startDueNotices,
    stopDueNotices,
    unannounced,
} from '../due';
import type { ContractView } from '../api';
import en from '../../locales/en.json';

addMessages('en', en as any);
init({ fallbackLocale: 'en', initialLocale: 'en' });

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

beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    contractsList.set([]);
    networkView.set(null);
    resetDueForTests();
});
afterEach(() => {
    stopDueNotices();
    resetDueForTests();
});

describe('what falls due', () => {
    it('is my debts within the window, past-due ones first, and nothing of anybody else', () => {
        const list = [
            contract(1, { maturity_epoch: 33 }), // in 3 days: soon
            contract(2, { maturity_epoch: 34 }), // in 4: not yet
            contract(3, { maturity_epoch: 30 }), // today
            contract(4, { maturity_epoch: 28 }), // the clock passed it, sweep not yet
            contract(5, { status: 'expired', maturity_epoch: 20 }),
            contract(6, { debtor: 2, creditor: 1, maturity_epoch: 30 }), // owed to me
            contract(7, { status: 'settled', maturity_epoch: 30 }),
            contract(8, { outstanding: 0, maturity_epoch: 30 }),
        ];
        const got = dueNotices(list, 1, 30);
        expect(got.map((n) => [n.contract.id, n.state, n.left])).toEqual([
            [5, 'past', -10],
            [4, 'past', -2],
            [3, 'soon', 0],
            [1, 'soon', 3],
        ]);
        expect(DUE_SOON_EPOCHS).toBe(3);
        expect(dueNotices(list, null, 30)).toEqual([]);
    });

    it('is announced once per contract per state', () => {
        const soon = { contract: contract(1), state: 'soon' as const, left: 2 };
        const past = { contract: contract(1), state: 'past' as const, left: -1 };
        const announced = new Set<string>([noticeKey(soon)]);
        // Falling due was said; past due is a different sentence and is said.
        expect(unannounced([soon, past], announced).map(noticeKey)).toEqual(['1:past']);
    });
});

describe('the notice on the device', () => {
    it('speaks once, on the waiting channel, and fills the block on My Contracts', () => {
        startDueNotices();
        networkView.set({ epoch: 30 } as any);
        contractsList.set([contract(1, { maturity_epoch: 31 })]);
        expect(notifyIfAway).toHaveBeenCalledOnce();
        const [title, body, channel] = vi.mocked(notifyIfAway).mock.calls[0];
        expect(title).toBe('A debt of yours falls due');
        expect(body).toContain('#1');
        expect(channel).toBe(CHANNEL_WAITING);
        expect(get(dueNow).map((n) => n.contract.id)).toEqual([1]);

        // Another poll, same state: nothing more.
        contractsList.set([contract(1, { maturity_epoch: 31 })]);
        expect(notifyIfAway).toHaveBeenCalledOnce();

        // The clock passes it: the second sentence, once.
        networkView.set({ epoch: 32 } as any);
        expect(notifyIfAway).toHaveBeenCalledTimes(2);
        expect(vi.mocked(notifyIfAway).mock.calls[1][0]).toBe('A debt of yours is past due');
        networkView.set({ epoch: 33 } as any);
        expect(notifyIfAway).toHaveBeenCalledTimes(2);
    });

    it('does not re-announce after a restart, and forgets a contract once it is paid', () => {
        startDueNotices();
        networkView.set({ epoch: 30 } as any);
        contractsList.set([contract(1, { maturity_epoch: 31 })]);
        expect(notifyIfAway).toHaveBeenCalledOnce();
        stopDueNotices();
        resetDueForTests();
        // A fresh start reads what was announced from storage.
        startDueNotices();
        contractsList.set([contract(1, { maturity_epoch: 31 })]);
        expect(notifyIfAway).toHaveBeenCalledOnce();
        // Paid: the block empties.
        contractsList.set([contract(1, { maturity_epoch: 31, status: 'settled' })]);
        expect(get(dueNow)).toEqual([]);
    });
});
