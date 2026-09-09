/**
 * The actor system: who am I in this community?
 *
 * The node is a shared ledger of numbered members; the app always acts *as*
 * one of them — the "current actor". Acting requires the member's Ed25519
 * seed, and this device holds exactly the seeds of identities created or
 * restored HERE (dev founders included: their recovery phrases are printed
 * by the launcher and restored through the same onboarding flow). There is
 * no fallback to derive anyone else's key: transactions needing other
 * parties' signatures go through the node's pending-signature pool, where
 * those parties sign from their own devices (see lib/submit.ts).
 *
 * A special 'pending' slot carries the seed of a freshly created identity
 * that has no member id yet. That is not a waiting room for an
 * admission — there is none — but for the identity's FIRST TRADE: an account
 * is seated by the first bonded trade that names its key, so a device sits in
 * this slot until a counterparty has traded with it and the ledger has a name
 * for it.
 *
 * Custody: seeds are persisted through the identity vault
 * (common/vault.ts) — XChaCha20-Poly1305 ciphertext in IndexedDB, wrapped
 * by a device key held in a separate store; never plaintext localStorage.
 * The recovery phrase itself is NEVER stored — it is shown once at
 * creation.
 *
 * Nicknames are purely local labels. There are no usernames on the ledger and
 * no attestations either: an account is a set of keys, and what makes it
 * accountable is the standing others have staked on it.
 */

import { derived, writable, get } from 'svelte/store';

import { lsGet, lsSet, lsRemove } from '../common/safeStorage';
import { loadVault, persistVault, type SeedRing } from '../common/vault';
import { derivePublicKey } from './crypto';
import type { Party } from './api';

/** Reserved seed-ring slot for a created-but-not-yet-admitted identity. */
const PENDING_SLOT = 'pending';

const ACTOR_KEY = 'edet-actor';
const NAMES_KEY = 'edet-names';

function loadJson<T>(key: string, fallback: T): T {
    const raw = lsGet(key);
    if (!raw) return fallback;
    try {
        return { ...fallback, ...JSON.parse(raw) };
    } catch {
        return fallback;
    }
}

// --------------------------------------------------------------- current ----

function loadActorId(): number | null {
    const raw = lsGet(ACTOR_KEY);
    if (raw === null || raw === '') return null;
    const n = Number(raw);
    return Number.isInteger(n) && n >= 0 ? n : null;
}

/** The member id the app is acting as, or null before onboarding picks one. */
export const currentActorId = writable<number | null>(loadActorId());

currentActorId.subscribe((id) => {
    if (id === null) lsRemove(ACTOR_KEY);
    else lsSet(ACTOR_KEY, String(id));
});

export function chooseActor(id: number): void {
    currentActorId.set(id);
}

export function clearActor(): void {
    currentActorId.set(null);
}

// -------------------------------------------------------------- seed ring ----

/**
 * Ed25519 seeds held on this device (member id -> 32 bytes). Starts empty;
 * `initActors()` fills it from the encrypted vault before the app renders,
 * and every later change is re-encrypted back into the vault.
 */
export const keyring = writable<SeedRing>({});

let vaultLoaded = false;

/**
 * Load the identity vault into the keyring. Must complete before anything
 * signs; App.svelte gates first render on it. Throws when the vault exists
 * but cannot be opened (corrupt store / foreign device key) so the failure
 * is loud instead of silently starting a fresh ring over it.
 */
export async function initActors(): Promise<void> {
    const ring = await loadVault();
    keyring.set(ring);
    vaultLoaded = true;
}

keyring.subscribe((k) => {
    // Persist only after the initial vault load: the empty first emission
    // must never overwrite an existing vault.
    if (vaultLoaded) void persistVault(k);
});

export function rememberSeed(id: number, seed: Uint8Array | number[]): void {
    const arr = Array.from(seed);
    keyring.update((k) => ({ ...k, [id]: arr }));
}

export function forgetSeed(id: number): void {
    keyring.update((k) => {
        const next = { ...k };
        delete next[id];
        return next;
    });
}

/** The seed this device holds for member `id`, or null. No fallbacks: a
 *  seed we don't hold means that party signs from their own device. */
export function heldSeed(id: number): Uint8Array | null {
    const held = get(keyring)[id];
    return held ? Uint8Array.from(held) : null;
}

export function holdsSeed(id: number): boolean {
    return get(keyring)[id] !== undefined;
}

/** The signing seed for member `id`; throws when this device doesn't hold
 *  it (callers must route through the pending pool instead). */
export function seedOf(id: number): Uint8Array {
    const held = heldSeed(id);
    if (!held) throw new Error(`no seed held for member ${id}`);
    return held;
}

// ------------------------------------------------------ pending identity ----

/** Seed of a created identity awaiting sponsor admission (no id yet). */
export const pendingIdentity = derived(keyring, ($k) => {
    const seed = ($k as Record<string, number[]>)[PENDING_SLOT];
    return seed ? Uint8Array.from(seed) : null;
});

export function setPendingSeed(seed: Uint8Array | number[]): void {
    keyring.update((k) => ({ ...k, [PENDING_SLOT]: Array.from(seed) }) as SeedRing);
}

/**
 * Does this device hold the seed for `party`, and which one?
 *
 * A party named by KEY is the newcomer's own identity, which lives in the
 * pending slot until their first trade commits — so the match is made by
 * deriving each held seed's public key rather than by any id, because an id is
 * precisely what that party does not have yet. Held member seeds are checked
 * too, so a counterparty naming an established member by key still resolves
 * here rather than looking unsignable.
 */
export function heldSeedOfParty(party: Party): Uint8Array | null {
    if ('Member' in party) return heldSeed(party.Member);
    const want = party.Key.join(',');
    for (const raw of Object.values(get(keyring) as Record<string, number[]>)) {
        const seed = Uint8Array.from(raw);
        if (derivePublicKey(seed).join(',') === want) return seed;
    }
    return null;
}

export function holdsParty(party: Party): boolean {
    return heldSeedOfParty(party) !== null;
}

/** The signing seed for `party`; throws when this device does not hold it. */
export function seedOfParty(party: Party): Uint8Array {
    const held = heldSeedOfParty(party);
    if (!held) throw new Error(`no seed held for ${JSON.stringify(party)}`);
    return held;
}

/** Promote the pending identity to a real member id (post-admission). */
export function adoptPendingSeed(id: number): void {
    keyring.update((k) => {
        const next = { ...(k as Record<string, number[]>) };
        const seed = next[PENDING_SLOT];
        if (seed) {
            next[String(id)] = seed;
            delete next[PENDING_SLOT];
        }
        return next as SeedRing;
    });
}

export function clearPendingSeed(): void {
    keyring.update((k) => {
        const next = { ...(k as Record<string, number[]>) };
        delete next[PENDING_SLOT];
        return next as SeedRing;
    });
}

// ------------------------------------------------------------- nicknames ----

/** Local display names per member id. Never leaves this device. */
export const nicknames = writable<Record<number, string>>(loadJson(NAMES_KEY, {}));

nicknames.subscribe((n) => lsSet(NAMES_KEY, JSON.stringify(n)));

export function setNickname(id: number, name: string): void {
    nicknames.update((n) => {
        const next = { ...n };
        if (name.trim() === '') delete next[id];
        else next[id] = name.trim();
        return next;
    });
}
