/**
 * The credential a read carries, and which key it speaks for.
 *
 * The read-credential tests. An `ipc-reads` shape pinning the proof an embedded-node
 * transport passed as a command argument. That transport is gone — the app is
 * a client of a node it does not run, so every read is HTTP and carries the
 * bearer token, exactly as the browser always did. What did NOT go away is the
 * property those tests existed for, because it was never about the transport:
 *
 *   **the needle of a lookup and the key of the caller are different things.**
 *
 * The defect: `whois` once verified `sig`/`ts` against the key being LOOKED
 * UP, so the only proof a caller could construct was over the very key it was
 * asking about. A member could resolve nobody but themself, and resolving a
 * counterparty — the first step of every purchase, vouch and admission —
 * was impossible. The shape that prevents it is the one asserted here: the
 * needle travels in the PATH, the caller in the HEADERS, and `pendingListByKey`
 * is the deliberate opposite case where they are the same key on purpose.
 *
 * A Rust test cannot see any of this; it is the client's half of the contract
 * with `crates/node/src/serve/auth.rs`.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import * as api from '../api';
import type { KeyProof } from '../session';

const CALLER = 'ca11e4'.padEnd(64, '0');
const NEEDLE = 'be11e5'.padEnd(64, '0');

const proofFor = (path: string): KeyProof => ({ key: CALLER, sig: `sig-over-${path}`, ts: 1700000000, nonce: '0123456789abcdef0123456789abcdef' });

/** The last fetch the read layer made. */
let calls: { url: string; headers: Record<string, string> }[] = [];

beforeEach(() => {
    calls = [];
    vi.stubGlobal(
        'fetch',
        vi.fn(async (url: string, init?: RequestInit) => {
            calls.push({ url, headers: (init?.headers as Record<string, string>) ?? {} });
            return { ok: true, status: 200, json: async () => ({}) } as unknown as Response;
        }),
    );
    api.configureAuth(
        () => 'tok-for-the-current-actor',
        async () => {},
    );
});

describe('reads carry the viewer credential', () => {
    it('sends the bearer token on an ordinary read', async () => {
        await api.memberDetail('http://node', 3);
        expect(calls).toHaveLength(1);
        expect(calls[0].url).toBe('http://node/member/3');
        expect(calls[0].headers['Authorization']).toBe('Bearer tok-for-the-current-actor');
    });

    it('reads anonymously before unlock rather than failing', async () => {
        // No token yet — the pre-unlock UI reads, and must not error. The node
        // answers the anonymous view, which is a degraded read, not a failure.
        api.configureAuth(
            () => null,
            async () => {},
        );
        await api.members('http://node');
        expect(calls[0].headers['Authorization']).toBeUndefined();
    });
});

describe('a lookup keeps its needle and its caller apart', () => {
    it('puts the whois needle in the path and the caller in the headers', async () => {
        const mine = proofFor(`/whois/${NEEDLE}`);

        await api.whois('http://node', NEEDLE, mine);

        expect(calls[0].url).toBe(`http://node/whois/${NEEDLE}`);
        expect(calls[0].headers['x-edet-viewer-key']).toBe(CALLER);
        expect(calls[0].headers['x-edet-viewer-sig']).toBe(mine.sig);
        expect(calls[0].headers['x-edet-viewer-ts']).toBe('1700000000');
        // The whole point: what is asked about is not who is asking.
        expect(calls[0].headers['x-edet-viewer-key']).not.toBe(NEEDLE);
    });

    it('sends no viewer proof for a whois the caller could not sign', async () => {
        // Onboarding builds its own proof and passes it explicitly; with no
        // seed yet the lookup goes out anonymous and the node answers
        // `{"member": null}` — the truthful "not admitted yet", not an error.
        await api.whois('http://node', NEEDLE);
        expect(calls[0].headers['x-edet-viewer-key']).toBeUndefined();
    });

    it('proves the SAME key it queries for the pending-by-key read', async () => {
        // The deliberate opposite of whois: this read has no member id to
        // compare against, so possession of the key IS the authorisation
        // (`serve::auth::ViewerParty`). Needle and caller coincide by design.
        const own = { key: NEEDLE, sig: 'sig-over-own-key', ts: 1700000000, nonce: '0123456789abcdef0123456789abcdef' };

        await api.pendingListByKey('http://node', NEEDLE, own);

        expect(calls[0].url).toBe(`http://node/pending/key/${NEEDLE}`);
        expect(calls[0].headers['x-edet-viewer-key']).toBe(NEEDLE);
    });
});

describe('the transaction pre-flight is a read, and authenticates like one', () => {
    it('carries the bearer token on POST /tx/check', async () => {
        // Answered anonymously the node returns the public bond quote with no
        // `ok`, and the UI reports ET-UNKNOWN for every action. That happened
        // once already.
        const tx = { tx: {}, nonce: [], not_after_epoch: 1, signers: [], signatures: [] };
        await api.checkTx('http://node', tx as never);
        expect(calls[0].url).toBe('http://node/tx/check');
        expect(calls[0].headers['Authorization']).toBe('Bearer tok-for-the-current-actor');
    });
});
