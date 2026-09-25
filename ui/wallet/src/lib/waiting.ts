/**
 * "Somebody is waiting on you" — the other half of what background mode is
 * for.
 *
 * The acceptance rule notifies what it SIGNED (`lib/background.ts`). That
 * covers the requests it was allowed to decide; everything else is precisely
 * what it was not, and those are the ones that need a person. A wallet left
 * running on a till so it can answer while nobody is looking, which then says
 * nothing when it cannot answer, has kept the easy half of the promise.
 *
 * Two things can be waiting, and they reach the member from different places:
 *
 *   - **A signature request** the rule left in the hold band, or that is out
 *     of its lane entirely (a key rotation, a governance vote, a purchase
 *     carrying an arbitration panel). It is in the pending pool under
 *     Requests.
 *   - **A support-circle approval.** Listing somebody costs them nothing and
 *     raises no pending-pool entry, so this one appears under Requests NEVER —
 *     it lives on one page, which is exactly why it is easy to miss.
 *
 * **Announced once, and only after the rule has had its pass.** An entry is
 * notified when it is still awaiting on a SECOND consecutive poll: the sweep
 * runs on the same store update, so anything the rule takes is gone before
 * then, and a member is not told a purchase needs them and then told it was
 * signed for them a moment later. The cost is one poll of delay on a message
 * that is not urgent by construction — it is waiting for a human.
 */

import { get, writable } from 'svelte/store';
import { _ as _t } from 'svelte-i18n';

import { formatNumber } from '../common/functions';
import { currentActorId } from './actors';
import { memberName } from './display';
import { epochDate } from './epoch';
import { CHANNEL_WAITING, notifyIfAway } from './background';
import { pendingView, supporterApprovals } from './node';
import { txSummary } from './txSummary';
import { lsGet, lsSet } from '../common/safeStorage';

/**
 * Digests already announced, so a request waiting for a week is one
 * notification and not one per poll.
 *
 * Persisted, and bounded: a restart must not re-announce a week's worth of
 * requests at once, which is the shape this failure would take on a device
 * that is restarted often. Keyed by actor for the same reason the decision
 * list is — another identity's inbox is not this one's.
 */
const KEY = 'edet-announced';
const MAX_ANNOUNCED = 200;

function load(actor: number | null): Set<string> {
    if (actor === null) return new Set();
    try {
        const raw = lsGet(KEY);
        if (!raw) return new Set();
        const parsed = JSON.parse(raw);
        if (!parsed || parsed.actor !== actor || !Array.isArray(parsed.ids)) return new Set();
        return new Set(parsed.ids.slice(0, MAX_ANNOUNCED));
    } catch {
        return new Set();
    }
}

function save(actor: number | null, ids: Set<string>): void {
    if (actor === null) return;
    lsSet(KEY, JSON.stringify({ actor, ids: [...ids].slice(-MAX_ANNOUNCED) }));
}

let announced = new Set<string>();
/** False until the support count has been read once; see the subscriber. */
let primed = false;
/** Digests seen on the previous poll, which is what "the rule left it" means. */
let seenLastPoll = new Set<string>();

/** How many support approvals were owed at the last look. */
export const supportOwed = writable<number>(0);

/**
 * One line for a waiting request: the same summary the Requests page shows.
 * Never throws — it is called from a store subscription.
 */
function describe(tx: Record<string, any>): string {
    try {
        return txSummary(tx, {
            t: get(_t),
            nameOf: get(memberName),
            fmt: (n, d) => formatNumber(n, d ?? 2),
            dateOf: get(epochDate),
        });
    } catch {
        return '';
    }
}

let unsubscribers: Array<() => void> = [];

/**
 * Start announcing what is waiting. Idempotent, and safe to call before the
 * first poll: nothing is announced until an entry has survived one.
 */
export function startWaitingNotices(): void {
    if (unsubscribers.length > 0) return;
    announced = load(get(currentActorId));

    const onActor = currentActorId.subscribe((id) => {
        announced = load(id);
        seenLastPoll = new Set();
    });

    const onPending = pendingView.subscribe((p) => {
        const entries = p?.awaiting_me ?? [];
        const now = new Set(entries.map((e) => e.digest));
        for (const entry of entries) {
            // Still here after a full poll, so the rule did not take it.
            if (!seenLastPoll.has(entry.digest)) continue;
            if (announced.has(entry.digest)) continue;
            announced.add(entry.digest);
            const t = get(_t);
            void notifyIfAway(
                t('waiting.requestTitle', { default: 'Waiting for you' }),
                describe(entry.tx) ||
                    t('waiting.requestBody', { default: 'A request needs your signature. Open edet to decide it.' }),
                CHANNEL_WAITING,
            );
        }
        seenLastPoll = now;
        // Forget what has left the pool, or the set grows for the life of the
        // device; the bound above is a backstop, not the mechanism.
        announced = new Set([...announced].filter((d) => now.has(d)));
        save(get(currentActorId), announced);
    });

    // The support circle has no pool entry to key on: what changes is a count,
    // so the RISE is the event. A fall is somebody withdrawing or this member
    // approving, and neither is news.
    const onSupport = supporterApprovals.subscribe((n) => {
        const before = get(supportOwed);
        supportOwed.set(n);
        // **The first emission establishes the baseline and announces
        // nothing.** A member opening the app is already looking at it, and a
        // count that has been owed for a week is not news because this device
        // has just started reading it.
        if (!primed) {
            primed = true;
            return;
        }
        if (n > before) {
            const t = get(_t);
            void notifyIfAway(
                t('waiting.supportTitle', { default: 'Waiting for your approval' }),
                t('waiting.supportBody', {
                    values: { count: n },
                    default: `${n} waiting for your approval in your support circle. Open edet → Support Circle.`,
                }),
                CHANNEL_WAITING,
            );
        }
    });

    unsubscribers = [onActor, onPending, onSupport];
}

export function stopWaitingNotices(): void {
    for (const u of unsubscribers) u();
    unsubscribers = [];
    seenLastPoll = new Set();
    primed = false;
}

/** Test-only: forget what has been announced and what was seen. */
export function resetWaitingForTests(): void {
    announced = new Set();
    seenLastPoll = new Set();
    primed = false;
    supportOwed.set(0);
}
