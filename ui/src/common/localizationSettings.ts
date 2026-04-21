import { writable, get } from 'svelte/store';

export interface LocalizationSettings {
    dateFormat: 'iso' | 'us' | 'eu';  // ISO: YYYY-MM-DD, US: MM/DD/YYYY, EU: DD/MM/YYYY
    timeFormat: '12h' | '24h';
    timezone: string;  // IANA timezone (e.g., 'America/New_York', 'Europe/Rome', 'UTC')
    numberFormat: 'dot-comma' | 'comma-dot';  // dot-comma: 1,234.56 | comma-dot: 1.234,56
    locale: string;
}

const DEFAULT_SETTINGS: LocalizationSettings = {
    dateFormat: 'iso',
    timeFormat: '24h',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
    numberFormat: 'dot-comma',
    locale: 'en'
};

function loadSettings(): LocalizationSettings {
    const stored = localStorage.getItem('edet-localization');
    if (stored) {
        try {
            return { ...DEFAULT_SETTINGS, ...JSON.parse(stored) };
        } catch (e) {
            console.error('Failed to parse localization settings:', e);
        }
    }
    return DEFAULT_SETTINGS;
}

function saveSettings(settings: LocalizationSettings) {
    localStorage.setItem('edet-localization', JSON.stringify(settings));
}

// Create the store
export const localizationSettings = writable<LocalizationSettings>(loadSettings());

// Subscribe to changes and save to localStorage
localizationSettings.subscribe(settings => {
    saveSettings(settings);
});

// Helper to update a single setting
export function updateSetting<K extends keyof LocalizationSettings>(
    key: K,
    value: LocalizationSettings[K]
) {
    localizationSettings.update(settings => ({
        ...settings,
        [key]: value
    }));
}

// Common timezone options grouped by region
export const TIMEZONE_OPTIONS = [
    { label: 'UTC', value: 'UTC' },
    { label: 'Auto-detect', value: 'auto' },
    { label: '─── Americas ───', value: '', disabled: true },
    { label: 'New York (EST/EDT)', value: 'America/New_York' },
    { label: 'Chicago (CST/CDT)', value: 'America/Chicago' },
    { label: 'Denver (MST/MDT)', value: 'America/Denver' },
    { label: 'Los Angeles (PST/PDT)', value: 'America/Los_Angeles' },
    { label: 'São Paulo', value: 'America/Sao_Paulo' },
    { label: '─── Europe ───', value: '', disabled: true },
    { label: 'London (GMT/BST)', value: 'Europe/London' },
    { label: 'Paris', value: 'Europe/Paris' },
    { label: 'Berlin', value: 'Europe/Berlin' },
    { label: 'Rome', value: 'Europe/Rome' },
    { label: 'Madrid', value: 'Europe/Madrid' },
    { label: 'Athens', value: 'Europe/Athens' },
    { label: 'Moscow', value: 'Europe/Moscow' },
    { label: '─── Asia ───', value: '', disabled: true },
    { label: 'Dubai', value: 'Asia/Dubai' },
    { label: 'Mumbai', value: 'Asia/Kolkata' },
    { label: 'Singapore', value: 'Asia/Singapore' },
    { label: 'Hong Kong', value: 'Asia/Hong_Kong' },
    { label: 'Tokyo', value: 'Asia/Tokyo' },
    { label: 'Seoul', value: 'Asia/Seoul' },
    { label: '─── Pacific ───', value: '', disabled: true },
    { label: 'Sydney', value: 'Australia/Sydney' },
    { label: 'Auckland', value: 'Pacific/Auckland' },
];

export const LOCALE_OPTIONS = [
    { label: 'English', value: 'en' },
    { label: 'Español', value: 'es' },
    { label: 'Français', value: 'fr' },
    { label: 'Italiano', value: 'it' },
    { label: 'Deutsch', value: 'de' },
    { label: '中文', value: 'zh' },
];
