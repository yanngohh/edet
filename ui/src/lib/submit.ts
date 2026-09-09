/**
 * One shared write path, production-shaped:
 *
 *   - dry-run check against the claimed on-ledger keys of every required
 *     signer (so protocol rejections surface before anything moves);
 *   - if this device holds enough of the required seeds (usually: the
 *     action is unilateral and I am the signer) → sign and submit;
 *   - otherwise → sign my part and park it in the node's pending pool;
 *     the counterparties see it in their own app and co-sign there.
 *
 * No device ever signs for a party whose seed it does not hold.
 */

import { get } from 'svelte/store';
import { _ } from 'svelte-i18n';
import { sha256 } from '@noble/hashes/sha2';

import * as api from './api';
import { currentActorId, heldSeed, heldSeedOfParty, holdsParty, holdsSeed, seedOfParty } from './actors';
import { activeBase, customChainId, memberKeyOf, networkId, networkView, paramsView, refreshSoon } from './node';
import { declaredChainId } from './networks';
import { bytesToHex, derivePublicKey, hexToBytes, randomNonce, signDigest } from './crypto';
import { toHex, txDigestLocal } from './txdigest';
import {
    dropScannedOffer,
    encodeOffer,
    offerEnvelope,
    offerTx,
    type Offer,
    type OfferLane,
    type OpenOffer,
    type VerifiedOffer,
} from './offer';
import { inviteWire, type Invite } from './invite';
import { keyProof } from './session';
import { errorStore } from '../common/errorStore';
import { custodyDowngrade, custodyDowngradeAcknowledged, lockState } from '../common/vault';

/**
 * The default validity window for a freshly-opened transaction —
 * mirrors the node's own ceiling (`edet_kernel::constants::
 * MAX_TX_LIFETIME_EPOCHS`, currently 30). The widest legal margin, on
 * purpose: a `Settle`/`Accept`/rotation signed here may sit in the pending
 * pool for a while awaiting counterparties, and there is no way to widen
 * the window after the fact (it is baked into the digest every signer
 * already signed) — see `PendingEntryView`'s doc comment in `lib/api.ts`.
 */
const NOT_AFTER_WINDOW_EPOCHS = 30;

/**
 * The epoch a fresh envelope's `not_after_epoch` is computed from — the
 * LEDGER's own logical clock (`lib/node.ts`'s polled `networkView.epoch`),
 * never a value invented client-side. Throws rather than falling back to an
 * absolute epoch guess if no view has loaded yet; every write path here
 * runs after the app's first successful poll, so this should not trigger in
 * practice.
 */
function defaultNotAfterEpoch(): number {
    const epoch = get(networkView)?.epoch;
    if (epoch === undefined || epoch === null) {
        throw new Error('cannot sign a transaction before the node epoch is known');
    }
    return epoch + NOT_AFTER_WINDOW_EPOCHS;
}

/**
 * The chain id this device binds its signatures to.
 *
 * **Declared out of the node's reach** — by the network (`networks.ts`) or,
 * for `custom`, by the member (`node.ts::customChainId`) — and only then
 * checked against what the node reports. The digest is computed here, so the
 * chain id is the one remaining input a node could supply, and a hostile one
 * that named another ledger would collect a signature valid THERE, on a
 * transaction the member approved for here. So the node's answer is never
 * what a signature binds to: with nothing declared this device refuses, and
 * with a declaration the node contradicts it refuses too.
 *
 * Exported for its tests; nothing outside this module signs.
 */
export function signingChainId(): string {
    const declared = declaredChainId(get(networkId), get(customChainId));
    if (declared === null) {
        throw new SigningRefused('undeclared');
    }
    const reported = get(networkView)?.chain_id;
    if (reported === undefined || reported === null || reported === '') {
        throw new Error('cannot sign a transaction before the node chain id is known');
    }
    if (declared !== reported) {
        throw new SigningRefused('chain');
    }
    return declared;
}

