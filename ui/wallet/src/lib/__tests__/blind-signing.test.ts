/**
 * **The wallet must not sign what a node tells it to sign**, as a gate.
 *
 * The hole this closes: fetching the signing payload from `/tx/digest` on
 * whichever node the app is reading and signing the reply; or co-signing a
 * pending request by signing `entry.digest`, a hex string the same node served,
 * with nothing checking that it corresponds to the `entry.tx` the member is
 * looking at. `/tx/check` is no help — the same node answers it. "The embedded
 * node IS the wallet's own code" would justify it, and this app embeds no node:
 * it is a client of one the member CHOOSES, and `custom` is any URL.
 *
 * So: the digest is computed here (`txdigest.ts`, cross-pinned by
 * `just tx-digest-check`), a pending entry whose digest does not cover its own
 * transaction is refused, and no module under `src/` may reach for the route
 * again.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { addMessages, init } from 'svelte-i18n';

import en from '../../locales/en.json';

const pendingSign = vi.fn(async () => ({ ok: true }));

vi.mock('../api', async () => {
    const actual = await vi.importActual<typeof import('../api')>('../api');
    return { ...actual, pendingSign: (...a: unknown[]) => pendingSign(...(a as [])) };
});

vi.mock('../actors', async () => {
    const { writable } = await import('svelte/store');
    return {
        currentActorId: writable<number | null>(1),
        holdsSeed: () => true,
        heldSeed: () => new Uint8Array(32).fill(7),
        heldSeedOfParty: () => new Uint8Array(32).fill(7),
        holdsParty: () => true,
        seedOfParty: () => new Uint8Array(32).fill(7),
    };
});

import type { PendingEntryView, Tx } from '../api';
import { customChainId, networkId, networkView } from '../node';
import { signPending, signingChainId } from '../submit';
import { toHex, txDigestLocal } from '../txdigest';
import { errorStore } from '../../common/errorStore';

const CHAIN = 'edet-dev';
const TX: Tx = { Settle: { contract: 4, amount: 12.5 } };
const NONCE = Array.from({ length: 16 }, (_, i) => i + 1);
const NOT_AFTER = 20_500;

function entry(over: Partial<PendingEntryView> = {}): PendingEntryView {
    return {
        digest: toHex(txDigestLocal(CHAIN, TX, NONCE, NOT_AFTER)),
        tx: TX as unknown as Record<string, unknown>,
        nonce: NONCE,
        not_after_epoch: NOT_AFTER,
        required: [{ Member: 1 }],
        min_sigs: 1,
        signed_by: [],
        opened_epoch: 20_470,
        ...over,
    } as PendingEntryView;
}

addMessages('en', en as never);
init({ fallbackLocale: 'en', initialLocale: 'en' });

beforeEach(() => {
    pendingSign.mockClear();
    errorStore.clearErrors();
    networkId.set('local');
    networkView.set({ chain_id: CHAIN, epoch: 20_470 } as never);
});

describe('a wallet signs what it can see', () => {
    it('co-signs a request whose digest covers the transaction it shows', async () => {
        expect(await signPending(entry())).toBe(true);
        expect(pendingSign).toHaveBeenCalledTimes(1);
    });

    /**
     * The attack, exactly: the node serves `entry.tx` — what the inbox renders
     * — beside the digest of something else. Nothing is signed.
     */
    it('refuses when the digest is of a DIFFERENT transaction', async () => {
        const other: Tx = { Settle: { contract: 4, amount: 12_500 } };
        const forged = entry({ digest: toHex(txDigestLocal(CHAIN, other, NONCE, NOT_AFTER)) });
        expect(await signPending(forged)).toBe(false);
        expect(pendingSign).not.toHaveBeenCalled();
    });

    /** The same, moved into the envelope rather than the transaction. */
    it('refuses when the digest is of a different envelope', async () => {
        for (const over of [{ nonce: NONCE.map((b, i) => (i === 0 ? b ^ 1 : b)) }, { not_after_epoch: NOT_AFTER + 1 }]) {
            expect(await signPending(entry(over))).toBe(false);
        }
        expect(pendingSign).not.toHaveBeenCalled();
    });

    /**
     * **And the chain id is not the node's to choose** where the network
     * declares one. A node that reported another ledger would otherwise
     * collect a signature valid THERE, for a transaction approved here.
     */
    it('refuses to sign when the node claims a chain the network does not declare', async () => {
        networkView.set({ chain_id: 'edet-somewhere-else', epoch: 20_470 } as never);
        expect(await signPending(entry())).toBe(false);
        expect(pendingSign).not.toHaveBeenCalled();
    });
});

