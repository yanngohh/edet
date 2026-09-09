#!/usr/bin/env node
/**
 * Regenerate `src/locales/fingerprint.json` — the record of which English
 * copy each translation was last written against.
 *
 * Why this exists. `locales.test.ts` compares KEY SETS, which catches a
 * string nobody translated at all and cannot catch the far commoner rot: an
 * English string that GREW while the five translations stayed as they were.
 * The key is present everywhere, every check passes, and five of six locales
 * quietly serve last month's meaning — a hole a key-set comparison cannot
 * see, ever.
 *
 * What it gives: editing an English string makes the fingerprint stale, and
 * `locales.test.ts` fails naming exactly which keys moved. Regenerating is
 * refused unless every locale's own text for those keys changed too — so the
 * reflex "test failed, re-run the generator" does not work. You have to open
 * the five files.
 *
 * What it does NOT give: any evidence a translation is CORRECT, only that it
 * was revisited in the same change. `--accept-en-only` waives even that, for
 * an English edit that genuinely cannot change meaning (a typo, punctuation).
 * Reaching for it on a content change is precisely how the five go stale, so
 * it is a deliberate, greppable act rather than a default.
 *
 *   node scripts/locale-fingerprint.mjs [--accept-en-only]
 */
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const LOCALES_DIR = join(HERE, '..', 'src', 'locales');
const FINGERPRINT = join(LOCALES_DIR, 'fingerprint.json');

/**
 * A locale file as loaded: nested objects bottoming out in strings.
 * @typedef {{ [k: string]: string | LocaleTree }} LocaleTree
 * Dotted key -> per-locale digest of that key's text.
 * @typedef {Record<string, Record<string, string>>} Fingerprint
 */

/** `en` first: it is the source every other locale is a translation of. */
export const LOCALE_NAMES = ['en', 'it', 'es', 'de', 'fr', 'zh'];

/**
 * Dotted key -> string, for one locale tree.
 * @param {LocaleTree} tree
 * @param {string} [prefix]
 * @returns {Record<string, string>}
 */
export function flatten(tree, prefix = '') {
    /** @type {Record<string, string>} */
    const out = {};
    for (const [k, v] of Object.entries(tree)) {
        const path = prefix ? `${prefix}.${k}` : k;
        if (typeof v === 'string') out[path] = v;
        else Object.assign(out, flatten(v, path));
    }
    return out;
}

/**
 * Short digest — collision resistance is not the property in question here;
 * "did this exact string change" is.
 * @param {string} s
 * @returns {string}
 */
export function digest(s) {
    return createHash('sha256').update(s, 'utf8').digest('hex').slice(0, 16);
}

/**
 * `{ key: { en, it, es, de, fr, zh } }` for every key present in `en`.
 * @param {Record<string, LocaleTree>} locales
 * @returns {Fingerprint}
 */
export function fingerprintOf(locales) {
    /** @type {Record<string, Record<string, string>>} */
    const flat = Object.fromEntries(LOCALE_NAMES.map((n) => [n, flatten(locales[n])]));
    /** @type {Fingerprint} */
    const out = {};
    for (const key of Object.keys(flat.en)) {
        out[key] = {};
        for (const name of LOCALE_NAMES) {
            const value = flat[name][key];
            if (value !== undefined) out[key][name] = digest(value);
        }
    }
    return out;
}

/**
 * Keys whose English changed since `recorded` while some locale's own text
 * did not — the ones a regeneration must refuse until they are revisited.
 * @param {Fingerprint} recorded
 * @param {Fingerprint} current
 * @returns {{ key: string, locales: string[] }[]}
 */
export function staleTranslations(recorded, current) {
    /** @type {{ key: string, locales: string[] }[]} */
    const stale = [];
    for (const [key, now] of Object.entries(current)) {
        const before = recorded[key];
        if (!before || before.en === now.en) continue; // new key, or English unchanged
        const untouched = LOCALE_NAMES.filter((n) => n !== 'en' && before[n] && before[n] === now[n]);
        if (untouched.length > 0) stale.push({ key, locales: untouched });
    }
    return stale;
}

function main() {
    const acceptEnOnly = process.argv.includes('--accept-en-only');
    /** @type {Record<string, LocaleTree>} */
    const locales = Object.fromEntries(
        LOCALE_NAMES.map((n) => [n, JSON.parse(readFileSync(join(LOCALES_DIR, `${n}.json`), 'utf8'))]),
    );
    const current = fingerprintOf(locales);

    /** @type {Fingerprint} */
    let recorded = {};
    try {
        recorded = JSON.parse(readFileSync(FINGERPRINT, 'utf8')).keys ?? {};
    } catch {
        // First run: nothing recorded yet, so nothing can be stale.
    }

    const stale = staleTranslations(recorded, current);
    if (stale.length > 0 && !acceptEnOnly) {
        console.error('English copy changed without the translations being revisited:\n');
        for (const { key, locales: names } of stale) {
            console.error(`  ${key}\n    still on the old English in: ${names.join(', ')}`);
        }
        console.error(
            '\nUpdate those translations, then re-run this. If the English edit genuinely\n' +
                'cannot change meaning (a typo, punctuation), re-run with --accept-en-only.',
        );
        process.exit(1);
    }

    const body = {
        note:
            'Generated by scripts/locale-fingerprint.mjs — which English copy each translation was ' +
            'last written against. Do not hand-edit; see that script and lib/__tests__/locales.test.ts.',
        keys: current,
    };
    writeFileSync(FINGERPRINT, `${JSON.stringify(body, null, 2)}\n`);
    const n = Object.keys(current).length;
    console.log(`wrote ${FINGERPRINT} (${n} keys)${acceptEnOnly && stale.length ? `, ${stale.length} accepted as English-only` : ''}`);
}

// Importable by the test (which recomputes rather than trusting the file),
// runnable as a script.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