/**
 * The digest for an envelope, computed on this device.
 *
 * Never a fetch to `/tx/digest` on the node the wallet happens to be reading:
 * that lets the node choose the bytes the wallet signs, and `/tx/check` is no
 * help because the same node answers it. `txdigest.ts` is cross-pinned to
 * `block.rs::tx_digest` by generated vectors (`just tx-digest-check`).
 */
function digestFor(tx: api.Tx, nonce: number[], notAfterEpoch: number): Uint8Array {
    requireCustody();
    return txDigestLocal(signingChainId(), tx, nonce, notAfterEpoch);
}

/**
 * **Does this pending entry's digest actually cover the transaction it
 * shows?** Recomputed from `(tx, nonce, not_after_epoch)` and compared against
 * the `digest` the node served.
 *
 * The inbox renders `entry.tx`; signing `entry.digest` without checking that
 * the two correspond is the same hole as `/tx/digest`, one step later and
 * harder to see, because the member is looking at a transaction the app is not
 * signing.
 *
 * Returns the digest to sign, or throws. There is deliberately no fallback to
 * the served value: a mismatch is either a hostile node or a client that has
 * drifted from the encoding, and signing is wrong in both cases.
 */
function digestOfPending(entry: api.PendingEntryView): Uint8Array {
    requireCustody();
    const local = txDigestLocal(signingChainId(), entry.tx as api.Tx, entry.nonce, entry.not_after_epoch);
    if (toHex(local) !== entry.digest.toLowerCase()) {
        throw new SigningRefused('mismatch');
    }
    return local;
}

/**
 * This device refused to sign, and it is not a transport failure.
 *
 * The distinction is the whole message. "Could not reach the node" is what a
 * member does something about by waiting; this is what they do something about
 * by not trusting the node — so it carries its own copy, and `transportError`
 * is not allowed to swallow it.
 */
class SigningRefused extends Error {
    constructor(public readonly reason: 'mismatch' | 'chain' | 'undeclared' | 'locked' | 'custody') {
        super(reason);
        this.name = 'SigningRefused';
    }
}

/**
 * **What must be true before this device puts a signature on anything.**
 *
 * Two conditions, and both are about custody rather than about the
 * transaction. The vault must be unlocked, or the seeds are not this session's
 * to use. And a custody DOWNGRADE must have been acknowledged: the device key
 * falls back to browser storage when the OS keychain or keystore cannot be
 * reached, which leaves the key beside the ciphertext — a member who believes
 * their seeds are in the OS keychain is acting on a promise the app is no
 * longer keeping, and it is a promise about the one secret they cannot replace.
 *
 * Checked here rather than at each call site, so a new signing path cannot be
 * added without it.
 */
function requireCustody(): void {
    if (get(lockState) === 'locked') {
        throw new SigningRefused('locked');
    }
    if (get(custodyDowngrade) !== null && !get(custodyDowngradeAcknowledged)) {
        throw new SigningRefused('custody');
    }
}

/**
 * Report a refusal to the member, in words that name what happened. A digest
 * that does not cover the request on screen is either a node serving one thing
 * and asking for a signature over another, or a client whose encoder has
 * drifted from the ledger's; neither is something to sign through.
 */
function signingRefused(e: SigningRefused): void {
    const t = get(_);
    const message = {
        chain: t('errors.signingChain', {
            default: 'This node claims to be a different network than the one you chose. Nothing was signed.',
        }),
        undeclared: t('errors.signingUndeclared', {
            default:
                'This network has no declared chain id, so this device will not sign for it. Enter it beside the node address in the network settings.',
        }),
        mismatch: t('errors.signingMismatch', {
            default: 'This request does not match what the node asked you to sign. Nothing was signed.',
        }),
        locked: t('errors.signingLocked', {
            default: 'Your vault is locked. Enter your passphrase to sign.',
        }),
        custody: t('errors.signingCustody', {
            default:
                'This device could not reach its secure key store, so your keys are protected only by this app’s own storage. Review that in Backup & security before signing.',
        }),
    }[e.reason];
    errorStore.pushError(message);
}

