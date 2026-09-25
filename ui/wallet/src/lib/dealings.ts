/**
 * **How another member has dealt with me on the ledger**, read off my own
 * contracts and nothing else.
 *
 * The community page scores a member from what the community has put behind
 * them; it says nothing about what they have done with ME, which is the one
 * fact a person actually weighs before trusting somebody again. Everything
 * here comes from contracts this member is a party to — what the node already
 * serves them — so no debt between two other people is ever read.
 */

import type { ContractView } from './api';

export interface Dealings {
    /** Debts of theirs to me that they settled or cured. */
    paidMe: number;
    /** Debts of theirs to me past due right now. */
    pastDueToMe: number;
    /** Debts of theirs to me still open and not yet due. */
    owedToMeOpen: number;
    /** Debts of mine to them that I settled or cured. */
    iPaid: number;
    /** Debts of mine to them still open, due or not. */
    iOweOpen: number;
}

export function dealingsWith(contracts: ContractView[], me: number, them: number): Dealings {
    const d: Dealings = { paidMe: 0, pastDueToMe: 0, owedToMeOpen: 0, iPaid: 0, iOweOpen: 0 };
    for (const c of contracts) {
        const theyOweMe = c.debtor === them && c.creditor === me;
        const iOweThem = c.debtor === me && c.creditor === them;
        if (!theyOweMe && !iOweThem) continue;
        const paid = c.status === 'settled' || c.status === 'cured';
        if (theyOweMe) {
            if (paid) d.paidMe += 1;
            else if (c.status === 'expired') d.pastDueToMe += 1;
            else if (c.status === 'active') d.owedToMeOpen += 1;
        } else {
            if (paid) d.iPaid += 1;
            else if (c.status === 'active' || c.status === 'expired') d.iOweOpen += 1;
        }
    }
    return d;
}

/** Whether there is anything to say at all. */
export function hasDealings(d: Dealings): boolean {
    return d.paidMe + d.pastDueToMe + d.owedToMeOpen + d.iPaid + d.iOweOpen > 0;
}
