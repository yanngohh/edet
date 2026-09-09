/**
 * Read-session tokens (the paper's §Implementation): one signed `POST /session`
 * request at unlock/actor-switch,
 * exchanged for a bearer token carried on subsequent reads. These tests
 * exercise the client-side logic against a mocked transport and a mocked
 * seed ring (no live node in this environment).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const seeds = new Map<number, Uint8Array>();
vi.mock('../actors', () => ({
    heldSeed: (id: number) => seeds.get(id) ?? null,
}));

import {
    clearSession,
    currentToken,
    ensureSession,
    mintSession,
    nowUnixSecs,
    remintSession,
    viewerAuthMessage,
    viewerChain,
    viewerNonce,
} from '../session';
import * as api from '../api';
import { verifyDigest } from '../crypto';

const jsonResponse = (body: unknown, ok = true) => ({ ok, json: async () => body }) as Response;

beforeEach(() => {
    vi.restoreAllMocks();
    seeds.clear();
    clearSession();
});

describe('viewerAuthMessage', () => {
    const NONCE = '0123456789abcdef0123456789abcdef';

    it('matches the node\'s exact byte shape', () => {
        viewerChain.set('edet-somewhere');
        const msg = new TextDecoder().decode(viewerAuthMessage('GET', '/members', 1000, NONCE));
        expect(msg).toBe(`edet-view-v2\nedet-somewhere\nGET /members\n1000\n${NONCE}`);
    });

    /** A credential is bound to ONE ledger: the same read, signed on another
     *  chain, is different bytes and does not verify there. */
    it('binds the credential to its chain', () => {
        viewerChain.set('edet-a');
        const a = new TextDecoder().decode(viewerAuthMessage('GET', '/members', 1000, NONCE));
        viewerChain.set('edet-b');
        const b = new TextDecoder().decode(viewerAuthMessage('GET', '/members', 1000, NONCE));
        expect(a).not.toBe(b);
    });

    /** **The node admits each verified signature once inside its window**, and
     *  Ed25519 is deterministic — so without a fresh nonce two honest reads of
     *  one path in one second would be one credential and the node would
     *  refuse the second. */
    it('is a different message for every nonce, and the nonce is 16 random bytes', () => {
        viewerChain.set('edet-a');
        const a = new TextDecoder().decode(viewerAuthMessage('GET', '/members', 1000, viewerNonce()));
        const b = new TextDecoder().decode(viewerAuthMessage('GET', '/members', 1000, viewerNonce()));
        expect(a).not.toBe(b);
        expect(viewerNonce()).toMatch(/^[0-9a-f]{32}$/);
        expect(viewerNonce()).not.toBe(viewerNonce());
    });
});

describe('mintSession', () => {
    it('signs the mint request with the held seed and stores the returned token', async () => {
        const seed = new Uint8Array(32).fill(7);
        seeds.set(3, seed);
        const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ token: 'tok-1', member_id: 3, expires_secs: nowUnixSecs() + 300 }));
        vi.stubGlobal('fetch', fetchMock);

        const token = await mintSession('http://node.test', 3);
        expect(token).toBe('tok-1');

        const [url, init] = fetchMock.mock.calls[0];
        expect(url).toBe('http://node.test/session');
        expect(init.method).toBe('POST');
        const headers = init.headers as Record<string, string>;
        expect(headers['x-edet-viewer']).toBe('3');
        expect(Number(headers['x-edet-viewer-ts'])).toBeGreaterThan(0);
        expect(headers['x-edet-viewer-nonce']).toMatch(/^[0-9a-f]{32}$/);

        // The signature verifies against the same message shape the node
        // recomputes (`crates/node/src/serve/auth.rs::viewer_auth_message`).
        const ts = Number(headers['x-edet-viewer-ts']);
        const msg = viewerAuthMessage('POST', '/session', ts, headers['x-edet-viewer-nonce']);
        const sigBytes = Uint8Array.from(headers['x-edet-viewer-sig'].match(/.{2}/g)!.map((b) => parseInt(b, 16)));
        const { derivePublicKey } = await import('../crypto');
        expect(verifyDigest(sigBytes, msg, derivePublicKey(seed))).toBe(true);

        expect(currentToken(3)).toBe('tok-1');
    });

    it('resolves to null (no network) when this device does not hold the seed', async () => {
        const fetchMock = vi.fn();
        vi.stubGlobal('fetch', fetchMock);
        const token = await mintSession('http://node.test', 42);
        expect(token).toBeNull();
        expect(fetchMock).not.toHaveBeenCalled();
    });

    it('resolves to null on a non-OK response rather than throwing', async () => {
        seeds.set(1, new Uint8Array(32).fill(1));
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ error: 'bad signature' }, false)));
        await expect(mintSession('http://node.test', 1)).resolves.toBeNull();
    });

    it('resolves to null on a transport failure rather than throwing', async () => {
        seeds.set(1, new Uint8Array(32).fill(1));
        vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('network down')));
        await expect(mintSession('http://node.test', 1)).resolves.toBeNull();
    });

    it('mints on every host, including the desktop and mobile app', async () => {
        // The opposite — a no-op under `isTauri()` — would be right only if
        // was right while the app read a node embedded in its own process:
        // IPC carries no headers, so there was nothing for a bearer token to
        // ride on and reads authenticated with a per-read key proof instead.
        // The app is a client of a node it does not run now, so the token is
        // the credential everywhere, and a host that skipped minting would
        // read its own ledger as a stranger.
        seeds.set(1, new Uint8Array(32).fill(1));
        (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
        const fetchMock = vi
            .fn()
            .mockResolvedValue(jsonResponse({ token: 't', member_id: 1, expires_secs: nowUnixSecs() + 300 }));
        vi.stubGlobal('fetch', fetchMock);
        try {
            await expect(mintSession('http://node.test', 1)).resolves.toBe('t');
            expect(fetchMock).toHaveBeenCalled();
        } finally {
            delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
        }
    });
});