/**
 * The chain id this device signs for, or `null` silently — for a signature
 * that is a convenience rather than a consent (a seller's invitation, minted
 * on every wallet render), where a toast per render would be noise and the
 * fallback is a plain address.
 */
export function chainIdIfDeclared(): string | null {
    try {
        return signingChainId();
    } catch {
        return null;
    }
}

/**
 * The chain id this device signs for, or `null` after telling the member why
 * there is none — for a caller that checks a signature against it (a buyer's
 * code) rather than making one.
 */
export function chainIdOrReport(): string | null {
    try {
        return signingChainId();
    } catch (e) {
        if (e instanceof SigningRefused) signingRefused(e);
        else transportError(e);
        return null;
    }
}

/** Localized message for a protocol rejection code. */
export function rejectionMessage(code: string): string {
    const t = get(_);
    const generic = t('errors.rejected', {
        values: { code },
        default: `Rejected by the ledger (${code})`,
    });
    return t(`errors.${code}`, { default: generic });
}

/**
 * The distinct parties a plan needs, deduplicated by value.
 *
 * By value rather than by id because a party may be a KEY, and
 * two encodings of the same person — the id and one of their keys — are not
 * equal here. That is deliberate rather than a gap: the node resolves a key
 * to its holder, so naming both is redundant but never wrong, and collapsing
 * them client-side would require knowing a mapping the newcomer's device does
 * not have.
 */
function distinctParties(parties: api.Party[]): api.Party[] {
    const seen = new Set<string>();
    return parties.filter((p) => {
        const tag = JSON.stringify(p);
        if (seen.has(tag)) return false;
        seen.add(tag);
        return true;
    });
}

/** Route for a plan given which required seeds this device holds. */
export function decideRoute(
    requiredCount: number,
    minSigs: number,
    heldCount: number,
): 'direct' | 'pending' | 'unsignable' {
    if (heldCount >= minSigs) return 'direct';
    return heldCount > 0 ? 'pending' : 'unsignable';
}

function transportError(e: unknown): void {
    // A refusal is not a transport failure, and telling a member their node is
    // unreachable when their node is lying is the wrong instruction.
    if (e instanceof SigningRefused) {
        signingRefused(e);
        return;
    }
    const t = get(_);
    errorStore.pushError(
        t('errors.transport', {
            values: { message: e instanceof Error ? e.message : String(e) },
            default: `Could not reach the node: ${e instanceof Error ? e.message : String(e)}`,
        }),
    );
}

/**
 * Dry-run a plan against current state using every required member's
 * claimed on-ledger key (no signatures involved). Used to choose between
 * alternative encodings of one user intent (e.g. cascade Sale vs plain
 * Accept for a purchase) before dispatching.
 */
export async function precheck(plan: api.TxPlan): Promise<{ ok: boolean; code?: string }> {
    const base = get(activeBase);
    const keyOf = get(memberKeyOf);
    const required = distinctParties(plan.signers);
    const claimed: number[][] = [];
    for (const party of required) {
        // A party named by KEY already IS its own claimed key — that is the
        // whole of what naming a key says, and it is the case a newcomer's
        // first trade is in.
        if ('Key' in party) {
            claimed.push(party.Key);
            continue;
        }
        const hex = keyOf(party.Member);
        const seed = heldSeedOfParty(party);
        if (hex) claimed.push(Array.from(hexToBytes(hex)));
        else if (seed) claimed.push(derivePublicKey(seed));
        else return { ok: true }; // key unknown: defer to send()'s own check
    }
    try {
        // The dry-run's own outcome doesn't depend on WHICH valid nonce is
        // used (`checkTx` trusts the claimed signers; it never verifies a
        // signature), only on the window being legal — so a fresh one here
        // is fine; this is never the envelope anything actually signs.
        return await api.checkTx(base, {
            tx: plan.tx,
            nonce: randomNonce(),
            not_after_epoch: defaultNotAfterEpoch(),
            signers: claimed,
            signatures: [],
        });
    } catch {
        return { ok: true };
    }
}

