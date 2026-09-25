/**
 * Which network this device acts in, and what counts as having CHOSEN one.
 *
 * The distinction is the whole test. `networkId` is a svelte store that
 * persists on subscription, so merely importing this module writes its
 * default to storage — before any member has seen a wizard. A first-run gate
 * that asks "is a network stored?" is therefore answered `yes` on a device
 * that has never been asked, and the step never appears. It is a separate
 * flag for that reason, written only by the step's own Continue.
 *
 * That matters more here than for the other first-run steps: the network
 * decides which ledger a member's standing lives on, founding is
 * irreversible, and there is no merge. A member who is never asked has the
 * default chosen for them, permanently, without being told.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

beforeEach(() => {
    window.localStorage.clear();
    vi.resetModules();
});

describe('the first-run network gate', () => {
    // The timeout is explicit because what this awaits is a module IMPORT:
    // `node.ts` pulls the client's whole store graph, and vitest's default 5 s
    // is a wall-clock bound on a transform that runs alongside every other
    // suite in the run. A gate that goes red for how busy the machine is
    // teaches a reader to ignore it.
    it('does not count the store\'s own default as a member\'s choice', { timeout: 30_000 }, async () => {
        const { networkId } = await import('../node');
        // Subscribing is what the app does; the default lands in storage.
        const stop = networkId.subscribe(() => {});
        stop();
        expect(window.localStorage.getItem('edet-network')).toBe('local');
        // ...and yet nobody has chosen anything. This is the flag App.svelte
        // reads, and it must still be unset.
        expect(window.localStorage.getItem('edet-network-chosen')).toBeNull();
    });
});

describe('resolveNodes', () => {
    it('gives a named network its declared nodes and ignores the custom URL', async () => {
        const { resolveNodes } = await import('../networks');
        expect(resolveNodes('local', 'http://typed-earlier:9999')).toEqual(['http://localhost:7001']);
    });

    it('gives `custom` the typed URL, and nothing when it is empty', async () => {
        const { resolveNodes } = await import('../networks');
        expect(resolveNodes('custom', 'http://my-node:7001')).toEqual(['http://my-node:7001']);
        // No URL means no node — which the poll reports as unreachable rather
        // than fetching a relative path against the app's own origin.
        expect(resolveNodes('custom', '')).toEqual([]);
    });

    it('is empty for a network id that does not exist', async () => {
        const { resolveNodes } = await import('../networks');
        expect(resolveNodes('mainnet', '')).toEqual([]);
    });
});
