import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';

import {
    adoptPendingSeed,
    chooseActor,
    clearActor,
    currentActorId,
    heldSeed,
    holdsSeed,
    keyring,
    nicknames,
    pendingIdentity,
    rememberSeed,
    seedOf,
    setNickname,
    setPendingSeed,
} from '../actors';

beforeEach(() => {
    window.localStorage.clear();
    keyring.set({});
    nicknames.set({});
    clearActor();
});

describe('actor selection', () => {
    it('persists the chosen actor id', () => {
        chooseActor(3);
        expect(get(currentActorId)).toBe(3);
        expect(window.localStorage.getItem('edet-actor')).toBe('3');
        clearActor();
        expect(window.localStorage.getItem('edet-actor')).toBeNull();
    });
});

describe('held seeds', () => {
    it('has NO fallback: an unheld member cannot be signed for', () => {
        expect(heldSeed(2)).toBeNull();
        expect(holdsSeed(2)).toBe(false);
        expect(() => seedOf(2)).toThrow();
    });

    it('returns a locally held seed', () => {
        const custom = new Uint8Array(32).fill(9);
        rememberSeed(2, custom);
        expect(seedOf(2)).toEqual(custom);
        expect(holdsSeed(2)).toBe(true);
        expect(heldSeed(3)).toBeNull();
    });
});

describe('pending identity', () => {
    it('parks a created-but-unadmitted seed and adopts it under its id', () => {
        const seed = new Uint8Array(32).fill(7);
        setPendingSeed(seed);
        expect(Array.from(get(pendingIdentity)!)).toEqual(Array.from(seed));
        expect(holdsSeed(5)).toBe(false);
        adoptPendingSeed(5);
        expect(get(pendingIdentity)).toBeNull();
        expect(seedOf(5)).toEqual(seed);
    });
});

describe('nicknames', () => {
    it('stores and clears local names', () => {
        setNickname(4, 'Ada');
        expect(get(nicknames)[4]).toBe('Ada');
        setNickname(4, '   ');
        expect(get(nicknames)[4]).toBeUndefined();
    });
});
