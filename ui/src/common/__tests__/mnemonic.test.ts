import { describe, expect, it } from 'vitest';

import {
    deriveKeys,
    fingerprintMatches,
    generateNewMnemonic,
    isValidMnemonic,
    mnemonicFingerprint,
    normaliseMnemonic,
    pickDistinctIndices,
} from '../mnemonic';

describe('mnemonic', () => {
    it('generates a valid 12-word phrase', () => {
        for (let i = 0; i < 5; i++) {
            const m = generateNewMnemonic();
            const words = m.split(' ');
            expect(words).toHaveLength(12);
            expect(isValidMnemonic(m)).toBe(true);
        }
    });

    it('rejects invalid phrases', () => {
        expect(isValidMnemonic('')).toBe(false);
        expect(isValidMnemonic('this is not a real bip39 mnemonic at all yes really')).toBe(false);
        expect(isValidMnemonic(generateNewMnemonic() + ' extra')).toBe(false);
    });

    it('normalises whitespace and case', () => {
        const m = generateNewMnemonic();
        const upper = m.toUpperCase().split(' ').join('   \t\n  ');
        expect(normaliseMnemonic(upper)).toBe(m);
        expect(isValidMnemonic(upper)).toBe(true);
    });

    it('derives deterministic keys from the same mnemonic', async () => {
        const m = generateNewMnemonic();
        const a = await deriveKeys(m);
        const b = await deriveKeys(m);
        expect(a.agentSeed).toEqual(b.agentSeed);
        expect(a.backupKey).toEqual(b.backupKey);
        expect(a.fingerprint).toEqual(b.fingerprint);
    });

    it('produces distinct agent and backup keys from the same seed', async () => {
        const a = await deriveKeys(generateNewMnemonic());
        expect(a.agentSeed).toHaveLength(32);
        expect(a.backupKey).toHaveLength(32);
        expect(a.fingerprint).toHaveLength(4);
        // Overwhelmingly improbable to collide by chance:
        let equal = true;
        for (let i = 0; i < 32; i++) if (a.agentSeed[i] !== a.backupKey[i]) equal = false;
        expect(equal).toBe(false);
    });

    it('derives different keys for different mnemonics', async () => {
        const m1 = generateNewMnemonic();
        let m2 = generateNewMnemonic();
        while (m2 === m1) m2 = generateNewMnemonic();
        const a = await deriveKeys(m1);
        const b = await deriveKeys(m2);
        expect(a.agentSeed).not.toEqual(b.agentSeed);
        expect(a.backupKey).not.toEqual(b.backupKey);
        expect(a.fingerprint).not.toEqual(b.fingerprint);
    });

    it('fingerprint matches deriveKeys().fingerprint', async () => {
        const m = generateNewMnemonic();
        const fp1 = mnemonicFingerprint(m);
        const { fingerprint: fp2 } = await deriveKeys(m);
        expect(fingerprintMatches(fp1, fp2)).toBe(true);
    });

    it('fingerprintMatches is constant-time-safe for length mismatch', () => {
        expect(fingerprintMatches(new Uint8Array([1, 2]), new Uint8Array([1, 2, 3]))).toBe(false);
    });

    it('throws on invalid mnemonic when deriving keys', async () => {
        await expect(deriveKeys('not a valid phrase indeed')).rejects.toThrow(/Invalid/);
    });

    it('pickDistinctIndices returns n distinct indices in range', () => {
        const picks = pickDistinctIndices(3, 12);
        expect(picks).toHaveLength(3);
        expect(new Set(picks).size).toBe(3);
        for (const i of picks) {
            expect(i).toBeGreaterThanOrEqual(0);
            expect(i).toBeLessThan(12);
        }
    });
});
