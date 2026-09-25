/**
 * A trusted member is signed on their name up to their ceiling, and held
 * above it like anybody else; the list reaches past none of the other
 * guards.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

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
vi.mock('../background', () => ({ notifySigned: vi.fn(async () => {}) }));
vi.mock('../submit', () => ({
    checkPending: vi.fn(async () => ({ ok: true })),
    signPending: vi.fn(async () => true),
    declinePending: vi.fn(async () => true),
    rejectionMessage: (code: string) => code,
}));

import * as submit from '../submit';
import { currentActorId } from '../actors';
import { membersList, paramsView } from '../node';
import { processEntry } from '../autosign';
import { normalizePolicy, setPolicy, trustedCeiling, DEFAULT_POLICY } from '../policy';
import type { MemberSummary, PendingEntryView } from '../api';

// A buyer nobody has backed: capacity zero, so they score at the ceiling and
// the rule alone holds them for a human.
const stranger = (id: number): MemberSummary => ({
    id,
    address: `0x${String(id).repeat(40).slice(0, 40)}`,
    keys: ['aa'.repeat(32)],
    status: 'active',
    capacity: 0,
    debt: 0,
    open_default: 0,
    d_in: 0,
    d_out: 0,
});

const entry = (amount: number): PendingEntryView =>
    ({
        digest: 'ab'.repeat(32),
        tx: { Sale: { seller: { Member: 1 }, buyer: { Member: 2 }, amount, maturity_epochs: 30 } },
        nonce: Array(16).fill(0),
        not_after_epoch: 30,
        required: [{ Member: 1 }, { Member: 2 }],
        min_sigs: 2,
        signed_by: [{ Member: 2 }],
        initiator: 2,
        created_secs: 0,
    }) as PendingEntryView;

beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.clear();
    vi.mocked(submit.checkPending).mockResolvedValue({ ok: true });
    currentActorId.set(1);
    membersList.set([stranger(1), stranger(2)]);
    paramsView.set(null);
    setPolicy({ ...DEFAULT_POLICY, auto: true, maxAmount: 0 });
});

describe('a trusted member', () => {
    it('is read at the highest ceiling listed, and only when listed', () => {
        const p = normalizePolicy({ trusted: [{ member: 2, maxAmount: 50 }, { member: 2, maxAmount: 80 }] });
        expect(trustedCeiling(p, 2)).toBe(80);
        expect(trustedCeiling(p, 3)).toBeNull();
        // Malformed entries are dropped, never read as unlimited.
        expect(normalizePolicy({ trusted: [{ member: 'x', maxAmount: 5 }, { member: 2 }] } as any).trusted).toEqual([]);
        expect(normalizePolicy({}).trusted).toEqual([]);
    });

    it('is signed on their name up to the ceiling, where the score alone would hold them', async () => {
        expect(await processEntry(entry(30))).toBe('left');
        setPolicy({ ...DEFAULT_POLICY, auto: true, trusted: [{ member: 2, maxAmount: 50 }] });
        expect(await processEntry(entry(30))).toBe('signed');
        expect(submit.signPending).toHaveBeenCalledOnce();
    });

    it('is held above the ceiling like anybody else', async () => {
        setPolicy({ ...DEFAULT_POLICY, auto: true, trusted: [{ member: 2, maxAmount: 50 }] });
        expect(await processEntry(entry(51))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('does not reach past the ledger, the master switch or the debtor side', async () => {
        setPolicy({ ...DEFAULT_POLICY, auto: true, trusted: [{ member: 2, maxAmount: 50 }] });
        vi.mocked(submit.checkPending).mockResolvedValue({ ok: false, code: 'ET-BND-001' });
        expect(await processEntry(entry(30))).toBe('left');
        vi.mocked(submit.checkPending).mockResolvedValue({ ok: true });
        setPolicy({ ...DEFAULT_POLICY, auto: false, trusted: [{ member: 2, maxAmount: 50 }] });
        expect(await processEntry(entry(30))).toBe('left');
        setPolicy({ ...DEFAULT_POLICY, auto: true, trusted: [{ member: 1, maxAmount: 50 }] });
        // Me as the buyer: the debtor side is never signed, trusted or not.
        const mine = { ...entry(30), tx: { Sale: { seller: { Member: 2 }, buyer: { Member: 1 }, amount: 30, maturity_epochs: 30 } } };
        expect(await processEntry(mine as PendingEntryView)).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });
});
