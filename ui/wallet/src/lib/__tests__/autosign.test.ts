/**
 * The acceptance engine's guard chain. Every case here is one where an
 * automatic signature would be wrong: these assert that nothing is signed
 * for the member outside the narrow lane the rule is allowed to act in.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
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

// The notification is the one thing the engine does that leaves this device,
// so it is observed rather than let through: `background.ts` has its own gate
// for what it puts on screen (`background.test.ts`).
vi.mock('../background', () => ({ notifySigned: vi.fn(async () => {}) }));

vi.mock('../submit', () => ({
    checkPending: vi.fn(async () => ({ ok: true })),
    signPending: vi.fn(async () => true),
    declinePending: vi.fn(async () => true),
    rejectionMessage: (code: string) => code,
}));

import * as submit from '../submit';
import { notifySigned } from '../background';
import { currentActorId } from '../actors';
import { membersList, paramsView } from '../node';
import { autoDecisions, processEntry, startAcceptance, stopAcceptance } from '../autosign';
import { acceptancePolicy, DEFAULT_POLICY, setPolicy } from '../policy';
import { resetPricing, setPricing } from '../pricing';
import type { MemberSummary, PendingEntryView } from '../api';

const member = (id: number, over: Partial<MemberSummary> = {}): MemberSummary => ({
    id,
    address: `0x${String(id).repeat(40).slice(0, 40)}`,
    keys: ['aa'.repeat(32)],
    status: 'active',
    // A buyer the community has put real backing behind. Capacity is what
    // confidence now reads, and the scale is `K·V_base` — so "trusted" has to
    // mean several multiples of the denomination rather than exactly one, or
    // this fixture scores mid-band and every assertion below reads as a hold.
    capacity: 5000,
    debt: 50,
    open_default: 0,
    d_in: 80,
    d_out: 80,
    ...over,
});

const entry = (tx: Record<string, any>): PendingEntryView =>
    ({
        digest: 'ab'.repeat(32),
        tx,
        nonce: Array(16).fill(0),
        not_after_epoch: 30,
        required: [{ Member: 1 }, { Member: 2 }],
        min_sigs: 2,
        signed_by: [{ Member: 2 }],
        initiator: 2,
        created_secs: 0,
    }) as PendingEntryView;

// A purchase where member 2 buys from me (member 1): I grant the credit.
const SALE = { Sale: { seller: { Member: 1 }, buyer: { Member: 2 }, amount: 30, maturity_epochs: 30 } };

beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(submit.checkPending).mockResolvedValue({ ok: true });
    vi.mocked(submit.signPending).mockResolvedValue(true);
    vi.mocked(submit.declinePending).mockResolvedValue(true);
    currentActorId.set(1);
    membersList.set([member(1), member(2)]);
    paramsView.set({ governed: [{ key: 'RiskK', value: 0.75, min: 0.25, max: 5, last_amend_epoch: null }] } as any);
    autoDecisions.set([]);
    window.localStorage.clear();
    // The engine is OFF by default and signs nothing without an amount
    // ceiling (`policy.ts`), so these tests opt in explicitly. The two
    // defaults have gates of their own in `risk.test.ts` and below.
    setPolicy({ ...DEFAULT_POLICY, auto: true, maxAmount: 1_000 });
    resetPricing();
});

describe('what it signs', () => {
    it('signs a purchase from a trusted buyer and records the decision', async () => {
        expect(await processEntry(entry(SALE))).toBe('signed');
        expect(submit.signPending).toHaveBeenCalledOnce();
        const [d] = get(autoDecisions);
        expect(d.band).toBe('accept');
        expect(d.debtor).toBe(2);
        expect(d.risk).toBeLessThanOrEqual(get(acceptancePolicy).accept);
    });

    it('declines a buyer carrying an open default', async () => {
        membersList.set([member(1), member(2, { capacity: 0, open_default: 40 })]);
        expect(await processEntry(entry(SALE))).toBe('declined');
        expect(submit.declinePending).toHaveBeenCalledOnce();
        expect(submit.signPending).not.toHaveBeenCalled();
    });
});

/**
 * **A member's own price moves the band and reaches nothing else.**
 *
 * The subjective rule is client-side and device-only, and every guard the
 * engine already applies stands in front of it: the debtor side is never
 * signed, the amount ceiling still holds, a cold start is still held for a
 * human. A rule that says "I trust this counterparty more than the ledger
 * does" therefore cannot buy anything past those.
 */
