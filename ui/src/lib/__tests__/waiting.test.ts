/**
 * What the wallet says when it CANNOT decide.
 *
 * Background mode's promise is that a device left running answers while
 * nobody is looking. The acceptance rule keeps half of it and announces what
 * it signed; the other half is everything the rule was not allowed to touch —
 * a request in the hold band, a key rotation, a support approval that raises
 * no pool entry at all — and a wallet that goes silent there has quietly
 * narrowed the promise to the easy cases.
 *
 * Three ways this can be wrong, and each has a probe: announcing a request
 * the rule is about to sign (so the member is told twice, contradictorily),
 * announcing the same request on every poll for as long as it waits, and
 * announcing a support count that was already owed when the app started.
 */
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

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

// The constant is part of the contract, not incidental: this module posts to
// the WAITING channel and nothing else, and a mock that omits it hides that.
vi.mock('../background', () => ({
    notifyIfAway: vi.fn(async () => {}),
    CHANNEL_WAITING: 'edet-waiting',
    CHANNEL_PAYMENTS: 'edet-payments',
}));

import { addMessages, init } from 'svelte-i18n';
import { CHANNEL_PAYMENTS, CHANNEL_WAITING, notifyIfAway } from '../background';
import { myMember, pendingView } from '../node';
import { resetWaitingForTests, startWaitingNotices, stopWaitingNotices } from '../waiting';
import type { PendingEntryView } from '../api';

import en from '../../locales/en.json';
addMessages('en', en as any);
init({ fallbackLocale: 'en', initialLocale: 'en' });

const SALE = { Sale: { seller: { Member: 1 }, buyer: { Member: 2 }, amount: 30, maturity_epochs: 30 } };

const entry = (digest: string): PendingEntryView =>
    ({
        digest,
        tx: SALE,
        nonce: Array(16).fill(0),
        not_after_epoch: 30,
        required: [{ Member: 1 }, { Member: 2 }],
        min_sigs: 2,
        signed_by: [{ Member: 2 }],
        initiator: 2,
        created_secs: 0,
    }) as PendingEntryView;

const poll = (...digests: string[]): void =>
    pendingView.set({ awaiting_me: digests.map(entry), mine: [] } as any);

beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    pendingView.set(null);
    myMember.set(null);
    resetWaitingForTests();
});

afterEach(() => {
    stopWaitingNotices();
    resetWaitingForTests();
});

describe('a request waiting for a person', () => {
    /**
     * **Not on the poll it first appears on.** The acceptance rule sweeps the
     * same store update, so a request it is allowed to sign is gone before the
     * next one; announcing immediately would tell the member a purchase needs
     * them and then, a second later, that it was signed for them.
     */
    it('says nothing until the rule has had its pass', () => {
        startWaitingNotices();
        poll('aa');
        expect(notifyIfAway).not.toHaveBeenCalled();

        poll('aa');
        expect(notifyIfAway).toHaveBeenCalledOnce();
        const [, body, channel] = vi.mocked(notifyIfAway).mock.calls[0];
        // The summary the Requests page shows, not "you have a notification".
        expect(body).toContain('30');
        // On the channel a member can silence apart from their receipts —
        // this one is the kind they cannot afford to silence.
        expect(channel).toBe(CHANNEL_WAITING);
        expect(channel).not.toBe(CHANNEL_PAYMENTS);
    });

    /** Once, however long it waits. */
    it('announces a request once and not once per poll', () => {
        startWaitingNotices();
        poll('aa');
        poll('aa');
        poll('aa');
        poll('aa');
        expect(notifyIfAway).toHaveBeenCalledOnce();
    });

    it('announces each new request', () => {
        startWaitingNotices();
        poll('aa');
        poll('aa');
        poll('aa', 'bb');
        poll('aa', 'bb');
        expect(notifyIfAway).toHaveBeenCalledTimes(2);
    });

    /**
     * A request that is decided and comes back — the same digest cannot, but
     * the bookkeeping must not grow for the life of the device either. What is
     * gone from the pool is forgotten.
     */
    it('forgets what has left the pool', () => {
        startWaitingNotices();
        poll('aa');
        poll('aa');
        poll();
        expect(JSON.parse(window.localStorage.getItem('edet-announced') ?? '{}').ids).toEqual([]);
    });
});

describe('a support approval', () => {
    const supporters = (waiting: number) =>
        myMember.set({
            id: 1,
            supporters: [
                ...Array.from({ length: waiting }, (_, i) => ({ member: 10 + i, weight: 1, approved: false })),
                { member: 99, weight: 1, approved: true },
            ],
        } as any);

    /**
     * **The first reading is a baseline, not news.** A count owed since last
     * week is not an event because this device has just started reading it,
     * and the member is looking at the app when it does.
     */
    it('does not announce what was already owed at startup', () => {
        supporters(2);
        startWaitingNotices();
        expect(notifyIfAway).not.toHaveBeenCalled();
    });

    it('announces a rise, and stays quiet on a fall', () => {
        supporters(1);
        startWaitingNotices();
        vi.mocked(notifyIfAway).mockClear();

        supporters(3);
        expect(notifyIfAway).toHaveBeenCalledOnce();
        expect(vi.mocked(notifyIfAway).mock.calls[0][2]).toBe(CHANNEL_WAITING);

        vi.mocked(notifyIfAway).mockClear();
        supporters(1);
        expect(notifyIfAway).not.toHaveBeenCalled();
    });
});
