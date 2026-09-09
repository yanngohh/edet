/**
 * Locale parity, as a gate rather than a habit.
 *
 * Every `$_(...)` call in this app carries an English `default`, so a
 * missing translation does not crash — it silently serves English to
 * someone who chose another language, which is exactly the kind of defect
 * that survives review. `contracts.sealedTitle` did: it sat in `en.json`
 * alone while five locales fell back, including on the wallet's own privacy
 * disclosure.
 *
 * Comparing the flattened key sets in both directions also catches the
 * opposite mistake — a key translated everywhere but dropped from `en`,
 * which would leave the reference file no longer describing the app.
 *
 * Key sets are only half of it, though, and the other half is the half that
 * rots. A key present in all six files says nothing about whether its
 * translation still means what the English means: an English string that
 * GAINS a sentence leaves every key check passing while five locales serve
 * the old meaning. That is not hypothetical —
 * the client-copy audit named it as the hole that
 * would go unnoticed "next time either". `fingerprint.json` closes it by
 * recording which English each translation was last written against; see
 * `scripts/locale-fingerprint.mjs`.
 */
import { describe, expect, it } from 'vitest';

// Suffixed because a bare `it` would shadow vitest's own `it`.
import { fingerprintOf, staleTranslations } from '../../../scripts/locale-fingerprint.mjs';
import { driftedDefaults } from '../../../scripts/locale-defaults.mjs';
import fingerprint from '../../locales/fingerprint.json';
import enLocale from '../../locales/en.json';
import itLocale from '../../locales/it.json';
import esLocale from '../../locales/es.json';
import deLocale from '../../locales/de.json';
import frLocale from '../../locales/fr.json';
import zhLocale from '../../locales/zh.json';

type Tree = { [k: string]: string | Tree };

function flatten(tree: Tree, prefix = ''): string[] {
    return Object.entries(tree).flatMap(([k, v]) => {
        const path = prefix ? `${prefix}.${k}` : k;
        return typeof v === 'string' ? [path] : flatten(v, path);
    });
}

const OTHERS: Array<[string, Tree]> = [
    ['it', itLocale as Tree],
    ['es', esLocale as Tree],
    ['de', deLocale as Tree],
    ['fr', frLocale as Tree],
    ['zh', zhLocale as Tree],
];

describe('locales', () => {
    const enKeys = flatten(enLocale as Tree);

    it.each(OTHERS)('%s covers every key in en, and adds none of its own', (_name, tree) => {
        const keys = flatten(tree);
        expect([...keys].sort()).toEqual([...enKeys].sort());
    });

    it('has no translation still written against older English', () => {
        // Recomputed from the locale files, never trusted from the recorded
        // file: the point is to compare what is in the tree now against what
        // the translations were last reviewed against.
        const current = fingerprintOf({
            en: enLocale,
            it: itLocale,
            es: esLocale,
            de: deLocale,
            fr: frLocale,
            zh: zhLocale,
        });
        const recorded = (fingerprint as { keys: Record<string, Record<string, string>> }).keys;

        const stale = staleTranslations(recorded, current);
        expect(
            stale,
            'English copy changed but these translations did not: ' +
                stale.map((s: { key: string; locales: string[] }) => `${s.key} (${s.locales.join(', ')})`).join('; ') +
                '. Update them, then `npm run locales:fingerprint`.',
        ).toEqual([]);

        // And the record itself must describe the tree — a key added or
        // removed in `en` without regenerating leaves it lying about
        // everything else it claims.
        expect(Object.keys(recorded).sort(), 'fingerprint.json is stale — run `npm run locales:fingerprint`').toEqual(
            Object.keys(current).sort(),
        );
    });

    // The third gate, and the one the first two could not have caught. Every
    // `$_()` call carries an inline `default:`; it renders whenever svelte-i18n
    // has no value for the key — before the bundle loads, and permanently if the
    // key is ever dropped — and it is the English a developer reads at the call
    // site. Nothing compared it to `en.json`, so the product tour went on
    // offering to "sponsor new admissions, promote members out of probation, and
    // vouch with a bonded stake" in `tour.ts` for a whole release after the
    // locale files were rewritten: three mechanisms this ledger does not have,
    // in the copy that teaches a newcomer what the system is.
    //
    // Sixteen had drifted when this was first run. Four were worse than drift —
    // keys that existed only in code, in NO locale file, which is precisely the
    // gap the key-set check above is blind to: a key absent everywhere is
    // consistent everywhere.
    it('has no inline default that disagrees with en.json', () => {
        const { drifted, checked, templates, unreadable } = driftedDefaults();
        expect(
            drifted,
            'inline `default:` copy has drifted from en.json: ' +
                drifted.map((d: { key: string; file: string }) => `${d.key} (${d.file})`).join('; '),
        ).toEqual([]);
        // Non-vacuity, and an honest account of what this reaches. Interpolated
        // defaults are compared too — masking `${…}` and
        // `{name}` to one marker compares the copy and ignores only the
        // placeholder names — and the first run of that found three, one of
        // them telling a member on the trade screen that a default "exposes
        // their sponsors", a mechanism deleted a release earlier.
        expect(checked).toBeGreaterThan(400);
        expect(templates).toBeGreaterThan(40);
        // What is left uncovered: a `default:` that is neither a literal nor a
        // template, i.e. assembled by code. There are none, and if one appears
        // this says so rather than counting it as checked.
        expect(unreadable).toBe(0);
    });

    it('translates the operation-bond strings everywhere', () => {
        // The bond wording is the one place the UI explains a mechanism a
        // member could otherwise mistake for a fee, so an untranslated
        // fallback here is worse than a cosmetic gap.
        const bondKeys = enKeys.filter((k) => k.startsWith('bond.') || k.includes('.bond'));
        expect(bondKeys.length).toBeGreaterThan(0);
        for (const [name, tree] of OTHERS) {
            const keys = new Set(flatten(tree));
            for (const k of bondKeys) expect(keys, `${name} is missing ${k}`).toContain(k);
        }
    });
});
