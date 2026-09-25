import { describe, expect, it } from 'vitest';

import { classifyAddressInput } from '../resolve';

describe('classifyAddressInput', () => {
    it('accepts wallet addresses with or without 0x, any case, padded', () => {
        const hex40 = 'AB'.repeat(20);
        expect(classifyAddressInput(`0x${hex40}`)).toEqual({ kind: 'address', clean: `0x${hex40.toLowerCase()}` });
        expect(classifyAddressInput(`  ${hex40}  `).kind).toBe('address');
    });

    it('accepts raw 64-hex public keys (restore/admission handles)', () => {
        const key = 'cd'.repeat(32);
        expect(classifyAddressInput(key)).toEqual({ kind: 'pubkey', clean: key });
        expect(classifyAddressInput(`0x${key}`).kind).toBe('pubkey');
    });

    it('rejects everything else', () => {
        expect(classifyAddressInput('').kind).toBe('invalid');
        expect(classifyAddressInput('0x1234').kind).toBe('invalid');
        expect(classifyAddressInput('not hex at all!').kind).toBe('invalid');
        expect(classifyAddressInput('zz'.repeat(20)).kind).toBe('invalid');
    });
});
