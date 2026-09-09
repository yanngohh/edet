#!/usr/bin/env node
/**
 * The third locale gate: **the English in the code against the English in
 * `en.json`.**
 *
 * `locales.test.ts` compares key SETS across the six files, and
 * `fingerprint.json` catches a translation left behind by an English edit.
 * Neither looks at the string every `$_()` call carries inline as its
 * `default:` — and that string is not decoration. It is what renders whenever
 * svelte-i18n has no value for the key: before the locale bundle has loaded,
 * and for good if the key is ever dropped or renamed. It is also the English a
 * developer reads at the call site, which makes it the copy most likely to be
 * believed and least likely to be revisited.
 *
 * It went stale exactly that way. The v0.7.0 paper pass rewrote the product
 * tour in six locales and left `tour.ts`'s own defaults untouched, so the app
 * still carried "Sponsor new admissions, promote members out of probation, and
 * vouch with a bonded stake" — three mechanisms this ledger does not have — in
 * the file that defines the tour, one commit after a register entry recorded
 * that string as removed. `tour.walletMetrics.text` still promised "settled
 * history, and trust", and the trust term went with the fixed point.
 *
 * What it checks: for every `$_('key', { ..., default: '...' })` and every
 * `xKey`/`xDefault` pair, the inline text must equal `en.json`'s value for that
 * key.
 *
 * **Template literals are compared too.** Counting them as uncheckable — 52
 * of them — on the reasoning that `${formatNumber(x)}` cannot be compared
 * with `{amount}` textually would leave the copy in them ungated. Only
 * the placeholder cannot: everything between the placeholders is ordinary copy,
 * and it is the copy that goes stale. Masking each `${...}` and each `{name}`
 * to the same marker compares the fixed text and ignores only the names, which
 * legitimately differ. The first run of it found `myWallet.openDefaultNote`
 * telling a defaulter their capacity was "capped at the stake bonded on you" in
 * `en.json` and that "the backing it drew stays consumed" at the call site —
 * one of them describing a mechanism this ledger does not have. A gate that
 * reports what it does not cover is honest; it is not a gate over that part.
 *
 *   node scripts/locale-defaults.mjs [--check]
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = join(HERE, '..', 'src');
const EN = join(SRC, 'locales', 'en.json');

/**
 * A locale file as loaded: nested objects bottoming out in strings.
 * @typedef {{ [k: string]: string | LocaleTree }} LocaleTree
 * One inline default found at a call site.
 * @typedef {{ key: string, kind: 'literal'|'template'|'other', text: string|null, file: string }} Pair
 * One disagreement between the code and `en.json`.
 * @typedef {{ key: string, file: string, locale: string|null, inline: string }} Drift
 */

/**
 * Every `.svelte`/`.ts` file under `src`, excluding the locale files themselves.
 * @param {string} [root]
 * @returns {string[]}
 */
export function sourceFiles(root = SRC) {
    /** @type {string[]} */
    const out = [];
    /** @param {string} dir */
    const walk = (dir) => {
        for (const name of readdirSync(dir).sort()) {
            const p = join(dir, name);
            if (statSync(p).isDirectory()) {
                if (name !== 'locales' && name !== 'node_modules') walk(p);
            } else if (name.endsWith('.svelte') || name.endsWith('.ts')) {
                out.push(p);
            }
        }
    };
    walk(root);
    return out;
}

/**
 * Dotted lookup into the locale tree; `undefined` when the key is absent.
 * @param {LocaleTree} tree
 * @param {string} key
 * @returns {string|undefined}
 */
function lookup(tree, key) {
    /** @type {string | LocaleTree | undefined} */
    let cur = tree;
    for (const part of key.split('.')) {
        if (typeof cur !== 'object' || cur === null || !(part in cur)) return undefined;
        cur = cur[part];
    }
    return typeof cur === 'string' ? cur : undefined;
}

/**
 * Read one JS string literal starting at `i` (which must be a quote), or
 * `null` if it is a backtick. Returns `{ value, end }`.
 */
/**
 * @param {string} src
 * @param {number} i
 * @returns {{ value: string, end: number }|null}
 */
function readLiteral(src, i) {
    const quote = src[i];
    if (quote === '`') return null;
    if (quote !== "'" && quote !== '"') return null;
    let out = '';
    for (let j = i + 1; j < src.length; j++) {
        const ch = src[j];
        if (ch === '\\') {
            const next = src[j + 1];
            out += next === 'n' ? '\n' : next === 't' ? '\t' : next;
            j++;
            continue;
        }
        if (ch === quote) return { value: out, end: j + 1 };
        out += ch;
    }
    return null;
}

/** Adjacent literals joined by `+` — the prettier-wrapped long-string form. */
/**
 * @param {string} src
 * @param {number} i
 * @returns {{ value: string, end: number }|null}
 */
function readConcat(src, i) {
    let value = '';
    let cursor = i;
    for (;;) {
        while (/\s/.test(src[cursor])) cursor++;
        const lit = readLiteral(src, cursor);
        if (!lit) return value === '' ? null : { value, end: cursor };
        value += lit.value;
        cursor = lit.end;
        let look = cursor;
        while (/\s/.test(src[look])) look++;
        if (src[look] !== '+') return { value, end: cursor };
        cursor = look + 1;
    }
}

/**
 * Every (key, inline default) pair in one source file.
 *
 * Two call shapes, because the tour builds its steps as data rather than as
 * `$_()` calls and would otherwise be exactly the part nothing checked — which
 * is where the stale copy actually was.
 */
