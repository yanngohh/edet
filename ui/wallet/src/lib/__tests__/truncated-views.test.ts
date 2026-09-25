/**
 * **The node answers a list in two shapes, and the client must read both.**
 *
 * `/members` and `/contracts` serve a bare JSON array while the community is
 * small and nobody asked for a page, and `{ members | contracts, truncated,
 * total, next }` otherwise (`crates/node/src/serve/views.rs`).
 *
 * A client typed only for the array does not degrade at that boundary, it
 * STOPS: the store holds an object, and every `$membersList.find(...)`,
 * `.filter(...)` and `.map(...)` in the app is a `TypeError`. One community
 * crossing five hundred members takes every wallet reading that node down,
 * all at once, on a ledger with nothing wrong with it.
 *
 * `just view-shape-check` cannot see this. It compares the client's types
 * against the ROW the node serves; the envelope is a different shape around
 * the same rows, and a type that never mentions it agrees with the checker and
 * disagrees with the node.
 *
 * So the two shapes are normalised in one place, the wallet WALKS the cursor
 * rather than showing a prefix, and the walk is BOUNDED: `next` comes from the
 * node, so an unbounded one is a loop a lying node can start and never end.
 * What the cap left out is kept rather than dropped — a list that quietly
 * shows four thousand of nine thousand is a wrong promise, and the store is
 * where the UI can find out.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import * as api from '../api';

let body: unknown = [];
/** Every path the client asked for, in order — the walk itself, observable. */
let asked: string[] = [];
/** When set, answers each request in turn instead of repeating `body`. */
let script: unknown[] | null = null;

beforeEach(() => {
    asked = [];
    script = null;
    vi.stubGlobal(
        'fetch',
        vi.fn(async (url: string) => {
            asked.push(new URL(url).pathname + new URL(url).search);
            const answer = script === null ? body : (script.shift() ?? []);
            return { ok: true, status: 200, json: async () => answer } as unknown as Response;
        }),
    );
    api.configureAuth(
        () => null,
        async () => {},
    );
});

const rows = (n: number, from = 0) => Array.from({ length: n }, (_, i) => ({ id: from + i }));

describe('a list the node pages is still a list', () => {
    it('reads /members in both shapes', async () => {
        body = rows(3);
        expect(await api.members('http://node')).toHaveLength(3);
        expect(api.lastTruncation('/members')).toBeNull();
        expect(asked).toEqual(['/members']);
    });

    it('reads /contracts in both shapes', async () => {
        body = rows(2);
        expect(await api.contracts('http://node')).toHaveLength(2);
        expect(api.lastTruncation('/contracts')).toBeNull();
    });

    /**
     * The walk: page one carries `next`, the client asks again from there, and
     * the union is every row once. Without the cursor the member sees a
     * five-hundred-row prefix of a nine-hundred-row community.
     */
    it('walks the cursor until the node says there is no next page', async () => {
        script = [
            { members: rows(500, 0), truncated: true, total: 917, next: 499 },
            { members: rows(417, 500), truncated: false, total: 917 },
        ];
        const all = await api.members('http://node');
        expect(all).toHaveLength(917);
        expect(all[0].id).toBe(0);
        expect(all[916].id).toBe(916);
        expect(asked).toEqual(['/members', '/members?after=499']);
        expect(api.lastTruncation('/members')).toBeNull();
    });

    /**
     * **A node that never stops offering a next page must not make the wallet
     * loop.** The walk stops at `MAX_LIST_PAGES` and reports what it left out,
     * so the copy under the list is true rather than absent.
     */
    it('stops at its own page cap and says what it left out', async () => {
        body = { contracts: rows(500), truncated: true, total: 100000, next: 499 };
        const some = await api.contracts('http://node');
        expect(some).toHaveLength(500 * api.MAX_LIST_PAGES);
        expect(asked).toHaveLength(api.MAX_LIST_PAGES);
        expect(api.lastTruncation('/contracts')).toEqual({
            shown: 500 * api.MAX_LIST_PAGES,
            total: 100000,
            next: 499,
        });
    });

    /**
     * A node that answers something neither shape describes — a lying node, a
     * proxy that rewrote the body — must not be able to hand the app a value
     * every list operation then throws on. An empty list is a degraded read;
     * a `TypeError` in ten components is not.
     */
    it('answers an unreadable body with an empty list rather than a crash', async () => {
        for (const junk of [null, {}, 'nonsense', 42, { members: 'not an array' }]) {
            body = junk;
            expect(await api.members('http://node')).toEqual([]);
            expect(await api.contracts('http://node')).toEqual([]);
        }
    });
});