/**
 * The bond a plan's transition would reserve, asked of the node rather than
 * computed here — the schedule lives in `edet_state::bond::bond_multiple`
 * and a second copy in TypeScript would drift the first time it moved.
 *
 * Cached by transition CLASS, not by plan: `bond_multiple` switches on the
 * transaction variant alone, so every `Accept` quotes the same amount
 * whatever the form currently holds. That makes this one request per action
 * page instead of one per keystroke, and it stays correct because the cache
 * key is exactly what the schedule itself keys on.
 *
 * `bond_unit` joins the key because the quoted amount is `multiple *
 * bond_unit`, and the unit does move — on a redenomination, or on a
 * governed amendment to `BondFraction`. Folding it into the key re-quotes
 * automatically instead of leaving a stale figure on screen.
 *
 * Returns null when the node does not report a bond (one predating the
 * disclosure) or the dry-run cannot be reached — callers show nothing
 * rather than guessing.
 */
const bondQuotes = new Map<string, api.TxBondView>();

export async function quoteBond(plan: api.TxPlan): Promise<api.TxBondView | null> {
    const cls = Object.keys(plan.tx)[0];
    if (!cls) return null;
    const unit = get(paramsView)?.bond_unit;
    const key = `${cls}@${unit ?? '?'}`;
    const hit = bondQuotes.get(key);
    if (hit) return hit;
    const base = get(activeBase);
    try {
        // The dry-run's VERDICT is irrelevant here — a half-filled form will
        // rightly be rejected, and the bond is quoted on the rejection paths
        // too precisely so the cost can be shown while the form is still
        // being filled. Only `bond` is read.
        const res = await api.checkTx(base, {
            tx: plan.tx,
            nonce: randomNonce(),
            not_after_epoch: defaultNotAfterEpoch(),
            signers: [],
            signatures: [],
        });
        if (!res.bond) return null;
        bondQuotes.set(key, res.bond);
        return res.bond;
    } catch {
        return null;
    }
}

/**
 * Check and dispatch a transaction plan. Returns true when the action is
 * on its way — committed directly, or parked awaiting counterparties (a
 * toast says which). Returns false on rejection or transport failure.
 */
