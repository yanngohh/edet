/**
 * Member display identity: wallet-like hex addresses + local nicknames.
 *
 * On the ledger a member is a numeric account id, local to one community; the
 * node derives a `0x…` address for each member from their KEY and from nothing
 * else, because an id is dense and per-community — id 7 here and id 7 there are
 * different people, so an address derived from one resolves, in the other
 * community, to somebody else.
 *
 * **An address therefore does NOT survive a guardian rotation.** The old
 * derivation used the admission attestation and did; attestations went with
 * admission, and nothing is left that is both immutable and globally unique to
 * hang it on. A member who rotates must be re-scanned, exactly as if they had
 * moved. This module is the one place that decides how a member is shown:
 *
 *   nickname (local, this device only)  >  short address  >  #id fallback
 *
 * Lives apart from actors.ts/node.ts because it needs both (nicknames +
 * the polled members list) and those two must not import each other both
 * ways.
 */

import { derived } from 'svelte/store';

import { nicknames } from './actors';
import { membersList, paramsView } from './node';

/** "0x1a2b3c…9f0e" — the compact form used in chips and lists. */
export function shortAddress(addr: string | null | undefined): string | null {
    if (!addr) return null;
    return addr.length > 13 ? `${addr.slice(0, 8)}…${addr.slice(-4)}` : addr;
}

/** Reactive lookup: `$addressOf(id)` → full 0x address, or null before the
 *  members list has loaded (or for an unknown id). */
export const addressOf = derived(membersList, ($members) => {
    const map = new Map<number, string>($members.map((m) => [m.id, m.address]));
    return (id: number | null | undefined): string | null =>
        id === null || id === undefined ? null : (map.get(id) ?? null);
});

/** Reactive display-name lookup: `$memberName(id)`. */
export const memberName = derived([nicknames, addressOf], ([$n, $addr]) => (id: number | null | undefined): string => {
    if (id === null || id === undefined) return '—';
    if ($n[id]) return $n[id];
    return shortAddress($addr(id)) ?? `#${id}`;
});

/** Deterministic hue for a member (matches the identicon's address hash). */
export function memberHue(address: string | null, id: number): number {
    if (!address) return (((id ?? 0) + 1) * 2654435761) % 360;
    let h = 0x811c9dc5;
    for (let i = 0; i < address.length; i++) {
        h ^= address.charCodeAt(i);
        h = Math.imul(h, 0x01000193);
    }
    return (h >>> 0) % 360;
}

/**
 * H2/the `SealAmounts` charter policy: once active (governed param
 * `SealAmounts` ≥ 0.5), every read endpoint reports contract/wallet amounts
 * rounded up to the next power of two (`edet_kernel::cascade::pow2_bucket`,
 * mirrored server-side in `serve::views`) instead of the exact figure —
 * sealed for everyone alike, not a per-viewer split. The UI must not present
 * those bucketed numbers as if they were precise.
 */
export const amountsSealed = derived(paramsView, ($p) => {
    const p = $p?.governed.find((g) => g.key === 'SealAmounts');
    return (p?.value ?? 0) >= 0.5;
});

/**
 * Render an amount, marking it as approximate whenever the node is
 * currently reporting pow2-bucketed figures (`amountsSealed`). `fmt` is the
 * caller's existing number formatter (e.g. `n.toFixed(2)`).
 */
export function formatAmount(fmt: (n: number) => string, value: number, sealed: boolean): string {
    return sealed ? `≈ ${fmt(value)}` : fmt(value);
}

/** Copy text to the clipboard, best-effort (secure contexts + fallback). */
export async function copyText(text: string): Promise<boolean> {
    try {
        if (navigator.clipboard?.writeText) {
            await navigator.clipboard.writeText(text);
            return true;
        }
    } catch {
        // fall through to the legacy path
    }
    try {
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        const ok = document.execCommand('copy');
        ta.remove();
        return ok;
    } catch {
        return false;
    }
}