describe('a subjective price', () => {
    it('moves a signed request into the hold band', async () => {
        setPricing({ personalK: null, adjustments: { 2: 0.5 } });
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('cannot make the debtor side auto-sign', async () => {
        // The mirror of the sale above: member 2 sells and I buy, so the
        // credit is granted TO me. Nothing prices that into an automatic
        // signature.
        const buying = { Sale: { seller: { Member: 2 }, buyer: { Member: 1 }, amount: 30, maturity_epochs: 30 } };
        setPricing({ personalK: null, adjustments: { 2: -0.5 } });
        expect(await processEntry(entry(buying))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('cannot pass the amount ceiling', async () => {
        setPolicy({ ...DEFAULT_POLICY, auto: true, maxAmount: 10 });
        setPricing({ personalK: null, adjustments: { 2: -0.5 } });
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('cannot turn a held cold start into an accept', async () => {
        // Nobody has backed member 2 yet, so the score is 1.0 — "nothing is
        // known", which the engine holds rather than declines. An offset of
        // −0.5 leaves 0.5, still above the accept threshold, and even a rule
        // that reached the threshold would meet the cold-start guard first.
        membersList.set([member(1), member(2, { capacity: 0, debt: 0, open_default: 0 })]);
        setPricing({ personalK: null, adjustments: { 2: -0.5 } });
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
        expect(submit.declinePending).not.toHaveBeenCalled();
    });

    it('is absent by default, so a wallet that has said nothing decides on the ledger', async () => {
        expect(await processEntry(entry(SALE))).toBe('signed');
        expect(submit.signPending).toHaveBeenCalledOnce();
    });
});

describe('what it refuses to touch', () => {
    it('holds a request above the amount ceiling however well it scores', async () => {
        setPolicy({ ...DEFAULT_POLICY, auto: true, maxAmount: 10 });
        // The same trusted buyer whose 30-unit purchase is signed above.
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
        expect(submit.declinePending).not.toHaveBeenCalled();
    });

    it('holds a request whose amount it cannot read', async () => {
        setPolicy({ ...DEFAULT_POLICY, auto: true, maxAmount: 1_000 });
        expect(await processEntry(entry({ Sale: { seller: 1, buyer: 2, maturity_epochs: 30 } }))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('leaves everything alone when the rule is off', async () => {
        setPolicy({ ...DEFAULT_POLICY, auto: false });
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
        expect(submit.checkPending).not.toHaveBeenCalled();
    });

    /**
     * **A counterparty who has no account yet is never auto-signed.**
     *
     * A trade may name a party by KEY, and that trade is what seats their
     * account — so there is no row to score: no capacity, no history, no open
     * default. The engine acts only where the ledger already says something,
     * and here it says nothing at all, so the request goes to the human. That
     * is the whole reason `scoredDebtor` answers `null` for a key rather than
     * reading through to some default.
     */
    it('never auto-signs a trade with somebody who has no account yet', async () => {
        setPolicy({ ...DEFAULT_POLICY, auto: true, maxAmount: 1_000 });
        const key = { Key: Array(32).fill(7) };
        expect(
            await processEntry(entry({ Sale: { seller: { Member: 1 }, buyer: key, amount: 30, maturity_epochs: 30 } })),
        ).toBe('left');
        expect(
            await processEntry(
                entry({ Accept: { creditor: { Member: 1 }, debtor: key, amount: 30, maturity_epochs: 30 } }),
            ),
        ).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
        expect(submit.declinePending).not.toHaveBeenCalled();
    });

    it('never signs what the ledger would reject', async () => {
        vi.mocked(submit.checkPending).mockResolvedValue({ ok: false, code: 'ET-CAP-001' });
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('never signs a request that would make me the debtor', async () => {
        expect(await processEntry(entry({ Sale: { seller: 2, buyer: 1, amount: 30, maturity_epochs: 30 } }))).toBe('left');
        expect(await processEntry(entry({ Accept: { debtor: 1, creditor: 2, amount: 30, maturity_epochs: 30 } }))).toBe(
            'left',
        );
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    /**
     * **An `Accept` that carries arbitration terms is never signed by machine,
     * whatever the buyer scores.**
     *
     * The panel is the buyer's remedy against the SELLER: an award is minted
     * as an obligation from the creditor to the debtor, bounded by the cap
     * and the amount (`ArbTerms`). The ledger refuses only a panel that
     * seats a party, so a buyer may name their own accomplices with a quorum
     * of one, and a seller whose engine signs without reading the terms is
     * bound to deliver to a bench they never chose — and, having never seen
     * the request, does not deliver, at which point the panel awards the
     * buyer the amount. The engine's lane is the same purchase WITHOUT a
     * panel; the control below shows that is what the probe separates.
     */
    it('never signs an Accept that carries arbitration terms, however well the buyer scores', async () => {
        const terms = { arbiters: [3], quorum: 1, window_epochs: 30, award_cap: 30 };
        const withPanel = {
            Accept: { debtor: { Member: 2 }, creditor: { Member: 1 }, amount: 30, maturity_epochs: 30, arb: terms },
        };
        expect(await processEntry(entry(withPanel))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
        expect(submit.declinePending).not.toHaveBeenCalled();
        // Control: the same purchase from the same buyer with no panel is
        // inside the lane and is signed.
        const plain = {
            Accept: { debtor: { Member: 2 }, creditor: { Member: 1 }, amount: 30, maturity_epochs: 30, arb: null },
        };
        expect(await processEntry(entry(plain))).toBe('signed');
        expect(submit.signPending).toHaveBeenCalledOnce();
    });

    it('never signs an identity or governance request, whatever it scores', async () => {
        // Every kind here is one the alphabet actually has
        // (`crates/state/src/tx.rs`). A kind it lacks — `AdmitMember`,
        // `PromoteMember` — makes an assertion about nothing at all and
        // leaves the engine's real identity and governance surface untested.
        for (const tx of [
            { RegisterGuardians: { member: 2, guardians: [1], threshold: 1, veto_window_epochs: 30 } },
            { RotateRequest: { member: 2, new_keys: [[1]] } },
            { RotateFinalize: { member: 2 } },
            { Propose: { author: 2, kind: { ParamChange: { key: 'RiskK', value: 0.6 } } } },
            { Assent: { member: 2, proposal: 0 } },
            { DeclareSupply: { member: 2, supply: 100 } },
            { ArbAttest: { contract: 1, arbiter: 1, amount: 5 } },
        ]) {
            expect(await processEntry(entry(tx))).toBe('left');
        }
        expect(submit.signPending).not.toHaveBeenCalled();
        expect(submit.declinePending).not.toHaveBeenCalled();
    });

    it("holds a stranger's first purchase instead of declining it", async () => {
        membersList.set([member(1), member(2, { capacity: 0, debt: 0 })]);
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.declinePending).not.toHaveBeenCalled();
    });

    it('holds a counterparty the ledger view has not loaded yet', async () => {
        membersList.set([member(1)]);
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('does nothing when this device cannot sign for the acting member', async () => {
        currentActorId.set(9); // no seed held for 9
        expect(await processEntry(entry(SALE))).toBe('left');
        expect(submit.signPending).not.toHaveBeenCalled();
    });

    it('does not retry a refused signature', async () => {
        vi.mocked(submit.signPending).mockResolvedValue(false);
        expect(await processEntry(entry(SALE))).toBe('failed');
        expect(get(autoDecisions)).toHaveLength(0);
    });
});

/**
 * **What the member reads when they come back.**
 *
 * The rule decides while they are elsewhere — that is the whole of background
 * mode — so the account of what it did has to outlive the session it was
 * written in, and a notification has to reach them for the one outcome that
 * granted credit in their name. A decline granted nothing and left nothing to
 * collect: it waits in the list.
 */
describe('what it leaves behind', () => {
    it('notifies a signature, and nothing else', async () => {
        expect(await processEntry(entry(SALE))).toBe('signed');
        expect(notifySigned).toHaveBeenCalledOnce();

        vi.mocked(notifySigned).mockClear();
        membersList.set([member(1), member(2, { capacity: 0, open_default: 40 })]);
        expect(await processEntry(entry(SALE))).toBe('declined');
        expect(notifySigned).not.toHaveBeenCalled();
    });

    it('keeps the decisions across a restart, for the identity that made them', async () => {
        expect(await processEntry(entry(SALE))).toBe('signed');
        expect(get(autoDecisions)).toHaveLength(1);

        // A restart: the store is fresh and the engine reads what this device
        // wrote. Without the persisted list the member comes back from an
        // evening away to an empty page.
        autoDecisions.set([]);
        stopAcceptance();
        startAcceptance();
        expect(get(autoDecisions)).toHaveLength(1);
        expect(get(autoDecisions)[0].digest).toBe('ab'.repeat(32));
        expect(get(autoDecisions)[0].at).toBeGreaterThan(0);

        // And another identity's decisions are not this one's history.
        currentActorId.set(2);
        expect(get(autoDecisions)).toHaveLength(0);
        stopAcceptance();
    });
});