export async function send(plan: api.TxPlan): Promise<boolean> {
    const base = get(activeBase);

    const t = get(_);
    const required = distinctParties(plan.signers);
    const minSigs = Math.max(0, Math.min(plan.minSigs ?? required.length, required.length));
    const held = required.filter(holdsParty);
    const route = decideRoute(required.length, minSigs, held.length);
    if (route === 'unsignable') {
        errorStore.pushError(
            t('requests.cannotSign', {
                default: 'None of the required signatures can be produced on this device.',
            }),
        );
        return false;
    }
    try {
        // Dry-run with every required member's claimed on-ledger key, so
        // state-machine rejections surface before anything is parked. Uses
        // a throwaway nonce/window — see `precheck`'s comment; this is not
        // the envelope that gets signed below.
        const keyOf = get(memberKeyOf);
        const claimed: number[][] = [];
        let allKnown = true;
        for (const party of required) {
            if ('Key' in party) {
                claimed.push(party.Key);
                continue;
            }
            const hex = keyOf(party.Member);
            const seed = heldSeedOfParty(party);
            if (hex) claimed.push(Array.from(hexToBytes(hex)));
            else if (seed) claimed.push(derivePublicKey(seed));
            else allKnown = false;
        }
        if (allKnown) {
            const chk = await api.checkTx(base, {
                tx: plan.tx,
                nonce: randomNonce(),
                not_after_epoch: defaultNotAfterEpoch(),
                signers: claimed,
                signatures: [],
            });
            // `ok` ABSENT is not a rejection: the node answered with the
            // public bond quote only because it did not recognize this
            // caller as a party to its own draft — the viewer proof was
            // missing or resolved to nobody (locked or stale vault, actor
            // whose seed this device no longer decrypts). Every action fails
            // that way at once, so name the actual problem instead of
            // reporting a state-machine verdict that never happened.
            if (chk.ok === undefined) {
                errorStore.pushError(
                    t('requests.noViewerProof', {
                        default:
                            'The ledger could not verify who you are: this device is not holding your identity key right now. Restore your identity from its recovery phrase and retry.',
                    }),
                );
                return false;
            }
            if (!chk.ok) throw new api.TxRejectedError(chk.code ?? 'ET-UNKNOWN');
        }

        // The nonce and validity window are fixed exactly ONCE, right
        // here, for this whole signing intent — direct or pending. Every
        // signature below (this device's own, and any co-signer's added
        // later through the pending pool) MUST cover this SAME envelope:
        // the node recomputes the digest from `(tx, nonce, not_after_epoch)`
        // to verify each signature, so two signers who signed different
        // envelopes never converge on one digest and the transaction can
        // never complete. A co-signer's device never generates its own —
        // it reads these back off the pending entry (`signPending`, below).
        const nonce = randomNonce();
        const notAfterEpoch = defaultNotAfterEpoch();
        const digest = digestFor(plan.tx, nonce, notAfterEpoch);

        if (route === 'direct') {
            const signed = api.signPlanWithDigest({ tx: plan.tx, signers: held }, digest, nonce, notAfterEpoch, seedOfParty);
            const res = await api.submitTx(base, signed);
            if (!res.queued) throw new api.TxRejectedError('ET-SIG-001');
            // Queued is not committed. The node keys the commit outcome on a
            // hash the client cannot reproduce, so it hands it back, and this
            // is where a refusal at commit — a replay, a window that closed, a
            // counterparty whose status moved — reaches the member instead of
            // leaving them with "sent".
            if (res.hash) void watchOutcome(res.hash);
            refreshSoon();
            return true;
        }

        // Pending route: contribute the signatures this device can make.
        let completed = false;
        for (const party of held) {
            const seed = heldSeedOfParty(party)!;
            const r = await api.pendingSign(base, {
                tx: plan.tx,
                nonce,
                not_after_epoch: notAfterEpoch,
                required,
                min_sigs: minSigs,
                signer: derivePublicKey(seed),
                signature: signDigest(digest, seed),
            });
            if (!r.ok) throw new Error(r.error ?? 'pending sign refused');
            if (r.completed) {
                completed = true;
                if (r.hash) void watchOutcome(r.hash);
            }
        }
        refreshSoon();
        if (!completed) {
            errorStore.pushError(
                t('requests.sentForSignature', {
                    default: 'Signature request sent — track or withdraw it under Requests.',
                }),
                'warning',
            );
        }
        return true;
    } catch (e) {
        if (e instanceof api.TxRejectedError) {
            errorStore.pushError(rejectionMessage(e.code));
        } else {
            transportError(e);
        }
        return false;
    }
}

// ------------------------------------------- the first trade, as a key ----

/**
 * Co-sign the trade that will seat this device's account.
 *
 * `signPending` cannot serve it: that path resolves the actor through
 * `currentActorId`, and the whole point of this moment is that there is no
 * actor id to resolve. The envelope rule holds identically — the nonce and window come off
 * the entry, never from here, or this signature would cover a different
 * envelope than the initiator's and the two would never converge on one
 * digest.
 */
export async function signPendingAsKey(entry: api.PendingEntryView, seed: Uint8Array): Promise<boolean> {
    const base = get(activeBase);
    try {
        const r = await api.pendingSign(base, {
            tx: entry.tx,
            nonce: entry.nonce,
            not_after_epoch: entry.not_after_epoch,
            required: entry.required,
            min_sigs: entry.min_sigs,
            signer: derivePublicKey(seed),
            signature: signDigest(digestOfPending(entry), seed),
        });
        if (!r.ok) {
            errorStore.pushError(r.error ?? 'refused');
            return false;
        }
        // The signature that completes the request is the one that puts the
        // envelope on its way, so this device is the one that learns whether
        // it then committed.
        if (r.completed && r.hash) void watchOutcome(r.hash);
        refreshSoon();
        return true;
    } catch (e) {
        transportError(e);
        return false;
    }
}

