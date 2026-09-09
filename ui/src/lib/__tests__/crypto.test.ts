import { describe, expect, it } from 'vitest';

import {
    bytesToHex,
    derivePublicKey,
    generatePhrase,
    hexToBytes,
    isValidPhrase,
    normalizePhrase,
    seedFromPhrase,
    signDigest,
    verifyDigest,
} from '../crypto';

// Standard BIP39 test vector (empty passphrase): the first 32 bytes of the
// derived seed are a fixed, cross-implementation constant.
const VECTOR_PHRASE = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
const VECTOR_SEED32 = '5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1';

describe('recovery phrases', () => {
    it('generates valid 12-word phrases', () => {
        const p = generatePhrase();
        expect(p.split(' ')).toHaveLength(12);
        expect(isValidPhrase(p)).toBe(true);
    });

    it('rejects garbage and tolerates messy whitespace/case', () => {
        expect(isValidPhrase('definitely not a phrase')).toBe(false);
        expect(isValidPhrase('  ' + VECTOR_PHRASE.toUpperCase().replace(/ /g, '   ') + '  ')).toBe(true);
        expect(normalizePhrase('  A  b ')).toBe('a b');
    });

    it('derives the standard BIP39 seed (first 32 bytes)', () => {
        expect(bytesToHex(seedFromPhrase(VECTOR_PHRASE))).toBe(VECTOR_SEED32);
    });

    it('same phrase → same key; different phrase → different key', () => {
        const k1 = derivePublicKey(seedFromPhrase(VECTOR_PHRASE));
        const k2 = derivePublicKey(seedFromPhrase(VECTOR_PHRASE));
        expect(k1).toEqual(k2);
        const other = generatePhrase();
        expect(derivePublicKey(seedFromPhrase(other))).not.toEqual(k1);
    });
});

describe('ed25519 signing', () => {
    it('sign/verify round-trips over a digest', () => {
        const seed = seedFromPhrase(VECTOR_PHRASE);
        const digest = hexToBytes('aa'.repeat(32));
        const pub = derivePublicKey(seed);
        const sig = signDigest(digest, seed);
        expect(sig).toHaveLength(64);
        expect(verifyDigest(sig, digest, pub)).toBe(true);
        // Tampered digest fails.
        expect(verifyDigest(sig, hexToBytes('bb'.repeat(32)), pub)).toBe(false);
    });
});

describe('hex helpers', () => {
    it('round-trips and strips 0x', () => {
        expect(bytesToHex(hexToBytes('0xdeadbeef'))).toBe('deadbeef');
        expect(() => hexToBytes('xyz')).toThrow();
    });
});
