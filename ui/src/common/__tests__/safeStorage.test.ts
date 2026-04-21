import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
    isLocalStorageAvailable,
    lsGet,
    lsRemove,
    lsSet,
    resetLocalStorageProbe,
} from '../safeStorage';

describe('safeStorage', () => {
    beforeEach(() => {
        window.localStorage.clear();
        resetLocalStorageProbe();
    });

    afterEach(() => {
        resetLocalStorageProbe();
    });

    it('round-trips values when localStorage is available', () => {
        expect(isLocalStorageAvailable()).toBe(true);
        lsSet('a', 'one');
        expect(lsGet('a')).toBe('one');
        lsRemove('a');
        expect(lsGet('a')).toBeNull();
    });

    it('returns safe defaults when the probe call throws', () => {
        // Swap window.localStorage for a stub whose every method throws.
        const original = Object.getOwnPropertyDescriptor(window, 'localStorage');
        const throwing: Storage = {
            length: 0,
            clear: () => { throw new Error('denied'); },
            getItem: () => { throw new Error('denied'); },
            key: () => { throw new Error('denied'); },
            removeItem: () => { throw new Error('denied'); },
            setItem: () => { throw new Error('denied'); },
        };
        Object.defineProperty(window, 'localStorage', {
            configurable: true,
            get: () => throwing,
        });
        resetLocalStorageProbe();
        try {
            expect(isLocalStorageAvailable()).toBe(false);
            // lsSet and lsGet must be no-ops rather than bubble the error.
            expect(() => lsSet('x', '1')).not.toThrow();
            expect(lsGet('x')).toBeNull();
            expect(() => lsRemove('x')).not.toThrow();
        } finally {
            if (original) {
                Object.defineProperty(window, 'localStorage', original);
            }
        }
    });

    it('returns null when the probe succeeds but later getItem throws', () => {
        // Keep the probe path intact (setItem + removeItem work) but cause
        // only getItem to throw. This simulates a quota / ACL edge case
        // where reads start failing mid-session.
        const store = new Map<string, string>();
        let readsThrow = false;
        const flaky: Storage = {
            get length() { return store.size; },
            clear: () => store.clear(),
            getItem: (k) => {
                if (readsThrow) throw new Error('denied');
                return store.get(k) ?? null;
            },
            key: (i) => Array.from(store.keys())[i] ?? null,
            removeItem: (k) => { store.delete(k); },
            setItem: (k, v) => { store.set(k, v); },
        };
        const original = Object.getOwnPropertyDescriptor(window, 'localStorage');
        Object.defineProperty(window, 'localStorage', {
            configurable: true,
            get: () => flaky,
        });
        resetLocalStorageProbe();
        try {
            expect(isLocalStorageAvailable()).toBe(true);
            lsSet('b', 'two');
            readsThrow = true;
            expect(lsGet('b')).toBeNull();
            expect(() => lsRemove('b')).not.toThrow();
        } finally {
            if (original) {
                Object.defineProperty(window, 'localStorage', original);
            }
        }
    });
});
