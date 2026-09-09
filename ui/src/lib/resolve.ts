/**
 * Counterparty resolution: from whatever the user typed or scanned to a
 * ledger member. Accepts a wallet address (0x + 40 hex) or a raw Ed25519
 * public key (64 hex). Resolution asks THIS device's node (`/whois`) — no
 * member enumeration involved, so it scales past a dropdown-sized
 * community.
 */

import { get } from 'svelte/store';

import * as api from './api';
import { activeBase, membersList } from './node';
import { currentActorId, heldSeed } from './actors';
import { keyProof } from './session';
import { derivePublicKey } from './crypto';

export type AddressKind = 'address' | 'pubkey' | 'invalid';

/** Classify pasted/scanned input (pure — unit-tested). */
export function classifyAddressInput(raw: string): { kind: AddressKind; clean: string } {
    const trimmed = raw.trim();
    const clean = (trimmed.startsWith('0x') || trimmed.startsWith('0X') ? trimmed.slice(2) : trimmed).toLowerCase();
    if (/^[0-9a-f]{40}$/.test(clean)) return { kind: 'address', clean: `0x${clean}` };
    if (/^[0-9a-f]{64}$/.test(clean)) return { kind: 'pubkey', clean };
    return { kind: 'invalid', clean: trimmed };
}

export interface Resolved {
    member: number;
    address: string | null;
}

/**
 * Resolve input to a member via the node; null when unknown/invalid.
 *
 * Carries this device's own key proof, not just whatever session happens to
 * be live. The node resolves an address only for an authenticated member, and
 * the two transports do not authenticate the same way: over HTTP a bearer
 * token is usually present, but the embedded (Tauri) node is same-process IPC
 * with no token at all — so relying on the session would leave every desktop
 * purchase, vouch and admission unable to resolve its counterparty. Proving
 * the key costs nothing here (the seed is already held to sign the write that
 * follows) and makes both transports behave identically.
 */
export async function resolveCounterparty(raw: string): Promise<Resolved | null> {
    const { kind, clean } = classifyAddressInput(raw);
    if (kind === 'invalid') return null;
    const needle = kind === 'address' ? clean.slice(2) : clean;
    const me = get(currentActorId);
    const seed = me === null ? null : heldSeed(me);
    // A device with no seated identity — a newcomer composing their first
    // purchase — is nobody the node resolves an address for. The members
    // listing it serves anonymously carries each row's address, so the lookup
    // runs here against that page instead; a counterparty past its last page
    // is out of reach until the account exists, and a raw key is a party in
    // its own right (`AddressInput`'s `byKey`), needing no lookup at all.
    if (!seed) {
        if (kind !== 'address') return null;
        const row = get(membersList).find((m) => m.address?.toLowerCase() === clean);
        return row ? { member: row.id, address: row.address } : null;
    }
    const proof = keyProof(derivePublicKey(seed), seed, 'GET', `/whois/${needle}`);
    try {
        const who = await api.whois(get(activeBase), needle, proof);
        if (who.member === null || who.member === undefined) return null;
        return { member: who.member, address: who.address ?? null };
    } catch {
        return null;
    }
}
