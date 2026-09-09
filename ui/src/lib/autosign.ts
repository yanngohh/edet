/**
 * The acceptance engine: the wallet applying its own rule to the requests
 * waiting on it, so that trading at volume does not mean moderating every
 * transaction by hand.
 *
 * What it will act on, and nothing else: a request in which THIS member is
 * the party granting credit — the seller of a Sale, the creditor of an
 * Accept. (A cascade drain is not a transaction of its own: it executes
 * inside the Sale the seller co-signs, so the member drained toward has no
 * request to answer. That is why the drain takes a standing pre-approval,
 * decided once on the Support page rather than here. A proposal to replace it
 * with a required signature and is refused: nothing signs while the app is
 * closed, so a signature owed by a sleeping beneficiary is a refusal.)
 * Anything that changes who someone is or what the community's rules are — key
 * rotations, guardian changes, governance, arbitration — is never signed by a
 * machine, whatever it scores. Arbitration in both senses: an attestation, and
 * a purchase that CARRIES a panel. An award is minted from the creditor to the
 * debtor, so the panel on an `Accept` is the buyer's remedy against the seller,
 * and the ledger refuses only a panel that seats a party — a buyer may name
 * their own accomplices with a quorum of one. A bench the member never read is
 * a bench they never chose, so such a request waits for them
 * (`carriesArbitration`). Nor is a request that would make THIS member the
 * debtor: taking on debt is always a decision its owner makes.
 *
 * Before any automatic signature the entry is dry-run against current state;
 * a request the ledger would reject is left alone for the member to see. So
 * is a stranger's first purchase: a member with no settled history scores at
 * the ceiling because nothing is known about them, and there is no trial tier
 * to route them into — an uninsured first trade is the ordinary way anybody
 * starts (§Recourse), and it is a decision its creditor makes, so it is held rather
 * than auto-declined.
 *
 * This runs on this device, while the app is RUNNING and the vault unlocked —
 * in front of the member or behind, with background mode on
 * (`lib/background.ts`) — and the seed never leaves it, so nothing can sign on
 * the member's behalf once the app is closed.
 */

import { get, writable } from 'svelte/store';

import type { PendingEntryView } from './api';
import { currentActorId, holdsSeed } from './actors';
import { firstLoadDone, membersList, nodeUp, paramsView, pendingView } from './node';
import { acceptancePolicy, classify, type AcceptancePolicy, type Band } from './policy';
import { subjectiveRisk, subjectivePricing, type SubjectivePricing } from './pricing';
import { memberRisk, riskK, vBase, type RiskInputs } from './risk';
import { checkPending, declinePending, signPending } from './submit';
import { notifySigned } from './background';
import { lsGet, lsSet } from '../common/safeStorage';

/** One automatic decision, for the inbox's "handled for you" list. */
export interface AutoDecision {
    digest: string;
    band: Exclude<Band, 'hold'>;
    risk: number;
    /** The party whose risk was scored (the one taking on the debt). */
    debtor: number;
    tx: Record<string, any>;
    /** When this device decided it, in milliseconds. */
    at: number;
}

/** Most recent automatic decisions, newest first. */
export const autoDecisions = writable<AutoDecision[]>([]);
const MAX_DECISIONS = 50;

/**
 * The list survives a restart, and it has to.
 *
 * It is the only account a member gets of what their rule did while they were
 * elsewhere — the whole point of deciding in the background is that they were
 * not there to watch — and a session-only list is empty exactly when they come
 * back to read it. Bounded, and a device preference like the rule itself: it
 * is a log of what THIS device did, reconstructible from the ledger's own
 * contracts, so it is not in the identity backup.
 *
 * Keyed by the actor it belongs to, and dropped whole when that changes: a
 * list of another identity's decisions is not this identity's history.
 */
const DECISIONS_KEY = 'edet-auto-decisions';

function loadDecisions(actor: number): AutoDecision[] {
    const raw = lsGet(DECISIONS_KEY);
    if (!raw) return [];
    try {
        const parsed = JSON.parse(raw);
        if (!parsed || parsed.actor !== actor || !Array.isArray(parsed.list)) return [];
        return parsed.list.slice(0, MAX_DECISIONS);
    } catch {
        return [];
    }
}

function saveDecisions(actor: number | null, list: AutoDecision[]): void {
    if (actor === null) return;
    lsSet(DECISIONS_KEY, JSON.stringify({ actor, list }));
}