/**
 * Decline it instead. A newcomer named by key is a required party like any
 * other, and refusing a trade offered to you is the same right every other
 * party has — which is also why the node's decline path resolves parties
 * rather than members.
 */
export async function declinePendingAsKey(entry: api.PendingEntryView, seed: Uint8Array): Promise<boolean> {
    const base = get(activeBase);
    try {
        const r = await api.pendingDecline(base, {
            digest: entry.digest,
            signer: derivePublicKey(seed),
            signature: signDigest(declineMessage(entry.digest), seed),
        });
        if (!r.ok) {
            errorStore.pushError('refused');
            return false;
        }
        refreshSoon();
        return true;
    } catch (e) {
        transportError(e);
        return false;
    }
}

// ------------------------------------------------- pending-pool actions ----

/**
 * Co-sign a request that awaits the current actor. `nonce`/
 * `not_after_epoch` come from the entry itself — fixed by whoever opened
 * it — NEVER regenerated here. The node recomputes the digest it checks
 * this signature against from these exact fields; a co-signer that
 * invented its own would sign a different envelope than the initiator did,
 * and the pool entry would never reach its signature threshold.
 */
export async function signPending(entry: api.PendingEntryView): Promise<boolean> {
    const me = get(currentActorId);
    if (me === null || !holdsSeed(me)) return false;
    const base = get(activeBase);
    const seed = heldSeed(me)!;
    try {
        const r = await api.pendingSign(base, {
            tx: entry.tx,
            nonce: entry.nonce,
            not_after_epoch: entry.not_after_epoch,
            required: entry.required,
            min_sigs: entry.min_sigs,
            signer: derivePublicKey(seed),
            signature: signDigest(digestOfPending(entry), seed),
        });
        if (!r.ok) {
            errorStore.pushError(r.error ?? 'refused');
            return false;
        }
        // The signature that completes the request is the one that puts the
        // envelope on its way, so this device is the one that learns whether
        // it then committed.
        if (r.completed && r.hash) void watchOutcome(r.hash);
        refreshSoon();
        return true;
    } catch (e) {
        transportError(e);
        return false;
    }
}

/**
 * Poll `/tx/outcome/:hash` until it leaves "pending" (or the tries run out),
 * and toast a rejection through the shared errorStore — surfacing a
 * commit-time apply failure (H4) that a queued response alone can't show.
 * Silent on transport errors and on "unknown" (a stale/unreachable hash
 * should not false-alarm).
 *
 * The node keys outcomes by `SignedTx::hash()` — sha256 of the consensus
 * encoding of the signed envelope, which this client does not reproduce — and
 * hands the hash back from `/tx` and from a completing `/pending/sign`, so
 * every path that puts an envelope on its way calls this with it.
 */
export async function watchOutcome(hashHex: string, tries = 10, intervalMs = 500): Promise<void> {
    const base = get(activeBase);
    for (let i = 0; i < tries; i++) {
        let out: api.TxOutcomeView;
        try {
            out = await api.txOutcome(base, hashHex);
        } catch {
            return;
        }
        if (out.status === 'rejected') {
            errorStore.pushError(rejectionMessage(out.code ?? 'ET-UNKNOWN'));
            return;
        }
        if (out.status === 'ok' || out.status === 'unknown') return;
        await new Promise((r) => setTimeout(r, intervalMs));
    }
}

