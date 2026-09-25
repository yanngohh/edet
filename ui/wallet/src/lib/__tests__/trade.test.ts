/**
 * Which party a trade's warning is about — the client-copy audit.
 *
 * The purchase page warned on the SELLER's capacity, and on a purchase the
 * seller is the creditor: insurance is decided by the DEBTOR's capacity, and an
 * uninsured loss falls on the creditor. So the page read the wrong party for
 * the question and then attributed the risk to the wrong member.
 */

import { describe, expect, it } from 'vitest';

import { asMember, asKey } from '../api';
import { counterpartyIsUnbacked, insuranceOutlook, tradeParties } from '../trade';

const DUST = 0.01;

describe('who carries an obligation that the community will not', () => {
    it('is insured when the debtor\'s capacity carries it — buying', () => {
        // The member is the debtor here, so it is THEIR capacity that decides.
        expect(insuranceOutlook('buy', 300, 100)).toBe('insured');
        expect(insuranceOutlook('buy', 100, 100)).toBe('insured');
    });

    it('is insured when the debtor\'s capacity carries it — selling', () => {
        // The counterparty is the debtor here, so it is theirs.
        expect(insuranceOutlook('sell', 300, 100)).toBe('insured');
    });

    it('puts an uninsured purchase on the SELLER, who is the creditor', () => {
        // The member is buying beyond their own capacity. The obligation is
        // booked anyway — capacity bounds what the community underwrites, not
        // what a member may risk — and the party at risk is the seller.
        expect(insuranceOutlook('buy', 50, 100)).toBe('uninsured-theirs');
    });

    it('puts an uninsured sale on the MEMBER, who is the creditor', () => {
        expect(insuranceOutlook('sell', 50, 100)).toBe('uninsured-mine');
    });

    it('is uninsured past the insured horizon however much capacity carries it', () => {
        // Capacity carries the amount; the TERM is what the community will
        // not stand behind, and the copy names the term rather than the
        // standing.
        expect(insuranceOutlook('buy', 300, 100, 366, 365)).toBe('beyond-horizon-theirs');
        expect(insuranceOutlook('sell', 300, 100, 366, 365)).toBe('beyond-horizon-mine');
        expect(insuranceOutlook('buy', 300, 100, 365, 365)).toBe('insured');
    });

    it('names the capacity, not the horizon, when both fall short', () => {
        // A shorter term would not insure this one, so the horizon copy would
        // promise what the ledger refuses.
        expect(insuranceOutlook('buy', 50, 100, 366, 365)).toBe('uninsured-theirs');
        expect(insuranceOutlook('sell', 50, 100, 366, 365)).toBe('uninsured-mine');
    });

    it('does not read a horizon it was not given', () => {
        expect(insuranceOutlook('buy', 300, 100, 10_000)).toBe('insured');
    });

    it('says nothing before there is an amount or a known capacity', () => {
        expect(insuranceOutlook('buy', undefined, 100)).toBe('unknown');
        expect(insuranceOutlook('sell', 300, 0)).toBe('unknown');
        expect(insuranceOutlook('sell', 300, -5)).toBe('unknown');
    });

    it('does not consult the counterparty on a purchase at all', () => {
        // The whole defect in one case: a seller with nothing behind them
        // changes nothing about whether the buyer's obligation is insured.
        expect(insuranceOutlook('buy', 300, 100)).toBe('insured');
    });
});

describe('whether a counterparty has anything behind them', () => {
    it('is a separate question, and only about arbitration', () => {
        expect(counterpartyIsUnbacked(0, DUST)).toBe(true);
        expect(counterpartyIsUnbacked(DUST, DUST)).toBe(true);
        expect(counterpartyIsUnbacked(DUST * 2, DUST)).toBe(false);
    });

    it('is unknown rather than true before their row has arrived', () => {
        expect(counterpartyIsUnbacked(undefined, DUST)).toBe(false);
    });
});

describe('which party is which', () => {
    const me = asMember(1);
    const them = asMember(2);

    it('makes the member the debtor when they bought', () => {
        const p = tradeParties('buy', me, them);
        expect(p.debtor).toEqual(me);
        expect(p.creditor).toEqual(them);
        // And the same two parties under the sale mechanism's names.
        expect(p.buyer).toEqual(p.debtor);
        expect(p.seller).toEqual(p.creditor);
    });

    it('makes the counterparty the debtor when the member sold', () => {
        const p = tradeParties('sell', me, them);
        expect(p.debtor).toEqual(them);
        expect(p.creditor).toEqual(me);
        expect(p.buyer).toEqual(p.debtor);
        expect(p.seller).toEqual(p.creditor);
    });

    it('carries a counterparty named by KEY through either direction', () => {
        // A party with no member id is named by key on whichever side it
        // stands: the newcomer a member buys from, or the seller a keyed
        // buyer names when it cannot look the id up. The trade is what seats
        // the key's row.
        const key = asKey(new Array(32).fill(7));
        expect(tradeParties('sell', me, key).debtor).toEqual(key);
        expect(tradeParties('buy', me, key).creditor).toEqual(key);
    });

    it('never puts the same party on both sides', () => {
        for (const d of ['buy', 'sell'] as const) {
            const p = tradeParties(d, me, them);
            expect(p.debtor).not.toEqual(p.creditor);
        }
    });
});