/**
 * What became of one entry.
 *   signed/declined — acted on; it is leaving the pool.
 *   left            — deliberately not acted on (out of scope, hold band,
 *                     counterparty unknown, would be rejected): re-evaluate
 *                     it on a later poll, since state moves.
 *   failed          — acted on and refused. Not retried: the member decides
 *                     it by hand rather than watch the app retry in a loop.
 */
export type AutoOutcome = 'signed' | 'declined' | 'left' | 'failed';

/** Digests already decided or in flight — the pool is polled every 1.5 s
 *  and no request may be acted on twice. */
const handled = new Set<string>();

/**
 * The member whose risk decides this request, or null when the request is
 * out of the engine's scope. `me` must be the party granting the credit.
 */
export function scoredDebtor(tx: Record<string, any>, me: number): number | null {
    const kind = Object.keys(tx)[0];
    const b = tx[kind] ?? {};
    // Both sides are parties, and a party named by KEY has no row
    // to score at all: it is the account this very trade would seat, so there
    // is no history, no capacity and no open default to read. `null` sends it
    // to the human, which is the honest answer — the engine's whole job is to
    // act only where the ledger already says something.
    const id = (p: any): number | null =>
        typeof p === 'number' ? p : p && typeof p === 'object' && 'Member' in p ? (p.Member as number) : null;
    switch (kind) {
        case 'Sale': {
            // The seller consents; the buyer takes on the debt.
            const seller = id(b.seller);
            const buyer = id(b.buyer);
            return seller === me && buyer !== null && buyer !== me ? buyer : null;
        }
        case 'Accept': {
            // The creditor consents; the debtor takes on the debt.
            const creditor = id(b.creditor);
            const debtor = id(b.debtor);
            return creditor === me && debtor !== null && debtor !== me ? debtor : null;
        }
        default:
            return null;
    }
}

/**
 * Does this request pin arbitration terms? Only an `Accept` can; a `Sale` has
 * no such field. Read as a presence test, so a panel the wallet cannot parse
 * still counts as one — the engine's job is to act only on what it has read.
 */
export function carriesArbitration(tx: Record<string, any>): boolean {
    const kind = Object.keys(tx)[0];
    const arb = (tx[kind] ?? {}).arb;
    return arb !== undefined && arb !== null;
}

/**
 * The credit this request would grant, for the two kinds the engine acts on.
 *
 * `null` when the amount cannot be read — treated as over any ceiling, since a
 * request whose size this device cannot determine is precisely one a human
 * should look at.
 */
export function grantedAmount(tx: Record<string, any>): number | null {
    const kind = Object.keys(tx)[0];
    const amount = (tx[kind] ?? {}).amount;
    return typeof amount === 'number' && Number.isFinite(amount) && amount >= 0 ? amount : null;
}

/**
 * The band a counterparty's row falls in under a policy — the whole
 * decision, including the cold-start guard.
 *
 * A newcomer with no settled history scores 1.0 by construction — nobody has
 * backed them yet, so their capacity is zero by arithmetic — and not because
 * anything is known against them. There is no trial tier to route them into
 * and there is no need of one: an uninsured first trade is how everybody
 * starts, and whether to take that risk is the creditor's own decision. So a
 * cold-start score is HELD for the member rather than auto-declined. A member
 * who HAS a record — an open default — is declined on the evidence.
 */
export function decideBand(
    row: Partial<RiskInputs & { open_default: number }>,
    policy: AcceptancePolicy,
    k: number,
    v: number,
    subjective?: { member: number; pricing: SubjectivePricing },
): { band: Band; risk: number } {
    // Authenticated reads (the paper's §Implementation):
    // the node omits every one of these fields for an unauthenticated
    // read — no session token yet, or one that expired mid-poll — rather
    // than sending a zero. A missing field must never be scored as "safe";
    // hold explicitly rather than lean on `NaN` comparisons happening to
    // fall through `classify` into 'hold' today.
    if (
        row.d_in === undefined ||
        row.d_out === undefined ||
        row.capacity === undefined ||
        row.debt === undefined ||
        row.open_default === undefined
    ) {
        return { band: 'hold', risk: NaN };
    }
    // **The member's own price, where they have set one.** With no rule this
    // is `memberRisk` with the governed K, which is the kernel's own score —
    // the same call, not a second implementation. A rule can move the band,
    // and it can never reach past the guards below and around this function:
    // the debtor side is never signed, `maxAmount` still holds, a cold start
    // is still held for a human, and the dry run still has to pass.
    const risk = subjective
        ? subjectiveRisk(subjective.member, row as RiskInputs, k, v, subjective.pricing)
        : memberRisk(row as RiskInputs, k, v);
    const band = classify(risk, policy);
    // The cold start, and the reason it needs a rule of its own: an account
    // nobody has backed scores exactly 1.0 — the SAME score a well-backed
    // member at their ceiling gets — because confidence reads capacity and a
    // newcomer's is zero. The score cannot tell "nothing is known" from
    // "everything is spoken for", so the client must not pretend to either.
    // Auto-declining would close the uninsured tier the protocol opens for
    // precisely this case (§Recourse); a member who HAS a record is declined on it.
    if (band === 'reject' && row.capacity === 0 && row.open_default <= 0) return { band: 'hold', risk };
    return { band, risk };
}

