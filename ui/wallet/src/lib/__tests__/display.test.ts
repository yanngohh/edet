import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';

import { addressOf, amountsSealed, formatAmount, memberName, shortAddress } from '../display';
import { nicknames, setNickname } from '../actors';
import { membersList, paramsView } from '../node';
import type { MemberSummary, ParamsView } from '../api';

const member = (id: number, address: string): MemberSummary => ({
    id,
    address,
    keys: ['aa'.repeat(32)],
    status: 'active',
    capacity: 0,
    debt: 0,
    open_default: 0,
    d_in: 0,
    d_out: 0,
});

const ADDR = '0x1a2b3c4d5e6f708192a3b4c5d6e7f80912345678';

beforeEach(() => {
    window.localStorage.clear();
    nicknames.set({});
    membersList.set([member(0, ADDR)]);
    paramsView.set(null);
});

describe('shortAddress', () => {
    it('compacts long addresses and passes null through', () => {
        expect(shortAddress(ADDR)).toBe('0x1a2b3c…5678');
        expect(shortAddress(null)).toBeNull();
        expect(shortAddress('0xshort')).toBe('0xshort');
    });
});

describe('addressOf', () => {
    it('resolves a member id to its ledger address', () => {
        expect(get(addressOf)(0)).toBe(ADDR);
        expect(get(addressOf)(99)).toBeNull();
    });
});

const paramsWithSeal = (value: number): ParamsView => ({
    governed: [{ key: 'SealAmounts', value, min: 0, max: 1, last_amend_epoch: null }],
    v_base: 0,
    dust: 0.01,
    epoch_secs: 60,
    min_maturity_epochs: 1,
    insured_horizon_epochs: 365,
    gov_cooldown_epochs: 1,
    last_redenom_epoch: null,
});

describe('amountsSealed', () => {
    it('is false before the governed SealAmounts param crosses 0.5', () => {
        paramsView.set(paramsWithSeal(0.4));
        expect(get(amountsSealed)).toBe(false);
        paramsView.set(null);
        expect(get(amountsSealed)).toBe(false);
    });

    it('is true once SealAmounts is armed (>= 0.5)', () => {
        paramsView.set(paramsWithSeal(0.5));
        expect(get(amountsSealed)).toBe(true);
        paramsView.set(paramsWithSeal(1));
        expect(get(amountsSealed)).toBe(true);
    });
});

describe('formatAmount', () => {
    const fmt = (n: number) => n.toFixed(2);

    it('marks the value as approximate only when sealed', () => {
        expect(formatAmount(fmt, 30, false)).toBe('30.00');
        expect(formatAmount(fmt, 30, true)).toBe('≈ 30.00');
    });
});

describe('memberName', () => {
    it('prefers nickname, then short address, then #id', () => {
        expect(get(memberName)(0)).toBe('0x1a2b3c…5678');
        setNickname(0, 'Ada');
        expect(get(memberName)(0)).toBe('Ada');
        // Unknown member (not on the ledger / list not loaded): #id fallback.
        expect(get(memberName)(99)).toBe('#99');
    });
});
