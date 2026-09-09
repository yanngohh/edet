/**
 * Every refusal the ledger can return has words, and no words describe a
 * refusal it cannot — the client-copy audit.
 *
 * `crates/state/src/errors.rs` is the normative list. Two ways this drifts and
 * they are not equally bad:
 *
 *   * A ledger code with **no string** degrades to the raw `ET-…`, which is
 *     ugly and reportable. That is polish.
 *   * A string for a code the ledger cannot return is **worse than none**: it
 *     survives the mechanism it described and says something false the first
 *     time somebody hits a reused number. `ET-ADM-003` did exactly that — it
 *     read "An admission needs at least one sponsor" long after admissions were
 *     deleted, while the code had become the key-list bound that moved onto
 *     `RotateRequest`. `ET-CND-001..005` described conditional legs the design
 *     refused and never built.
 *
 * So this reads both sides. It parses the Rust rather than pinning a copied
 * list: a cross-pin whose oracle is hand-copied is pinned to whatever was last
 * pasted, which is the one failure a cross-pin exists to be immune to.
 */

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import en from '../../locales/en.json';

const HERE = dirname(fileURLToPath(import.meta.url));
const ERRORS_RS = join(HERE, '..', '..', '..', '..', 'crates', 'state', 'src', 'errors.rs');

/** Every `ET-…` code the ledger declares. */
function ledgerCodes(): Set<string> {
    const src = readFileSync(ERRORS_RS, 'utf8');
    // Only `pub const … = "ET-…"` declarations, so a code merely MENTIONED in a
    // comment — `ET-ADM-004` is deliberately retired and named in one, so that
    // a stale client asserting on it finds nothing rather than something else —
    // is not counted as live.
    return new Set([...src.matchAll(/pub const \w+: Code = "(ET-[A-Z]+-\d+)"/g)].map((m) => m[1]));
}

const clientCodes = new Set(Object.keys((en as { errors: Record<string, string> }).errors).filter((k) => k.startsWith('ET-')));

describe('refusal codes, both ways', () => {
    it('every refusal the ledger can return has words', () => {
        const missing = [...ledgerCodes()].filter((c) => !clientCodes.has(c)).sort();
        expect(missing, 'add these to every locale').toEqual([]);
    });

    it('no words describe a refusal the ledger cannot return', () => {
        const orphan = [...clientCodes].filter((c) => !ledgerCodes().has(c)).sort();
        expect(orphan, 'these outlived their mechanism — delete them').toEqual([]);
    });

    it('reads a normative list rather than a copied one', () => {
        // A guard on the guard: if the parse ever stops matching, both
        // assertions above pass vacuously and this suite becomes decoration.
        expect(ledgerCodes().size).toBeGreaterThan(30);
        expect(ledgerCodes().has('ET-BND-004')).toBe(true);
    });
});