/**
 * Message a decline signs — must mirror the node byte-for-byte:
 * `crates/node/src/serve/pending.rs::decline_message` = sha256(digest ++
 * b"edet-decline"). Both sides pin the same fixed vector (see
 * `__tests__/decline-message.test.ts` and pending.rs's test module), so the
 * two cannot silently drift.
 */
export function declineMessage(digestHex: string): Uint8Array {
    const digest = hexToBytes(digestHex);
    const marker = new TextEncoder().encode('edet-decline');
    const bytes = new Uint8Array(digest.length + marker.length);
    bytes.set(digest);
    bytes.set(marker, digest.length);
    return sha256(bytes);
}

/** Decline (counterparty) or withdraw (initiator) a pending request. */
export async function declinePending(entry: api.PendingEntryView): Promise<boolean> {
    const me = get(currentActorId);
    if (me === null || !holdsSeed(me)) return false;
    const base = get(activeBase);
    const seed = heldSeed(me)!;
    try {
        const r = await api.pendingDecline(base, {
            digest: entry.digest,
            signer: derivePublicKey(seed),
            signature: signDigest(declineMessage(entry.digest), seed),
        });
        refreshSoon();
        return r.ok;
    } catch (e) {
        transportError(e);
        return false;
    }
}

/** Dry-run a pending entry as it would apply now (review aid). */
export async function checkPending(entry: api.PendingEntryView): Promise<{ ok: boolean; code?: string }> {
    const base = get(activeBase);
    const keyOf = get(memberKeyOf);
    // A party named by KEY is already its own claimed key; a member is looked
    // up. Either way the dry run needs one key per required party, and a
    // member whose key this device has not seen defers to `send`'s own check.
    const claimed = entry.required
        .map((party) => ('Key' in party ? bytesToHex(Uint8Array.from(party.Key)) : keyOf(party.Member)))
        .filter((k): k is string => !!k);
    if (claimed.length !== entry.required.length) return { ok: true };
    return api.checkTx(base, {
        tx: entry.tx,
        // The entry's OWN window — not a fresh default — so an expired
        // proposal correctly previews as expired rather than as healthy.
        nonce: entry.nonce,
        not_after_epoch: entry.not_after_epoch,
        signers: claimed.map((k) => Array.from(hexToBytes(k))),
        signatures: [],
    });
}

// ------------------------------------------------- a purchase by hand ------

/**
 * Sign a purchase on this device: the newcomer's half of a first trade,
 * which the pool holds only on the seller's invitation (`openInvited`) and
 * which otherwise travels as a code (`lib/offer.ts`). Both carry the SAME
 * envelope, fixed here exactly as `send` fixes one and for the same reason
 * — the seller's signature must cover the same bytes whichever way it
 * arrives — and the digest is this device's own computation. Nothing is
 * remembered here; the caller records the code with the outcome it got.
 */
export async function composeOffer(
    fields: { lane: OfferLane; seller: api.Party; amount: number; maturityEpochs: number; arb: api.ArbTermsView | null },
    seed: Uint8Array,
): Promise<{ open: OpenOffer; offer: Offer } | null> {
    try {
        const nonce = randomNonce();
        const notAfterEpoch = defaultNotAfterEpoch();
        const chainId = signingChainId();
        const unsigned: Offer = {
            chainId,
            lane: fields.lane,
            seller: fields.seller,
            buyerKeyHex: bytesToHex(Uint8Array.from(derivePublicKey(seed))),
            amount: fields.amount,
            maturityEpochs: fields.maturityEpochs,
            arb: fields.lane === 'accept' ? fields.arb : null,
            nonce,
            notAfterEpoch,
            signature: [],
        };
        const digest = digestFor(offerTx(unsigned), nonce, notAfterEpoch);
        const offer: Offer = { ...unsigned, signature: signDigest(digest, seed) };
        const open: OpenOffer = {
            payload: encodeOffer(offer),
            lane: fields.lane,
            seller: fields.seller,
            amount: fields.amount,
            maturityEpochs: fields.maturityEpochs,
            notAfterEpoch,
            createdSecs: Math.floor(Date.now() / 1000),
        };
        return { open, offer };
    } catch (e) {
        transportError(e);
        return null;
    }
}