/**
 * The other half, and the one a future change is most likely to undo: no
 * module the wallet ships may ask a node what to sign. A grep rather than a
 * type, because the hazard is a NEW call site and a type only constrains the
 * ones that exist.
 */
describe('no signing path asks a node for its payload', () => {
    it('never references /tx/digest anywhere under src/', () => {
        const offenders: string[] = [];
        const walk = (dir: string) => {
            for (const name of readdirSync(dir)) {
                const path = join(dir, name);
                if (statSync(path).isDirectory()) {
                    walk(path);
                    continue;
                }
                if (!/\.(ts|svelte|js)$/.test(name)) continue;
                // The suites are not the wallet; this file itself names the
                // route in the assertion below.
                if (path.includes('__tests__')) continue;
                // A CALL, not a mention. Several files name the route in the
                // comment explaining why nothing calls it, so the comments come
                // out first — a gate nobody can keep green while writing down
                // the reason it exists is a gate that gets deleted.
                const text = readFileSync(path, 'utf8')
                    .replace(/\/\*[\s\S]*?\*\//g, '')
                    .replace(/(^|[^:])\/\/.*$/gm, '$1');
                if (text.includes('/tx/digest')) offenders.push(path);
            }
        };
        walk(join(__dirname, '..', '..'));
        expect(offenders, 'a wallet computes its own digest — see lib/txdigest.ts').toEqual([]);
    });
});

/** The reason a signing refusal carries, or `null` when nothing refused. */
function refusal(fn: () => unknown): string | null {
    try {
        fn();
        return null;
    } catch (e) {
        const err = e as { name?: string; reason?: string };
        return err.name === 'SigningRefused' ? (err.reason ?? 'refused') : `unexpected: ${String(e)}`;
    }
}

/**
 * **The chain id is never the node's to choose.** A named network declares
 * it; `custom` is the member's own declaration; and a node's answer is only
 * ever CHECKED against one of those — with nothing declared the wallet
 * refuses rather than binding a signature to whatever the node reports.
 */
describe('the chain id a signature binds to', () => {
    it('is the named network\'s declaration, held against what the node reports', () => {
        networkId.set('local');
        expect(signingChainId()).toBe(CHAIN);
        networkView.set({ chain_id: 'edet-somewhere-else', epoch: 20_470 } as never);
        expect(refusal(signingChainId)).toBe('chain');
    });

    it('on `custom` is the member\'s declaration, and refuses without one', () => {
        networkId.set('custom');
        customChainId.set('');
        networkView.set({ chain_id: 'edet-federation', epoch: 20_470 } as never);
        // The node names a chain; that is exactly the answer not to take.
        expect(refusal(signingChainId)).toBe('undeclared');
        customChainId.set('edet-federation');
        expect(signingChainId()).toBe('edet-federation');
        customChainId.set('edet-other');
        expect(refusal(signingChainId)).toBe('chain');
    });

    it('signs nothing for a pending request on an undeclared custom network', async () => {
        networkId.set('custom');
        customChainId.set('');
        expect(await signPending(entry())).toBe(false);
        expect(pendingSign).not.toHaveBeenCalled();
    });
});
