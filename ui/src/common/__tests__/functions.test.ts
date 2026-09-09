import { beforeEach, describe, expect, it } from 'vitest';

import { localizationSettings, updateSetting } from '../localizationSettings';
import { cleanNumberInput, formatNumber, isValidNumberFormat, parseNumber } from '../functions';

beforeEach(() => {
    updateSetting('numberFormat', 'dot-comma');
});

describe('formatNumber', () => {
    it('formats US style with thousand separators', () => {
        expect(formatNumber(1234.5)).toBe('1,234.50');
        expect(formatNumber(0.005, 2)).toBe('0.01');
    });

    it('formats EU style when comma-dot is selected', () => {
        updateSetting('numberFormat', 'comma-dot');
        expect(formatNumber(1234.5)).toBe('1.234,50');
    });
});

describe('parseNumber', () => {
    it('round-trips its own format in both locales', () => {
        expect(parseNumber(formatNumber(1234.56))).toBeCloseTo(1234.56);
        updateSetting('numberFormat', 'comma-dot');
        expect(parseNumber(formatNumber(1234.56))).toBeCloseTo(1234.56);
    });
});

describe('isValidNumberFormat', () => {
    it('rejects ambiguous thousand separators', () => {
        expect(isValidNumberFormat('1,23')).toBe(false);
        expect(isValidNumberFormat('1,234.56')).toBe(true);
    });
});

describe('cleanNumberInput', () => {
    it('swaps the wrong decimal separator while typing', () => {
        expect(cleanNumberInput('12,5')).toBe('12.5');
        updateSetting('numberFormat', 'comma-dot');
        expect(cleanNumberInput('12.5')).toBe('12,5');
    });
});
