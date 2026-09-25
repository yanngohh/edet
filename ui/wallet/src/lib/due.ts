/**
 * **A debt that is about to fall due, or has.** The one thing a wallet must
 * say without being asked.
 *
 * The Contracts page has always shown "due in N days" beside each record, and
 * nothing brought a member to that page. In the civitas pilots every default
 * whose debtor held the cash was a debtor nobody had told: pilot-3 recorded
 * 181 such defaults and two attempts to pay, and the run that told people
 * what fell due recorded none. A reminder is not advice about whether to pay
 * — it is the date, which the ledger holds and the member does not.
 *
 * Two states, each announced once per contract: **falling due**, within
 * `DUE_SOON_EPOCHS` of maturity, and **past due**, once the sweep has marked
 * it or the clock has passed it. A contract that was announced as falling
 * due is announced again when it passes — that is a different sentence — and
 * never a third time. Announced on the waiting channel, since a debt due is
 * something only the member can act on, and shown at the top of My Contracts
 * as a short block ahead of the full list.
 */

import { get, writable } from 'svelte/store';
import { _ as _t } from 'svelte-i18n';

import type { ContractView } from './api';
import { currentActorId } from './actors';
import { CHANNEL_WAITING, notifyIfAway } from './background';
import { memberName } from './display';
import { epochDate } from './epoch';
import { contractsList, networkView } from './node';
import { formatNumber } from '../common/functions';
import { lsGet, lsSet } from '../common/safeStorage';

/** How many epochs ahead a maturity counts as falling due. */
export const DUE_SOON_EPOCHS = 3;

export type DueState = 'soon' | 'past';

export interface DueNotice {
    contract: ContractView;
    state: DueState;
    /** Epochs until maturity; zero on the due day, negative once past it. */
    left: number;
}

/**
 * My debts that are falling due or past due, nearest first. Pure: the
 * contracts, who I am, and the clock.
 *
 * Past due is the ledger's word where it has said one (`expired`) and the
 * clock's where it has not yet — the sweep marks a contract at the boundary,
 * so between maturity and the next boundary an `active` debt is past due in
 * every sense but the record's.
 */
export function dueNotices(contracts: ContractView[], me: number | null, epoch: number): DueNotice[] {
    if (me === null) return [];
    const out: DueNotice[] = [];
    for (const c of contracts) {
        if (c.debtor !== me || c.outstanding <= 0) continue;
        const left = c.maturity_epoch - epoch;
        if (c.status === 'expired' || (c.status === 'active' && left < 0)) {
            out.push({ contract: c, state: 'past', left });
        } else if (c.status === 'active' && left <= DUE_SOON_EPOCHS) {
            out.push({ contract: c, state: 'soon', left });
        }
    }
    return out.sort((a, b) => a.left - b.left || a.contract.id - b.contract.id);
}

/** What one notice is announced under: once per contract per state. */
export function noticeKey(n: DueNotice): string {
    return `${n.contract.id}:${n.state}`;
}

/**
 * Which of these notices have not been announced yet, given what has. Pure,
 * so the dedupe rule is testable without a store.
 */
export function unannounced(notices: DueNotice[], announced: Set<string>): DueNotice[] {
    return notices.filter((n) => !announced.has(noticeKey(n)));
}

/**
 * Announced keys, persisted per actor and bounded, so a restart re-announces
 * nothing and a device that is restarted often is not told the same due
 * date every morning. Keys of contracts no longer due are forgotten.
 */
const KEY = 'edet-due-announced';
const MAX_ANNOUNCED = 200;

function load(actor: number | null): Set<string> {
    if (actor === null) return new Set();
    try {
        const raw = lsGet(KEY);
        if (!raw) return new Set();
        const parsed = JSON.parse(raw);
        if (!parsed || parsed.actor !== actor || !Array.isArray(parsed.keys)) return new Set();
        return new Set(parsed.keys.slice(0, MAX_ANNOUNCED));
    } catch {
        return new Set();
    }
}

function save(actor: number | null, keys: Set<string>): void {
    if (actor === null) return;
    lsSet(KEY, JSON.stringify({ actor, keys: [...keys].slice(-MAX_ANNOUNCED) }));
}

/** What My Contracts shows above the list: the current notices. */
export const dueNow = writable<DueNotice[]>([]);

let announced = new Set<string>();
let unsubscribers: Array<() => void> = [];

function describe(n: DueNotice): string {
    const t = get(_t);
    const who = n.contract.creditor === undefined ? '' : get(memberName)(n.contract.creditor);
    const amount = formatNumber(n.contract.outstanding);
    if (n.state === 'past') {
        return t('waiting.dueBodyPast', {
            values: { id: n.contract.id, amount, who },
            default: `Contract #${n.contract.id}: ${amount} to ${who} is past due. Pay it under My Contracts to cure the default.`,
        });
    }
    const date = get(epochDate)(n.contract.maturity_epoch);
    return n.left <= 0
        ? t('waiting.dueBodyToday', {
              values: { id: n.contract.id, amount, who },
              default: `Contract #${n.contract.id}: ${amount} to ${who} falls due today.`,
          })
        : t('waiting.dueBodySoon', {
              values: { id: n.contract.id, amount, who, days: n.left, date },
              default: `Contract #${n.contract.id}: ${amount} to ${who} falls due on ${date}, in ${n.left} days.`,
          });
}

function look(): void {
    const me = get(currentActorId);
    const epoch = get(networkView)?.epoch;
    if (epoch === undefined) return;
    const notices = dueNotices(get(contractsList), me, epoch);
    dueNow.set(notices);
    const t = get(_t);
    for (const n of unannounced(notices, announced)) {
        announced.add(noticeKey(n));
        void notifyIfAway(
            n.state === 'past'
                ? t('waiting.duePastTitle', { default: 'A debt of yours is past due' })
                : t('waiting.dueSoonTitle', { default: 'A debt of yours falls due' }),
            describe(n),
            CHANNEL_WAITING,
        );
    }
    // Forget what is no longer due, so a contract that is paid and later
    // re-enters nothing is not silenced by its own past.
    const live = new Set(notices.map(noticeKey));
    announced = new Set([...announced].filter((k) => live.has(k)));
    save(me, announced);
}

/** Start announcing what falls due. Idempotent. */
export function startDueNotices(): void {
    if (unsubscribers.length > 0) return;
    announced = load(get(currentActorId));
    const onActor = currentActorId.subscribe((id) => {
        announced = load(id);
        look();
    });
    const onContracts = contractsList.subscribe(() => look());
    const onNetwork = networkView.subscribe(() => look());
    unsubscribers = [onActor, onContracts, onNetwork];
}

export function stopDueNotices(): void {
    for (const u of unsubscribers) u();
    unsubscribers = [];
}

/** Test-only. */
export function resetDueForTests(): void {
    announced = new Set();
    dueNow.set([]);
}