/**
 * Park a signed purchase in the pool on the seller's invitation, so their
 * app shows it under Requests with nothing scanned.
 *
 * The node charges the entry to the inviting member and refuses anything
 * the invitation does not cover: another seller, an expired one, too many
 * open at once. A refusal is not an error to the buyer — the same envelope
 * still travels as a code — so it comes back as a word, not a toast.
 */
export async function openInvited(offer: Offer, invite: Invite): Promise<'sent' | 'refused'> {
    const buyerKey = Array.from(hexToBytes(offer.buyerKeyHex));
    const buyer = api.asKey(buyerKey);
    try {
        const r = await api.pendingSign(get(activeBase), {
            tx: offerTx(offer),
            nonce: offer.nonce,
            not_after_epoch: offer.notAfterEpoch,
            // The order `tx.sale` / `tx.accept` plans sign in.
            required: offer.lane === 'sale' ? [offer.seller, buyer] : [buyer, offer.seller],
            min_sigs: 2,
            signer: buyerKey,
            signature: offer.signature,
            invite: inviteWire(invite),
        });
        if (!r.ok) return 'refused';
        if (r.completed && r.hash) void watchOutcome(r.hash);
        refreshSoon();
        return 'sent';
    } catch {
        return 'refused';
    }
}

/** The claimed signers a scanned code's dry run names, in envelope order. */
function offerSigners(v: VerifiedOffer, myKey: number[]): number[][] {
    const buyerKey = Array.from(hexToBytes(v.offer.buyerKeyHex));
    return v.offer.lane === 'sale' ? [myKey, buyerKey] : [buyerKey, myKey];
}

/**
 * Dry-run a scanned code as it would apply now. This device is a party to
 * it, so the node answers with a verdict rather than the public bond quote.
 */
export async function checkOffer(v: VerifiedOffer): Promise<{ ok: boolean; code?: string }> {
    const seed = heldSeedOfParty(v.me);
    if (!seed) return { ok: true };
    try {
        return await api.checkTx(get(activeBase), {
            tx: v.tx as unknown as Record<string, unknown>,
            nonce: v.offer.nonce,
            not_after_epoch: v.offer.notAfterEpoch,
            signers: offerSigners(v, derivePublicKey(seed)),
            signatures: [],
        });
    } catch {
        return { ok: true };
    }
}

/**
 * Co-sign a scanned code and submit both signatures in one envelope.
 *
 * The digest is computed again here rather than read off the verified code:
 * `verifyOffer` ran on this device, but signing reads the envelope one more
 * time so nothing between the two steps can substitute what is signed.
 */
export async function acceptOffer(v: VerifiedOffer): Promise<boolean> {
    const t = get(_);
    const seed = heldSeedOfParty(v.me);
    if (!seed) {
        errorStore.pushError(
            t('requests.cannotSign', {
                default: 'None of the required signatures can be produced on this device.',
            }),
        );
        return false;
    }
    const base = get(activeBase);
    try {
        const chk = await checkOffer(v);
        if (chk.ok === false) throw new api.TxRejectedError(chk.code ?? 'ET-UNKNOWN');
        const digest = digestFor(v.tx, v.offer.nonce, v.offer.notAfterEpoch);
        if (toHex(digest) !== v.digestHex) throw new SigningRefused('mismatch');
        const res = await api.submitTx(base, offerEnvelope(v, derivePublicKey(seed), signDigest(digest, seed)));
        if (!res.queued) throw new api.TxRejectedError('ET-SIG-001');
        if (res.hash) void watchOutcome(res.hash);
        dropScannedOffer(v.digestHex);
        refreshSoon();
        return true;
    } catch (e) {
        if (e instanceof api.TxRejectedError) {
            errorStore.pushError(rejectionMessage(e.code));
        } else {
            transportError(e);
        }
        return false;
    }
}