describe('currentToken', () => {
    it('is null before anything is minted', () => {
        expect(currentToken(1)).toBeNull();
    });

    it('is null for a different member id than the one a token was minted for', async () => {
        seeds.set(1, new Uint8Array(32).fill(1));
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ token: 't', member_id: 1, expires_secs: nowUnixSecs() + 300 })));
        await mintSession('http://node.test', 1);
        expect(currentToken(1)).toBe('t');
        expect(currentToken(2)).toBeNull();
    });

    it('is null once past the expiry safety margin', async () => {
        seeds.set(1, new Uint8Array(32).fill(1));
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ token: 't', member_id: 1, expires_secs: nowUnixSecs() + 1 })));
        await mintSession('http://node.test', 1);
        expect(currentToken(1)).toBeNull();
    });
});

describe('ensureSession', () => {
    it('mints once, then reuses the still-valid token without a second request', async () => {
        seeds.set(1, new Uint8Array(32).fill(1));
        const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ token: 't', member_id: 1, expires_secs: nowUnixSecs() + 300 }));
        vi.stubGlobal('fetch', fetchMock);

        expect(await ensureSession('http://node.test', 1)).toBe('t');
        expect(await ensureSession('http://node.test', 1)).toBe('t');
        expect(fetchMock).toHaveBeenCalledTimes(1);
    });

    it('resolves to null for a null member id (no acting identity yet)', async () => {
        expect(await ensureSession('http://node.test', null)).toBeNull();
    });
});

describe('remintSession', () => {
    it('discards the held token first, so a 401-triggered remint cannot short-circuit back to it', async () => {
        seeds.set(1, new Uint8Array(32).fill(1));
        const fetchMock = vi
            .fn()
            .mockResolvedValueOnce(jsonResponse({ token: 'stale', member_id: 1, expires_secs: nowUnixSecs() + 300 }))
            .mockResolvedValueOnce(jsonResponse({ token: 'fresh', member_id: 1, expires_secs: nowUnixSecs() + 300 }));
        vi.stubGlobal('fetch', fetchMock);

        expect(await ensureSession('http://node.test', 1)).toBe('stale');
        expect(await remintSession('http://node.test', 1)).toBe('fresh');
        expect(fetchMock).toHaveBeenCalledTimes(2);
        expect(currentToken(1)).toBe('fresh');
    });
});

describe('api.ts read-path integration', () => {
    it('attaches the configured bearer token to a fetch-mode read', async () => {
        const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ dust: 0.01 }));
        vi.stubGlobal('fetch', fetchMock);
        api.configureAuth(() => 'my-token', async () => {});

        await api.params('http://node.test');

        const [, init] = fetchMock.mock.calls[0];
        expect((init.headers as Record<string, string>).Authorization).toBe('Bearer my-token');
    });

    it('re-mints on a 401 and retries once with the fresh token', async () => {
        const fetchMock = vi
            .fn()
            .mockResolvedValueOnce({ ok: false, status: 401 })
            .mockResolvedValueOnce(jsonResponse({ dust: 0.01 }));
        vi.stubGlobal('fetch', fetchMock);

        let token = 'expired';
        const remint = vi.fn(async () => {
            token = 'renewed';
        });
        api.configureAuth(() => token, remint);

        const result = await api.params('http://node.test');
        expect(result).toEqual({ dust: 0.01 });
        expect(remint).toHaveBeenCalledTimes(1);
        expect(fetchMock).toHaveBeenCalledTimes(2);
        expect((fetchMock.mock.calls[1][1].headers as Record<string, string>).Authorization).toBe('Bearer renewed');
    });

    it('throws when even the post-remint retry is rejected', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 401 }));
        api.configureAuth(() => null, async () => {});
        await expect(api.params('http://node.test')).rejects.toThrow();
    });
});
