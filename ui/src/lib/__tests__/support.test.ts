import { describe, expect, it } from 'vitest';

import type { ContractView, MemberDetail, SupportEdgeView } from '../api';
import { drainReach, reachBySupporter, supportPrompt } from '../support';

const owe = (id: number, outstanding: number, status: ContractView['status'] = 'active'): ContractView => ({
    id,
    outstanding,
    original: outstanding,
    status,
    maturity_epoch: 100,
    created_epoch: 1, accepted_epoch: 1,
    insured: true,
});

describe('drainReach — the ledger rule the page has to mirror', () => {
    it('takes the oldest obligations first, exactly as the cascade walks them', () => {
        const r = drainReach(150, [owe(3, 100), owe(1, 80), owe(2, 60)]);
        // 80 (id 1) + 60 (id 2) + 10 of id 3.
        expect(r.contracts).toEqual([1, 2, 3]);
        expect(r.clears).toBeCloseTo(150, 9);
        expect(r.routed).toBe(true);
    });

    it('stops at the cap rather than at the book', () => {
        const r = drainReach(50, [owe(1, 400)]);
        expect(r.clears).toBeCloseTo(50, 9);
        expect(r.contracts).toEqual([1]);
    });

    it('stops at the book rather than at the cap', () => {
        const r = drainReach(500, [owe(1, 40), owe(2, 30)]);
        expect(r.clears).toBeCloseTo(70, 9);
    });

    // A defaulted claim is not routed: `clear_member_debts` filters on Active,
    // so the cascade cannot rescue a member who has already fallen due. A page
    // that counted expired rows here would promise exactly the rescue the
    // ledger refuses.
    it('never counts an expired obligation', () => {
        const r = drainReach(500, [owe(1, 100, 'expired'), owe(2, 40)]);
        expect(r.contracts).toEqual([2]);
        expect(r.clears).toBeCloseTo(40, 9);
    });

    it('ignores rows that are already closed', () => {
        for (const status of ['settled', 'cured', 'transferred'] as ContractView['status'][]) {
            expect(drainReach(500, [owe(1, 0, status)]).routed).toBe(false);
        }
    });

    // Zero is the ledger's answer for a pair with no settled history — there is
    // deliberately no floor on the drain cap — and `null`/`undefined` is an
    // unauthenticated read. Neither is a route, and neither may report one.
    it('reports no reach for an edge that carries nothing, and for one it cannot see', () => {
        expect(drainReach(0, [owe(1, 100)])).toEqual({ routed: false, visible: true, clears: 0, contracts: [] });
        expect(drainReach(null, [owe(1, 100)]).routed).toBe(false);
        expect(drainReach(undefined, [owe(1, 100)]).routed).toBe(false);
        expect(drainReach(Number.NaN, [owe(1, 100)]).routed).toBe(false);
    });
});

describe('supportPrompt — four different facts, never merged', () => {
    const owes = [owe(1, 100)];
    it('separates "no route" from "nothing to clear"', () => {
        expect(supportPrompt(drainReach(0, owes), 0)).toBe('no-route');
        expect(supportPrompt(drainReach(200, []), 200)).toBe('nothing-to-clear');
        expect(supportPrompt(drainReach(200, owes), 200)).toBe('trade-off');
    });

    it('says "cannot see" rather than "nothing" when the book was not served', () => {
        const hidden = reachBySupporter(detail({ debt: 250, owes: [], supporters: [edge(7, 90)] })).get(7)!;
        expect(hidden.visible).toBe(false);
        expect(supportPrompt(hidden, 90)).toBe('unknown');
    });
});

const edge = (member: number, drain_cap: number | null): SupportEdgeView => ({
    member,
    weight: 1,
    approved: false,
    drain_cap,
});

const detail = (p: Partial<MemberDetail>): MemberDetail =>
    ({
        id: 1,
        address: 'a',
        status: 'active',
        capacity: 0,
        conferrable: 0,
        debt: 0,
        joined_epoch: 0,
        is_validator: false,
        owes: [],
        owed: [],
        ...p,
    }) as MemberDetail;

describe('reachBySupporter', () => {
    it('answers per supporter, and survives a view that served neither list', () => {
        const m = reachBySupporter(detail({ debt: 250, owes: [owe(1, 250)], supporters: [edge(7, 90), edge(8, 0)] }));
        expect(m.get(7)?.clears).toBeCloseTo(90, 9);
        expect(m.get(8)?.routed).toBe(false);
        expect(reachBySupporter(null).size).toBe(0);
        expect(reachBySupporter(detail({})).size).toBe(0);
    });

    // The node filters `owes` to the contracts the READER is party to, so
    // an empty array from anyone but the member themselves is "not shown", not
    // "none". `debt` is served to every reader and is the discriminator; a page
    // that missed this would print a reassurance the ledger never gave.
    // A closed book can leave a residue in `debt` that no row accounts for —
    // the close forgives dust and the cache follows the book — and reading that
    // as "hidden" would print "not visible here" to a member who owes nothing.
    it('does not read a dust residue as a hidden book', () => {
        const r = reachBySupporter(detail({ debt: 1e-9, owes: [], supporters: [edge(7, 90)] }), 1e-6).get(7)!;
        expect(r.visible).toBe(true);
        expect(supportPrompt(r, 90)).toBe('nothing-to-clear');
        // ...and a real balance the reader cannot see is still hidden.
        const hidden = reachBySupporter(detail({ debt: 250, owes: [], supporters: [edge(7, 90)] }), 1e-6).get(7)!;
        expect(hidden.visible).toBe(false);
    });

    it('treats an empty book with debt outstanding as hidden, not as empty', () => {
        const hidden = reachBySupporter(detail({ debt: 250, owes: [], supporters: [edge(7, 90)] })).get(7)!;
        expect(hidden).toEqual({ routed: false, visible: false, clears: 0, contracts: [] });

        // A member who genuinely owes nothing is a different answer.
        const clean = reachBySupporter(detail({ debt: 0, owes: [], supporters: [edge(7, 90)] })).get(7)!;
        expect(clean.visible).toBe(true);
        expect(clean.routed).toBe(false);
    });
});
