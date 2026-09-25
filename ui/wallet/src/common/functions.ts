import { get } from 'svelte/store';
import { localizationSettings } from './localizationSettings';

/** Format a millisecond timestamp to a localized date and time string. */
export function formatDateTime(timestampMs: number): string {
    return `${formatDate(timestampMs)} ${formatTime(timestampMs)}`;
}

/** Format a millisecond timestamp to a localized date string. */
export function formatDate(timestampMs: number): string {
    const settings = get(localizationSettings);
    const date = new Date(timestampMs);

    const tz = settings.timezone === 'auto'
        ? Intl.DateTimeFormat().resolvedOptions().timeZone
        : settings.timezone;

    const year = date.toLocaleString('en-US', { year: 'numeric', timeZone: tz });
    const month = date.toLocaleString('en-US', { month: '2-digit', timeZone: tz });
    const day = date.toLocaleString('en-US', { day: '2-digit', timeZone: tz });

    switch (settings.dateFormat) {
        case 'us':
            return `${month}/${day}/${year}`;
        case 'eu':
            return `${day}/${month}/${year}`;
        case 'iso':
        default:
            return `${year}-${month}-${day}`;
    }
}

/** Format a millisecond timestamp to a localized time string. */
export function formatTime(timestampMs: number): string {
    const settings = get(localizationSettings);
    const date = new Date(timestampMs);

    const tz = settings.timezone === 'auto'
        ? Intl.DateTimeFormat().resolvedOptions().timeZone
        : settings.timezone;

    const options: Intl.DateTimeFormatOptions = {
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        hour12: settings.timeFormat === '12h',
        timeZone: tz
    };

    return date.toLocaleString('en-US', options);
}

/** Format a number with localized decimal and thousand separators. */
export function formatNumber(value: number | string, decimals: number = 2): string {
    const settings = get(localizationSettings);

    const numValue = typeof value === 'string' ? parseFloat(value) : value;

    if (isNaN(numValue)) return "0";

    const rounded = Number(numValue.toFixed(decimals));

    const parts = rounded.toFixed(decimals).split('.');
    const integerPart = parts[0];
    const decimalPart = parts[1];

    let formattedInteger: string;
    if (settings.numberFormat === 'dot-comma') {
        // US format: 1,234.56
        formattedInteger = integerPart.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
        return decimalPart ? `${formattedInteger}.${decimalPart}` : formattedInteger;
    } else {
        // EU format: 1.234,56
        formattedInteger = integerPart.replace(/\B(?=(\d{3})+(?!\d))/g, '.');
        return decimalPart ? `${formattedInteger},${decimalPart}` : formattedInteger;
    }
}

/** Format a percentage value. */
export function formatPercentage(value: number, decimals: number = 1): string {
    return formatNumber(value, decimals) + '%';
}

/** Parse a localized number string into a numeric value. */
export function parseNumber(input: string): number {
    const settings = get(localizationSettings);
    if (!input) return 0;

    let sanitized = input.trim();

    if (settings.numberFormat === 'comma-dot') {
        // EU: 1.234,56 -> remove dots, replace comma with dot
        sanitized = sanitized.replace(/\./g, '').replace(',', '.');
    } else {
        // US: 1,234.56 -> remove commas
        sanitized = sanitized.replace(/,/g, '');
    }

    return parseFloat(sanitized);
}

/**
 * Validate if a string is a valid number according to localized settings.
 * Strict about thousand separators to prevent dangerous misinterpretations.
 */
export function isValidNumberFormat(input: string): boolean {
    if (!input || input.trim() === "") return false;

    const settings = get(localizationSettings);
    const val = input.trim();

    if (settings.numberFormat === 'comma-dot') {
        // EU: 1.234,56 — only digits, dots, and one comma.
        if (/[^0-9.,]/.test(val)) return false;

        const commaCount = (val.match(/,/g) || []).length;
        if (commaCount > 1) return false;

        // If a dot is used, it must look like a thousand separator.
        if (val.includes('.')) {
            const dotIndices = [];
            for (let i = 0; i < val.length; i++) if (val[i] === '.') dotIndices.push(i);

            for (const idx of dotIndices) {
                const afterDot = val.substring(idx + 1).split(/[.,]/)[0];
                if (afterDot.length !== 3) return false;
            }
        }
    } else {
        // US: 1,234.56
        if (/[^0-9.,]/.test(val)) return false;

        const dotCount = (val.match(/\./g) || []).length;
        if (dotCount > 1) return false;

        if (val.includes(',')) {
            const commaIndices = [];
            for (let i = 0; i < val.length; i++) if (val[i] === ',') commaIndices.push(i);

            for (const idx of commaIndices) {
                const afterComma = val.substring(idx + 1).split(/[.,]/)[0];
                if (afterComma.length !== 3) return false;
            }
        }
    }

    const parsed = parseNumber(val);
    return !isNaN(parsed) && isFinite(parsed);
}

/**
 * Clean input string to only allow valid characters for the current locale.
 * Replaces the "wrong" decimal separator if it's clearly intended as one.
 */
export function cleanNumberInput(input: string): string {
    const settings = get(localizationSettings);

    if (settings.numberFormat === 'comma-dot') {
        // EU mode: auto-swap dot to comma, digits + one comma only.
        let cleaned = input.replace(/\./g, ',');
        cleaned = cleaned.replace(/[^0-9,]/g, '');
        const parts = cleaned.split(',');
        if (parts.length > 2) {
            cleaned = parts[0] + ',' + parts.slice(1).join('');
        }
        return cleaned;
    } else {
        // US mode: auto-swap comma to dot, digits + one dot only.
        let cleaned = input.replace(/,/g, '.');
        cleaned = cleaned.replace(/[^0-9.]/g, '');
        const parts = cleaned.split('.');
        if (parts.length > 2) {
            cleaned = parts[0] + '.' + parts.slice(1).join('');
        }
        return cleaned;
    }
}
