/**
 * **The member's own price on a counterparty**, beside the ledger's.
 *
 * `risk.ts` computes the protocol's advisory score, term for term with
 * `crates/kernel/src/risk.rs`. That score is the community's reading, and a
 * client that quietly scored differently would be automating a decision the
 * protocol never described. This module is the other half: a member may say
 * what they think, and the app then shows BOTH — "the ledger scores this X;
 * your rule says Y" — rather than replacing one with the other.
 *
 * Two dials, and no more:
 *
 *   * a **personal K**, in place of the governed one. K is the scale
 *     confidence is measured on, so raising it says "I want more backing
 *     behind somebody before I call them safe" and lowering it says the
 *     opposite. It is the one term of the score whose meaning is a judgement
 *     rather than a fact.
 *   * a **per-counterparty offset** in [−0.5, +0.5] on the resulting score:
 *     what this member knows about that member and the ledger does not.
 *
 * No per-underwriter weighting, deliberately. A wallet cannot see the shares
 * behind a capacity figure — `capacity` is a max-flow and what it discloses is
 * its value, not its routes — so a dial for "I trust this underwriter less"
 * would be a control over a quantity the client does not hold.
 *
 * **It can only ever narrow what is automated.** `autosign` classifies the
 * subjective score, and every guard it already applies stays: the debtor side
 * is never signed, `maxAmount` still holds, a cold start is still held for a
 * human, and the dry run still has to pass. A rule that made a request MORE
 * acceptable than the ledger's own score still meets all of those.
 *
 * Device-only, like the acceptance policy and for the same reason: it is
 * policy rather than ledger state, nobody else can read or enforce it, and it
 * is not part of the encrypted identity backup, which carries only what cannot
 * be recreated.
 */

import { writable } from 'svelte/store';

import { lsGet, lsSet } from '../common/safeStorage';
import type { ParamsView } from './api';
import { memberRisk, riskK, vBase, type RiskInputs } from './risk';

const KEY = 'edet-subjective-pricing';

/** The widest an offset may move a score, either way. */
export const MAX_ADJUSTMENT = 0.5;

export interface SubjectivePricing {
    /**
     * The member's own K, or `null` to use the governed one. Never zero or
     * negative: K is a scale, and a scale of nothing makes every account with
     * any capacity at all read as fully confident.
     */
    personalK: number | null;
    /** Per-counterparty offset on the score, by member id, in [−0.5, +0.5]. */
    adjustments: Record<number, number>;
}

export const DEFAULT_PRICING: SubjectivePricing = { personalK: null, adjustments: {} };

/** Clamp a stored rule to one that means something. */
export function normalizePricing(p: Partial<SubjectivePricing> | null | undefined): SubjectivePricing {
    const k = p?.personalK;
    const personalK = typeof k === 'number' && Number.isFinite(k) && k > 0 ? k : null;
    const adjustments: Record<number, number> = {};
    for (const [id, value] of Object.entries(p?.adjustments ?? {})) {
        const member = Number(id);
        if (!Number.isInteger(member) || member < 0) continue;
        if (typeof value !== 'number' || !Number.isFinite(value)) continue;
        // A zero offset is the absence of one: keeping it would grow the
        // stored rule with every counterparty the member ever looked at.
        const clamped = Math.min(MAX_ADJUSTMENT, Math.max(-MAX_ADJUSTMENT, value));
        if (clamped !== 0) adjustments[member] = clamped;
    }
    return { personalK, adjustments };
}

function load(): SubjectivePricing {
    const raw = lsGet(KEY);
    if (!raw) return { ...DEFAULT_PRICING, adjustments: {} };
    try {
        return normalizePricing(JSON.parse(raw));
    } catch {
        return { ...DEFAULT_PRICING, adjustments: {} };
    }
}

export const subjectivePricing = writable<SubjectivePricing>(load());

subjectivePricing.subscribe((p) => lsSet(KEY, JSON.stringify(p)));

/** Persist a new rule (normalized). */
export function setPricing(p: Partial<SubjectivePricing>): SubjectivePricing {
    const next = normalizePricing(p);
    subjectivePricing.set(next);
    return next;
}

/** Back to the ledger's own reading, everywhere. */
export function resetPricing(): SubjectivePricing {
    return setPricing(DEFAULT_PRICING);
}

/** Set (or clear, with 0) one counterparty's offset. */
export function priceCounterparty(
    current: SubjectivePricing,
    member: number,
    offset: number,
): SubjectivePricing {
    return setPricing({ ...current, adjustments: { ...current.adjustments, [member]: offset } });
}

/** Has the member said anything at all? What the UI shows both scores for. */
export function hasRule(p: SubjectivePricing, member?: number): boolean {
    if (p.personalK !== null) return true;
    return member === undefined ? Object.keys(p.adjustments).length > 0 : (p.adjustments[member] ?? 0) !== 0;
}

/**
 * The member's own score for one counterparty.
 *
 * **With no rule, this is the kernel's score exactly** — not an
 * approximation of it, and not a second implementation: it calls the same
 * `memberRisk` with the same governed K, so a wallet that has said nothing
 * decides on what the ledger says and nothing else.
 */
export function subjectiveRisk(
    member: number,
    row: RiskInputs,
    governedK: number,
    v: number,
    pricing: SubjectivePricing,
): number {
    const base = memberRisk(row, pricing.personalK ?? governedK, v);
    const offset = pricing.adjustments[member] ?? 0;
    return Math.min(1, Math.max(0, base + offset));
}

/** The same, from the served parameters — what a view has in hand. */
export function subjectiveRiskOf(
    member: number,
    row: RiskInputs,
    params: ParamsView | null,
    pricing: SubjectivePricing,
): number {
    return subjectiveRisk(member, row, riskK(params), vBase(params), pricing);
}