/**
 * @param {string} src
 * @param {string} file
 * @returns {Pair[]}
 */
export function pairsIn(src, file) {
    /** @type {Pair[]} */
    const found = [];
    // $_('key', { ... default: <literal|template> ... })
    const call = /\$_\(\s*(['"])([^'"]+)\1\s*,/g;
    for (let m; (m = call.exec(src)); ) {
        const key = m[2];
        const rest = src.slice(m.index, m.index + 4000);
        const d = /\bdefault:\s*/.exec(rest);
        if (!d) continue;
        const at = m.index + d.index + d[0].length;
        found.push({ key, ...classify(src, at), file });
    }
    // xKey: 'key', ... xDefault: <literal>
    const pair = /(\w+)Key:\s*(['"])([^'"]+)\2\s*,/g;
    for (let m; (m = pair.exec(src)); ) {
        const kind = m[1];
        const key = m[3];
        const rest = src.slice(m.index, m.index + 4000);
        const d = new RegExp(`\\b${kind}Default:\\s*`).exec(rest);
        if (!d) continue;
        const at = m.index + d.index + d[0].length;
        found.push({ key, ...classify(src, at), file });
    }
    return found;
}

/**
 * Read one template literal starting at its backtick, with every `${...}`
 * replaced by `MARK`. Brace depth is tracked so a nested object or a template
 * inside the expression cannot end it early.
 * @param {string} src
 * @param {number} i
 * @returns {{ value: string, end: number }|null}
 */
function readTemplate(src, i) {
    if (src[i] !== '`') return null;
    let out = '';
    for (let j = i + 1; j < src.length; j++) {
        const ch = src[j];
        if (ch === '\\') {
            const next = src[j + 1];
            out += next === 'n' ? '\n' : next === 't' ? '\t' : next;
            j++;
            continue;
        }
        if (ch === '`') return { value: out, end: j + 1 };
        if (ch === '$' && src[j + 1] === '{') {
            let depth = 1;
            let k = j + 2;
            for (; k < src.length && depth > 0; k++) {
                if (src[k] === '{') depth++;
                else if (src[k] === '}') depth--;
                else if (src[k] === '`') {
                    const inner = readTemplate(src, k);
                    if (!inner) return null;
                    k = inner.end - 1;
                }
            }
            if (depth > 0) return null;
            out += MARK;
            j = k - 1;
            continue;
        }
        out += ch;
    }
    return null;
}

/**
 * @param {string} src
 * @param {number} at
 * @returns {{ kind: 'literal'|'template'|'other', text: string|null }}
 */
function classify(src, at) {
    let i = at;
    while (/\s/.test(src[i])) i++;
    if (src[i] === '`') {
        const t = readTemplate(src, i);
        return t ? { kind: 'template', text: t.value } : { kind: 'other', text: null };
    }
    const lit = readConcat(src, i);
    return lit ? { kind: 'literal', text: lit.value } : { kind: 'other', text: null };
}

/** Stands in for one interpolation on either side of the comparison. */
const MARK = '\u0000';

/** Collapse the whitespace prettier introduced when it wrapped a long string. */
/** @param {string} s */
const norm = (s) => s.replace(/\s+/g, ' ').trim();

/** `en.json`'s ICU placeholders, masked to the same marker as `${...}`. */
/** @param {string} s */
const mask = (s) => s.replace(/\{[^{}]*\}/g, MARK);

/**
 * Every inline default that disagrees with `en.json`, plus how many of each
 * kind were reached — and the count of the ones nothing here can read at all,
 * which is a default assembled by code rather than written as copy.
 */
/** @returns {{ drifted: Drift[], templates: number, checked: number, unreadable: number }} */
export function driftedDefaults() {
    const en = /** @type {LocaleTree} */ (JSON.parse(readFileSync(EN, 'utf8')));
    /** @type {Drift[]} */
    const drifted = [];
    let templates = 0;
    let checked = 0;
    let unreadable = 0;
    for (const file of sourceFiles()) {
        const src = readFileSync(file, 'utf8');
        for (const p of pairsIn(src, file)) {
            if (p.kind === 'other' || p.text === null) {
                unreadable++;
                continue;
            }
            const value = lookup(en, p.key);
            checked++;
            if (p.kind === 'template') templates++;
            // The marker is what makes the two sides comparable: `${…}` on one
            // side and `{name}` on the other stand for the same hole, and only
            // the copy around them is a claim.
            const inline = norm(p.text);
            if (value === undefined) {
                drifted.push({ key: p.key, file: relative(SRC, file), locale: null, inline });
            } else if (mask(norm(value)) !== inline) {
                drifted.push({ key: p.key, file: relative(SRC, file), locale: mask(norm(value)), inline });
            }
        }
    }
    return { drifted, templates, checked, unreadable };
}

if (process.argv[1] && process.argv[1].endsWith('locale-defaults.mjs')) {
    const { drifted, templates, checked, unreadable } = driftedDefaults();
    const show = (/** @type {string} */ s) => s.replace(new RegExp(MARK, 'g'), '⟨…⟩');
    for (const d of drifted) {
        console.log(`\n${d.key}  (${d.file})`);
        console.log(`  en.json: ${d.locale === null ? '<no such key>' : show(d.locale)}`);
        console.log(`  inline : ${show(d.inline)}`);
    }
    console.log(
        `\n${checked} inline defaults compared (${templates} of them interpolated), ` +
            `${unreadable} not written as copy, ${drifted.length} drifted.`,
    );
    process.exit(drifted.length === 0 ? 0 : 1);
}