/** Decide one entry under the current policy. */
export async function processEntry(entry: PendingEntryView): Promise<AutoOutcome> {
    const me = get(currentActorId);
    if (me === null || !holdsSeed(me)) return 'left';
    const policy = get(acceptancePolicy);
    if (!policy.auto) return 'left';

    const debtor = scoredDebtor(entry.tx, me);
    if (debtor === null) return 'left';
    // A panel binds the member to a remedy they have not read. The score
    // prices the buyer's default and says nothing about who would judge a
    // dispute, so this is out of the lane whatever the band.
    if (carriesArbitration(entry.tx)) return 'left';

    const row = get(membersList).find((m) => m.id === debtor);
    // No ledger row for the counterparty yet: score nothing.
    if (!row) return 'left';

    const { band, risk } = decideBand(row, policy, riskK(get(paramsView)), vBase(get(paramsView)), {
        member: debtor,
        pricing: get(subjectivePricing),
    });
    if (band === 'hold') return 'left';

    // The ceiling binds only the SIGNING half. Declining a request too large
    // to sign automatically would be worse than holding it: the score says the
    // counterparty is bad, the ceiling says only that the amount is large, and
    // those are different sentences. A large request from a well-scoring
    // counterparty is the archetype of one its owner should read.
    if (band === 'accept') {
        const amount = grantedAmount(entry.tx);
        if (amount === null || amount > policy.maxAmount) return 'left';
    }

    // The ledger's own verdict comes first: never auto-sign what would be
    // rejected, and never auto-decline on the strength of a score alone when
    // the request is already dead — either way the member should see it.
    const chk = await checkPending(entry);
    if (!chk.ok) return 'left';

    const ok = band === 'accept' ? await signPending(entry) : await declinePending(entry);
    if (!ok) return 'failed';
    let kept: AutoDecision[] = [];
    autoDecisions.update((list) => {
        kept = [{ digest: entry.digest, band, risk, debtor, tx: entry.tx, at: Date.now() }, ...list].slice(
            0,
            MAX_DECISIONS,
        );
        return kept;
    });
    saveDecisions(me, kept);
    // A signature GRANTED credit in the member's name while they were not
    // watching; a decline gave nothing away and left nothing to collect.
    // Deliberately NOT awaited: the decision is made and recorded, and a
    // notification that never comes back would stall the sweep behind it —
    // the engine's own "a call that never returns is not an error". It logs
    // its own failures and answers to nobody here.
    if (band === 'accept') void notifySigned(entry.tx);
    return band === 'accept' ? 'signed' : 'declined';
}

/** Apply the policy to everything currently awaiting this device. */
async function sweep(entries: PendingEntryView[]): Promise<void> {
    for (const e of entries) {
        if (handled.has(e.digest)) continue;
        // Claim the digest before awaiting, so the next poll cannot race it.
        handled.add(e.digest);
        let outcome: AutoOutcome = 'failed';
        try {
            outcome = await processEntry(e);
        } catch {
            outcome = 'failed';
        }
        if (outcome === 'left') handled.delete(e.digest);
    }
}

let unsubscribe: (() => void) | null = null;
let unsubscribeActor: (() => void) | null = null;
let running = false;

/** Start applying the policy to the pending pool. Idempotent. */
export function startAcceptance(): void {
    if (unsubscribe) return;
    unsubscribeActor = currentActorId.subscribe((id) => {
        handled.clear();
        autoDecisions.set(id === null ? [] : loadDecisions(id));
    });
    unsubscribe = pendingView.subscribe((p) => {
        const entries = p?.awaiting_me ?? [];
        if (entries.length === 0 || running) return;
        // Wait for a first full poll: scoring against an empty members list
        // would read every counterparty as unknown anyway.
        if (!get(firstLoadDone) || !get(nodeUp)) return;
        running = true;
        void sweep(entries).finally(() => {
            running = false;
        });
    });
}

export function stopAcceptance(): void {
    unsubscribe?.();
    unsubscribeActor?.();
    unsubscribe = null;
    unsubscribeActor = null;
    handled.clear();
}
