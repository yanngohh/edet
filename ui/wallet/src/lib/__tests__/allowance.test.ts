/**
 * The allowance cliff and the founder's banner — the client-copy audit —
 * and the quantity underneath both.
 *
 * A wallet asking `capacity` where the ledger asks `conferrable` tells a
 * member whose backing has been withdrawn "32 of 32 free actions" in the good
 * tone, and every founding underwriter "nobody has backed you yet" while
 * carrying the community. And a wallet re-deriving the ledger's test drifts
 * from it: the gate reads that quantity GROSS of live credit, because a
 * residual reading makes the qualification a statement about how busy a
 * member's backers are — measured on the node, a full allowance to zero at 20%
 * community utilisation for a member who had borrowed nothing. So the client
 * does not re-derive it: `operation_bond.established` is the ledger's own
 * answer, and these cases are about what the wallet SAYS.
 */

import { describe, expect, it } from 'vitest';

import { allowanceState, bondNotice, hasNoStanding } from '../allowance';
import type { MemberDetail, OperationBondView, TxBondView } from '../api';

const DUST = 0.01;

function bond(over: Partial<OperationBondView> = {}): OperationBondView {
    return { encumbered: 0, headroom: 300, unit: 20, free_remaining: 32, saturated_epochs: 0, established: true, seat_reach: 300, seat_slot: false, ...over };
}

function member(over: Partial<MemberDetail> = {}): MemberDetail {
    return {
        id: 1,
        address: '0x' + '11'.repeat(20),
        keys: [],
        status: 'active',
        capacity: 300,
        debt: 0,
        conferrable: 300,
        joined_epoch: 0,
        is_validator: false,
        owes: [],
        owed: [],
        operation_bond: bond(),
        ...over,
    } as MemberDetail;
}

describe('the write allowance, as the wallet should report it', () => {
    it('is ok for a backed member with free actions left', () => {
        expect(allowanceState(member(), DUST)).toBe('ok');
    });

    it("is spent when the epoch's free actions are used and standing remains", () => {
        const m = member({ operation_bond: bond({ encumbered: 40, headroom: 260, free_remaining: 0 }) });
        expect(allowanceState(m, DUST)).toBe('spent');
    });

    it('is no-standing once the ledger says so, whatever the counter says', () => {
        // The cliff. Waiting for the next epoch does not help here and the
        // "spent" message would say it does, so the two must not collapse.
        const m = member({ capacity: 0, conferrable: 0, operation_bond: bond({ established: false, headroom: 0 }) });
        expect(allowanceState(m, DUST)).toBe('no-standing');
        const stillCounting = member({
            capacity: 0,
            conferrable: 0,
            operation_bond: bond({ established: false, headroom: 0, free_remaining: 32 }),
        });
        expect(allowanceState(stillCounting, DUST)).toBe('no-standing');
    });

    it('is NOT no-standing for a member whose backers are merely fully drawn', () => {
        // The gross reading, from the wallet's side. Their capacity is
        // zero because the community's ceiling binds — that is the cut doing
        // its job — and the community has still put 300 behind them, so the
        // one thing the wallet must not say is "nobody is backing you".
        const drawn = member({ capacity: 0, conferrable: 0, operation_bond: bond({ headroom: 300 }) });
        expect(allowanceState(drawn, DUST)).toBe('ok');
        expect(hasNoStanding(drawn, DUST)).toBe(false);
    });

    it('is defaulted while a default is open, which is not "spent"', () => {
        // A default costs the allowance, and it does not come back with the
        // epoch — it comes back with the cure. "You have used every free action
        // this epoch" would be a promise about tomorrow.
        const d = member({ open_default: 100, operation_bond: bond({ free_remaining: 0 }) });
        expect(allowanceState(d, DUST)).toBe('defaulted');
        expect(hasNoStanding(d, DUST)).toBe(false);
        expect(allowanceState(member({ open_default: DUST / 2 }), DUST)).toBe('ok');
    });

    it('says nothing at all without full access, rather than saying zero', () => {
        expect(allowanceState(null, DUST)).toBe('unknown');
        expect(allowanceState(member({ operation_bond: undefined }), DUST)).toBe('unknown');
    });
});

describe('who is told nobody is backing them', () => {
    it('a newcomer, and a member whose backing lapsed — the same message', () => {
        expect(hasNoStanding(member({ capacity: 0, conferrable: 0, operation_bond: bond({ established: false }) }), DUST)).toBe(
            true,
        );
    });

    it('NOT a founding underwriter, whose capacity is zero and standing is not', () => {
        // A cut into an underwriter draws on the OTHER underwriters, and at
        // genesis nobody has backed anybody — so `capacity` is 0 for the one
        // member who has demonstrably accepted a real liability. Keying the
        // banner on capacity told every founder they were new here; the ledger
        // answers `established` for them because a declared supply is what they
        // may confer.
        const founder = member({ capacity: 0, conferrable: 2500, supply: { declared: 2500, committed: 0 } });
        expect(hasNoStanding(founder, DUST)).toBe(false);
        expect(allowanceState(founder, DUST)).toBe('ok');
    });

    it('NOT a backed member', () => {
        expect(hasNoStanding(member(), DUST)).toBe(false);
    });

    it('NOT a suspended or exited account, which is a different sentence', () => {
        const gone = bond({ established: false, headroom: 0 });
        expect(hasNoStanding(member({ capacity: 0, status: 'suspended', operation_bond: gone }), DUST)).toBe(false);
        expect(hasNoStanding(member({ capacity: 0, status: 'exited', operation_bond: gone }), DUST)).toBe(false);
    });
});

describe('bondNotice', () => {
    const quote = (over: Partial<TxBondView> = {}): TxBondView => ({ amount: 20, release_epochs: 3, ...over });

    it('says nothing before the quote lands, and says free where the class is free', () => {
        expect(bondNotice(null, bond())).toBe('unknown');
        expect(bondNotice(quote({ amount: 0 }), bond())).toBe('always-free');
    });

    it('covers an ordinary write from the allowance', () => {
        expect(bondNotice(quote(), bond({ free_remaining: 32 }))).toBe('covered');
    });

    it('never calls a seating trade free, however much allowance is left', () => {
        // The ledger consults the allowance only where nothing is seated, so
        // "32 of 32 free actions" is true and irrelevant: this one is charged.
        expect(bondNotice(quote({ allowance_applies: false }), bond({ free_remaining: 32, headroom: 300 }))).toBe(
            'seats',
        );
    });

    it('names the binding constraint when the capacity is not there', () => {
        // The tape's own case: established, 32 free actions showing, headroom
        // 0, refused ET-BND-001. The member needs the number, not the reason.
        expect(bondNotice(quote({ allowance_applies: false }), bond({ free_remaining: 32, headroom: 0 }))).toBe(
            'short',
        );
    });

    it('keeps what it always said where the node does not answer', () => {
        // A node predating the field, or a read that is not a party: unknown
        // must not invent a warning.
        expect(bondNotice(quote(), bond({ free_remaining: 32 }))).toBe('covered');
        expect(bondNotice(quote(), undefined)).toBe('reserves');
    });

    it('falls to the plain reservation when the allowance is spent and there is room', () => {
        expect(bondNotice(quote(), bond({ free_remaining: 0, headroom: 300 }))).toBe('reserves');
        expect(bondNotice(quote(), bond({ free_remaining: 0, headroom: 5 }))).toBe('short');
    });
});
