/** How another member has dealt with me, read off my own contracts only. */
import { describe, expect, it } from 'vitest';
import { dealingsWith, hasDealings } from '../dealings';
import type { ContractView } from '../api';

const c = (id: number, debtor: number, creditor: number, status: ContractView['status']): ContractView => ({
    id,
    debtor,
    creditor,
    outstanding: status === 'settled' || status === 'cured' ? 0 : 10,
    original: 10,
    status,
    maturity_epoch: 30,
    created_epoch: 0,
    accepted_epoch: 0,
    insured: false,
});

describe('dealings with a member', () => {
    it('counts what they paid me, what is past due to me, what stands open, and the same the other way', () => {
        const list = [
            c(1, 2, 1, 'settled'),
            c(2, 2, 1, 'cured'),
            c(3, 2, 1, 'expired'),
            c(4, 2, 1, 'active'),
            c(5, 1, 2, 'settled'),
            c(6, 1, 2, 'active'),
            c(7, 1, 2, 'expired'),
            c(8, 2, 1, 'transferred'),
            // Somebody else's, and a debt between two others: never counted.
            c(9, 3, 1, 'settled'),
            c(10, 2, 3, 'expired'),
        ];
        const d = dealingsWith(list, 1, 2);
        expect(d).toEqual({ paidMe: 2, pastDueToMe: 1, owedToMeOpen: 1, iPaid: 1, iOweOpen: 2 });
        expect(hasDealings(d)).toBe(true);
    });

    it('says so when there is nothing', () => {
        const d = dealingsWith([c(1, 3, 1, 'settled')], 1, 2);
        expect(hasDealings(d)).toBe(false);
    });
});
