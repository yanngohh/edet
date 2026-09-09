/**
 * `/tx/outcome/:hash` surfaces a commit-time apply failure (H4) that a
 * queued response alone can't show. These tests exercise the typed client
 * and the toast-on-rejection helper against a mocked transport (no live
 * node in this environment).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { init, register } from 'svelte-i18n';

vi.mock('../node', async () => {
    const { writable } = await import('svelte/store');
    return { activeBase: writable('http://node.test') };
});

import * as api from '../api';
import { watchOutcome } from '../submit';
import { errorStore } from '../../common/errorStore';

const jsonResponse = (body: unknown) => ({ ok: true, json: async () => body }) as Response;

register('en', () => Promise.resolve({}));
init({ fallbackLocale: 'en', initialLocale: 'en' });

beforeEach(() => {
    vi.restoreAllMocks();
    errorStore.clearErrors();
});

describe('api.txOutcome', () => {
    it('parses each status the node can report', async () => {
        const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ status: 'ok' }));
        vi.stubGlobal('fetch', fetchMock);

        const out = await api.txOutcome('http://node.test', 'ab'.repeat(32));
        expect(out).toEqual({ status: 'ok' });
        // Goes through the shared `read()` path now (so it carries a viewer
        // credential like every other read), hence the options argument.
        expect(fetchMock).toHaveBeenCalledWith('http://node.test/tx/outcome/' + 'ab'.repeat(32), { headers: {} });
    });

    it('carries the ET code through on rejection', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ status: 'rejected', code: 'ET-CTR-007' })));
        const out = await api.txOutcome('http://node.test', 'cd'.repeat(32));
        expect(out).toEqual({ status: 'rejected', code: 'ET-CTR-007' });
    });

    it('throws on a non-OK response', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 500 }));
        await expect(api.txOutcome('http://node.test', 'ab'.repeat(32))).rejects.toThrow();
    });
});

describe('submit.watchOutcome', () => {
    it('toasts the rejection message once the node reports commit-time failure', async () => {
        vi.stubGlobal(
            'fetch',
            vi.fn().mockResolvedValue(jsonResponse({ status: 'rejected', code: 'ET-CTR-007' })),
        );
        await watchOutcome('ab'.repeat(32), 1, 0);
        const errors = get(errorStore);
        expect(errors).toHaveLength(1);
        expect(errors[0].message).toContain('ET-CTR-007');
    });

    it('stays silent when the outcome is ok', async () => {
        vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ status: 'ok' })));
        await watchOutcome('ab'.repeat(32), 1, 0);
        expect(get(errorStore)).toHaveLength(0);
    });

    it('stays silent (does not false-alarm) on transport failure', async () => {
        vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('network down')));
        await watchOutcome('ab'.repeat(32), 1, 0);
        expect(get(errorStore)).toHaveLength(0);
    });
});
