/**
 * Who may assent — the paper's §Governance, from the client's side.
 *
 * The governance page offered an Assent button to every signed-in member while
 * only underwriters a ceremony seated may use one; the ledger refused the rest
 * with `ET-GOV-007`, translated in six locales, so the refusal explained itself
 * after the fact. **A control that is always refused is a promise the chain
 * does not keep**, which is the same shape as a view describing a mechanism the
 * chain does not run — and the test for it was already on the wire the whole
 * time, in the member's own declared supply.
 *
 * These cases are the ledger's own rules, one per refusal it can return.
 */

import { describe, expect, it } from 'vitest';

import { assentVerdict } from '../governance';
import type { MemberDetail, ProposalView } from '../api';

const DUST = 0.01;

function member(external: number | null): MemberDetail {
    return {
        id: 1,
        address: '0x' + '11'.repeat(20),
        keys: [],
        status: 'active',
        capacity: 300,
        debt: 0,
        joined_epoch: 0,
        is_validator: false,
        owes: [],
        owed: [],
        ...(external === null ? {} : { supply: { declared: external, committed: 0 } }),
    };
}

function proposal(over: Partial<ProposalView> = {}): ProposalView {
    return {
        id: 7,
        author: 2,
        assents: [],
        assented_seed: 0,
        enacted: false,
        kind: { type: 'param_change', key: 'RiskK', value: 0.6 },
        ...over,
    } as ProposalView;
}

describe('who may assent', () => {
    it('offers it to an underwriter a ceremony seated', () => {
        expect(assentVerdict(1, member(2500), proposal(), DUST)).toBe('offer');
    });

    it('refuses a member whose backing came from inside the community', () => {
        // ET-GOV-007. Their declaration is a real accepted liability and a
        // promise on a promise at the same time; the vote is the half that is
        // neither, and they have none of it.
        expect(assentVerdict(1, member(0), proposal(), DUST)).toBe('no-mandate');
    });

    it('refuses a member who is not an underwriter at all', () => {
        expect(assentVerdict(1, member(null), proposal(), DUST)).toBe('no-mandate');
    });

    it('treats a dust-sized endorsement as none', () => {
        // "Declared nothing" and "declared a rounding error" are one answer, or
        // the button appears for a member the ledger will refuse anyway.
        expect(assentVerdict(1, member(DUST), proposal(), DUST)).toBe('no-mandate');
        expect(assentVerdict(1, member(DUST * 2), proposal(), DUST)).toBe('offer');
    });

    it('refuses the author of a seed amendment their own assent', () => {
        // ET-SED-002: the kind names no beneficiary, so the author IS one, and
        // the vote would be cast with the weight the amendment enlarges.
        const own = proposal({ author: 1, kind: { type: 'seed_amendment', amount: 500 } as ProposalView['kind'] });
        expect(assentVerdict(1, member(2500), own, DUST)).toBe('own-amendment');
    });

    it('still offers a seated underwriter somebody ELSE\'s amendment', () => {
        const theirs = proposal({ author: 2, kind: { type: 'seed_amendment', amount: 500 } as ProposalView['kind'] });
        expect(assentVerdict(1, member(2500), theirs, DUST)).toBe('offer');
    });

    it('says nothing about an enacted proposal, or to a caller who is not signed in', () => {
        expect(assentVerdict(1, member(2500), proposal({ enacted: true }), DUST)).toBe('none');
        expect(assentVerdict(null, member(2500), proposal(), DUST)).toBe('none');
    });

    it('reports an assent already given rather than offering it twice', () => {
        expect(assentVerdict(1, member(2500), proposal({ assents: [1] }), DUST)).toBe('assented');
    });

    it('holds rather than refusing while the member detail has not arrived', () => {
        // A poll that has not landed is "not known yet", never "no". A refusal
        // flickering on every slow poll would be worse than either answer, and
        // it is the same rule the acceptance engine applies to a missing field.
        expect(assentVerdict(1, null, proposal(), DUST)).toBe('unknown');
    });

    it('answers about the proposal before it looks at the member', () => {
        // Ordering matters for the loading case: an enacted proposal is decided
        // whether or not this device knows who it is yet.
        expect(assentVerdict(1, null, proposal({ enacted: true }), DUST)).toBe('none');
        expect(assentVerdict(1, null, proposal({ assents: [1] }), DUST)).toBe('assented');
    });
});
