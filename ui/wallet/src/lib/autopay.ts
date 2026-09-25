/**
 * **Pay at maturity**: a standing instruction on one debt, kept on this
 * device, that opens the payment request the day before the debt falls due.
 *
 * A settlement is two-party — the debtor offers, the creditor acknowledges —
 * so what this automates is the debtor's half: it signs a `Settle` for the
 * outstanding amount and parks it in the pending pool, where the creditor
 * finds it under Requests and is notified of it. Nothing is discharged until
 * they sign, and this device never signs for them.
 *
 * It runs exactly as far as the acceptance rule does: while this app is
 * running, in front or in the background, with the vault unlocked. A closed
 * app opens nothing, and the copy beside the switch says so. The instruction
 * is a device preference like the rule, persisted per actor, and not part of
 * the identity backup.
 *
 * Fired the day before maturity, or on the due day if the switch was set
 * later than that; once per contract per epoch, and never while a settlement
 * of this contract is already waiting in the pool.
 */

import { get, writable } from 'svelte/store';

import type { ContractView, PendingView } from './api';
import { tx } from './api';
import { currentActorId, holdsSeed } from './actors';
import { contractsList, firstLoadDone, networkView, nodeUp, pendingView } from './node';
import { send } from './submit';
import { lsGet, lsSet } from '../common/safeStorage';

const KEY = 'edet-autopay';

/** Contract ids armed to pay at maturity, for the acting identity. */
export const autopay = writable<Set<number>>(new Set());

function load(actor: number | null): Set<number> {
    if (actor === null) return new Set();
    try {
        const raw = lsGet(KEY);
        if (!raw) return new Set();
        const parsed = JSON.parse(raw);
        if (!parsed || parsed.actor !== actor || !Array.isArray(parsed.ids)) return new Set();
        return new Set(parsed.ids.filter((n: unknown) => typeof n === 'number'));
    } catch {
        return new Set();
    }
}

function save(actor: number | null, ids: Set<number>): void {
    if (actor === null) return;
    lsSet(KEY, JSON.stringify({ actor, ids: [...ids] }));
}

/** Arm or disarm one contract. */
export function setAutopay(contractId: number, on: boolean): void {
    autopay.update((ids) => {
        const next = new Set(ids);
        if (on) next.add(contractId);
        else next.delete(contractId);
        save(get(currentActorId), next);
        return next;
    });
}

/**
 * Whether the instruction fires for this contract now. Pure.
 *
 * The day before maturity is the day, and the due day itself is late but not
 * too late — a settlement acknowledged on the due day still lands before the
 * sweep. Past that the debt is expired and the debtor cures it by hand,
 * since a cure is a different sentence to sign.
 */
export function shouldPayNow(c: ContractView, epoch: number): boolean {
    if (c.status !== 'active' || c.outstanding <= 0) return false;
    const left = c.maturity_epoch - epoch;
    return left <= 1 && left >= 0;
}

/** A settlement of this contract already in the pool, mine or theirs. */
export function settlementPending(view: PendingView | null | undefined, contractId: number): boolean {
    if (!view) return false;
    const names = (e: { tx: Record<string, any> }) => e.tx?.Settle?.contract === contractId;
    return view.mine.some(names) || view.awaiting_me.some(names);
}

/** What was already tried this epoch: `contract:epoch`, so a refusal is not retried until the clock moves. */
const tried = new Set<string>();
let running = false;
/** A poll that landed while a sweep was running: swept again after it, not dropped. */
let again = false;
let unsubscribers: Array<() => void> = [];

async function sweep(): Promise<void> {
    const me = get(currentActorId);
    if (me === null || !holdsSeed(me)) return;
    const epoch = get(networkView)?.epoch;
    if (epoch === undefined) return;
    const armed = get(autopay);
    if (armed.size === 0) return;
    const pool = get(pendingView);
    for (const c of get(contractsList)) {
        if (!armed.has(c.id) || c.debtor !== me) continue;
        if (!shouldPayNow(c, epoch) || settlementPending(pool, c.id)) continue;
        const key = `${c.id}:${epoch}`;
        if (tried.has(key)) continue;
        tried.add(key);
        try {
            await send(tx.settle(c, c.outstanding));
        } catch {
            // Refused or failed: the member sees it on the card, and the
            // instruction tries again when the clock moves.
        }
    }
}

function kick(): void {
    if (!get(firstLoadDone) || !get(nodeUp)) return;
    if (running) {
        again = true;
        return;
    }
    running = true;
    void sweep().finally(() => {
        running = false;
        if (again) {
            again = false;
            kick();
        }
    });
}

/** Start carrying out the instructions. Idempotent. */
export function startAutopay(): void {
    if (unsubscribers.length > 0) return;
    autopay.set(load(get(currentActorId)));
    const onActor = currentActorId.subscribe((id) => {
        tried.clear();
        autopay.set(load(id));
    });
    const onContracts = contractsList.subscribe(() => kick());
    const onNetwork = networkView.subscribe(() => kick());
    unsubscribers = [onActor, onContracts, onNetwork];
}

export function stopAutopay(): void {
    for (const u of unsubscribers) u();
    unsubscribers = [];
}

/** Test-only. */
export function resetAutopayForTests(): void {
    tried.clear();
    autopay.set(new Set());
}
