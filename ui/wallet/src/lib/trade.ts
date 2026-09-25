/**
 * What a trade is worth telling a member before they sign it, from whichever
 * side they stand on: the buyer recording a purchase, or the seller reviewing
 * one — a pool request, or a keyed buyer's code.
 *
 * Two facts, and both are about WHICH party is read.
 *
 * **Insurance is decided by the DEBTOR's capacity.** Accepting an obligation
 * computes an augmenting flow of the amount into the debtor; where the residual
 * carries it the obligation is insured, and where it does not the obligation is
 * booked uninsured rather than refused (capacity bounds what the community
 * underwrites, never what a member may risk). On a purchase the seller is the
 * creditor — a party whose standing has nothing to do with it.
 *
 * **An uninsured loss falls on the CREDITOR, alone.** So on a purchase the risk
 * being described belongs to the counterparty, and on a sale to the member.
 */

import type { Party } from './api';

/** Which side of the obligation the member is on. */
export type Direction =
    /** The member buys: they become the debtor, the counterparty the creditor. */
    | 'buy'
    /** The member sells: the counterparty becomes the debtor, the member the creditor. */
    | 'sell';

export type InsuranceOutlook =
    /** The debtor's standing is not known yet — say nothing rather than guess. */
    | 'unknown'
    /** The community's backing carries it: it reserves flow, and recourse follows. */
    | 'insured'
    /** Uninsured, and the member is the creditor: theirs to bear, alone. */
    | 'uninsured-mine'
    /** Uninsured, and the counterparty is the creditor: theirs to bear, alone. */
    | 'uninsured-theirs'
    /** Capacity carries it, but the maturity is past the insured horizon, so it
     *  books uninsured — and the member is the creditor. The remedy is the
     *  term, not the standing. */
    | 'beyond-horizon-mine'
    /** The same, with the counterparty as the creditor. */
    | 'beyond-horizon-theirs';

/**
 * @param direction which side the member is on
 * @param debtorCapacity the capacity of whoever will owe — the member on a
 *        purchase, the counterparty on a sale. `undefined` while unknown.
 * @param amount the obligation's size; 0 or less means nothing to say yet
 * @param maturityEpochs the term the trade names, in epochs from now
 * @param insuredHorizon the network's insured horizon in epochs
 *        (`NetworkView.insured_horizon_epochs`); a claim maturing past it is
 *        booked uninsured however much capacity carries it
 */
export function insuranceOutlook(
    direction: Direction,
    debtorCapacity: number | undefined,
    amount: number,
    maturityEpochs?: number,
    insuredHorizon?: number,
): InsuranceOutlook {
    if (debtorCapacity === undefined || !(amount > 0)) return 'unknown';
    // The gate is the debtor's capacity against the amount, on both lanes and
    // in both directions — asked first, because its copy is true whatever the
    // term is, and a term that is also too long would otherwise be told a
    // shorter one insures it.
    if (debtorCapacity < amount) return direction === 'buy' ? 'uninsured-theirs' : 'uninsured-mine';
    // Then the horizon: capacity carries it, and the date is what does not.
    if (maturityEpochs !== undefined && insuredHorizon !== undefined && maturityEpochs > insuredHorizon) {
        return direction === 'buy' ? 'beyond-horizon-theirs' : 'beyond-horizon-mine';
    }
    return 'insured';
}

/**
 * Has the community put nothing behind this counterparty?
 *
 * Relevant in both directions and for one reason only: an arbitration panel is
 * the only thing that binds somebody with no standing to an outcome. It is NOT
 * the insurance question — that one is the debtor's capacity above — and it is
 * not a mark against anybody. A zero is the absence of information, and every
 * account starts at one.
 */
export function counterpartyIsUnbacked(capacity: number | undefined, dust: number): boolean {
    return capacity !== undefined && capacity <= dust;
}


/**
 * The four roles a recorded trade names, from one direction.
 *
 * Extracted and tested because a swapped party is the defect this client keeps
 * producing — the insurance warning read the creditor's capacity, the assent
 * control read the wrong member, and both looked right. There is exactly one
 * fact here: **the buyer owes the seller**, so the buyer is the debtor.
 *
 * `Accept` names debtor and creditor; `Sale` names seller and buyer. They are
 * the same two parties under different names, which is why both come from one
 * place.
 */
export function tradeParties(direction: Direction, me: Party, other: Party) {
    const buyer = direction === 'buy' ? me : other;
    const seller = direction === 'buy' ? other : me;
    return { buyer, seller, debtor: buyer, creditor: seller };
}
