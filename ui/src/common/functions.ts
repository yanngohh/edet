import { HolochainError, decodeHashFromBase64, encodeHashToBase64, type AppClient } from "@holochain/client";
import { get } from 'svelte/store'
import { _, format, locale } from 'svelte-i18n'
import { localizationSettings, type LocalizationSettings } from './localizationSettings';

/**
 * Format a timestamp to a localized date and time string
 */
export function formatDateTime(timestamp: number): string {
    const settings = get(localizationSettings);
    const date = new Date(timestamp / 1000);

    // Handle timezone
    const tz = settings.timezone === 'auto'
        ? Intl.DateTimeFormat().resolvedOptions().timeZone
        : settings.timezone;

    const dateStr = formatDate(timestamp);
    const timeStr = formatTime(timestamp);

    return `${dateStr} ${timeStr}`;
}

/**
 * Format a timestamp to a localized date string
 */
export function formatDate(timestamp: number): string {
    const settings = get(localizationSettings);
    const date = new Date(timestamp / 1000);

    const tz = settings.timezone === 'auto'
        ? Intl.DateTimeFormat().resolvedOptions().timeZone
        : settings.timezone;

    const year = date.toLocaleString('en-US', { year: 'numeric', timeZone: tz });
    const month = date.toLocaleString('en-US', { month: '2-digit', timeZone: tz });
    const day = date.toLocaleString('en-US', { day: '2-digit', timeZone: tz });

    switch (settings.dateFormat) {
        case 'iso':
            return `${year}-${month}-${day}`;
        case 'us':
            return `${month}/${day}/${year}`;
        case 'eu':
            return `${day}/${month}/${year}`;
        default:
            return `${year}-${month}-${day}`;
    }
}

/**
 * Format a timestamp to a localized time string
 */
export function formatTime(timestamp: number): string {
    const settings = get(localizationSettings);
    const date = new Date(timestamp / 1000);

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

/**
 * Format a number with localized decimal and thousand separators
 */
export function formatNumber(value: number | string, decimals: number = 2): string {
    const settings = get(localizationSettings);

    // Ensure we have a number
    const numValue = typeof value === 'string' ? parseFloat(value) : value;

    if (isNaN(numValue)) return "0";

    // Round to specified decimals
    const rounded = Number(numValue.toFixed(decimals));

    // Split into integer and decimal parts
    const parts = rounded.toFixed(decimals).split('.');
    const integerPart = parts[0];
    const decimalPart = parts[1];

    // Add thousand separators
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

/**
 * Format a percentage value
 */
export function formatPercentage(value: number, decimals: number = 1): string {
    return formatNumber(value, decimals) + '%';
}

/**
 * Parse a localized number string into a numeric value
 */
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
 * Becomes very strict about thousand separators to prevent dangerous misinterpretations.
 */
export function isValidNumberFormat(input: string): boolean {
    if (!input || input.trim() === "") return false;

    const settings = get(localizationSettings);
    const val = input.trim();

    if (settings.numberFormat === 'comma-dot') {
        // EU: 1.234,56
        // Only allow digits, dots, and one comma
        if (/[^0-9.,]/.test(val)) return false;

        const commaCount = (val.match(/,/g) || []).length;
        if (commaCount > 1) return false;

        // If a dot is used, it MUST be followed by exactly 3 digits (standard thousand separator rule)
        // or we simply block it in input to be safe if it's ambiguous.
        // For now, let's be strict: if there's a dot, it must look like a thousand separator.
        if (val.includes('.')) {
            const dotIndices = [];
            for (let i = 0; i < val.length; i++) if (val[i] === '.') dotIndices.push(i);

            for (const idx of dotIndices) {
                // If the dot is followed by something that isn't exactly 3 digits (until the next separator or end)
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
        // EU Mode: Replace dot with comma if it's likely a decimal separator
        // and filter out anything else that isn't a digit or comma.
        // We block the thousand separator dot during LIVE typing to avoid ambiguity.
        let cleaned = input.replace(/\./g, ','); // Auto-swap dot to comma
        cleaned = cleaned.replace(/[^0-9,]/g, '');
        // Keep only the first comma
        const parts = cleaned.split(',');
        if (parts.length > 2) {
            return parts[0] + ',' + parts.slice(1).join('');
        }
        return cleaned;
    } else {
        // US Mode: Replace comma with dot if it's likely a decimal separator
        let cleaned = input.replace(/,/g, '.'); // Auto-swap comma to dot
        cleaned = cleaned.replace(/[^0-9.]/g, '');
        // Keep only the first dot
        const parts = cleaned.split('.');
        if (parts.length > 2) {
            return parts[0] + '.' + parts.slice(1).join('');
        }
        return cleaned;
    }
}

/**
 * Legacy function for backward compatibility (deprecated)
 * @deprecated Use formatDateTime instead
 */
export function formatTimestampToLocalDateTime(timestamp: number): string {
    return formatDateTime(timestamp);
}

/**
 * Extract a structured error code from a Holochain wasm error message string.
 *
 * Holochain surfaces wasm errors as a long message of the form:
 *   "internal_error Wasm runtime error while working with Ribosome: RuntimeError:
 *    WasmError { file: "...", line: N, error: Guest("EC200019") }"
 *
 * A naive `lastIndexOf(":")` extracts `Guest("EC200019") }` instead of `EC200019`,
 * causing i18n lookups to always miss and the raw wasm dump to be shown to users.
 *
 * This function tries the wasm pattern first, then falls back to the simple
 * colon-split for plain string errors.
 */
export function extractHolochainErrorCode(message: string): string {
    // Primary: match Guest("ECXXXXXX") or Guest("EVXXXXXX") in the wasm error format
    const guestMatch = message.match(/Guest\("([^"]+)"\)/);
    if (guestMatch) return guestMatch[1];

    // Fallback: for plain "something: CODE" error strings
    return message.substring(message.lastIndexOf(':') + 1).trim();
}

export function isValidAddress(address: string): boolean {
    let valid = false;
    try {
        valid = address.startsWith("uhCAk") && decodeHashFromBase64(address).length === 39;
    } catch (_) {
    }
    return valid;
}

export async function now(client: AppClient): Promise<number> {
    const sysTime = await client.callZome({
        role_name: 'edet',
        zome_name: 'support',
        fn_name: 'get_sys_time',
        payload: null
    });
    return Math.trunc(sysTime / 1000);
}