/**
 * A purchase that travels by hand.
 *
 * A row is seated by the first bonded trade that names its key, and that
 * trade needs the newcomer's signature like any other. But a key cannot OPEN
 * an entry in the node's pending pool (`serve/driver.rs::pending_sign`,
 * `PendingPool::awaits`): keys are free, so the pool's occupancy is paid by a
 * member, and a signer with no account may only co-sign an entry a member
 * already opened naming that exact key. A newcomer who wants to buy first has
 * nowhere on the node to put their half.
 *
 * So it does not go through the node. The wallet signs the purchase here and
 * hands the seller a CODE — a QR, or the same text pasted into a chat — that
 * carries the trade, the envelope fields and the buyer's signature. The
 * seller's wallet rebuilds the envelope, recomputes the digest itself (never
 * trusting the code's word for it), checks the signature, shows the trade for
 * review, co-signs and submits both signatures to `/tx` in one envelope. On
 * the ledger it is the trade the pool would have assembled; only the transport
 * differs, and the seller sees an ordinary purchase request.
 *
 * What a code cannot do: it names ONE seller, so nobody else can complete it;
 * it expires with `not_after_epoch`; and the buyer's signature covers exactly
 * the fields it carries, so a code altered in transit fails verification
 * rather than booking something else.
 */

import { writable } from 'svelte/store';

import type { ArbTermsView, Party, SignedTx, Tx } from './api';
import { asKey, asMember, partyKeyHex, partyMember } from './api';
import { hexToBytes, verifyDigest } from './crypto';
import { toHex, txDigestLocal } from './txdigest';
import { lsGet, lsSet } from '../common/safeStorage';

/** Which transition the code carries: the cascade `Sale`, or a plain
 *  `Accept` where arbitration terms are pinned. */
export type OfferLane = 'sale' | 'accept';

/** A signed purchase, as the code carries it. */
export interface Offer {
    chainId: string;
    lane: OfferLane;
    /** The seller: a member by id, or by one of their keys when the buyer
     *  could not look the id up. The node resolves a key to its holder. */
    seller: Party;
    buyerKeyHex: string;
    amount: number;
    maturityEpochs: number;
    /** Accept lane only; `null` on the sale lane. */
    arb: ArbTermsView | null;
    nonce: number[];
    notAfterEpoch: number;
    /** The buyer's Ed25519 signature over the envelope digest. */
    signature: number[];
}

const SCHEME = /^edet:\/\/buy\?(.*)$/i;
const HEX = (n: number) => new RegExp(`^[0-9a-f]{${n}}$`);

function b64url(s: string): string {
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
function unb64url(s: string): string | null {
    try {
        const padded = s.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - (s.length % 4)) % 4);
        return atob(padded);
    } catch {
        return null;
    }
}

/** The transaction a code stands for, rebuilt field by field. */
export function offerTx(o: Offer): Tx {
    const buyer = asKey(Array.from(hexToBytes(o.buyerKeyHex)));
    if (o.lane === 'sale') {
        return { Sale: { seller: o.seller, buyer, amount: o.amount, maturity_epochs: o.maturityEpochs } };
    }
    return { Accept: { debtor: buyer, creditor: o.seller, amount: o.amount, maturity_epochs: o.maturityEpochs, arb: o.arb } };
}

/** `edet://buy?…`: the text behind the QR, and what a chat message carries. */
export function encodeOffer(o: Offer): string {
    const q = new URLSearchParams();
    q.set('v', '1');
    q.set('c', o.chainId);
    q.set('t', o.lane);
    const id = partyMember(o.seller);
    q.set('s', id !== null ? `m${id}` : `k${partyKeyHex(o.seller)}`);
    // The shortest decimal that round-trips the double: `Number()` of it is
    // the same amount the buyer signed, so the seller's rebuilt envelope
    // digests to the same bytes.
    q.set('a', String(o.amount));
    q.set('m', String(o.maturityEpochs));
    if (o.lane === 'accept' && o.arb) q.set('arb', b64url(JSON.stringify(o.arb)));
    q.set('n', toHex(Uint8Array.from(o.nonce)));
    q.set('e', String(o.notAfterEpoch));
    q.set('k', o.buyerKeyHex.toLowerCase());
    q.set('g', toHex(Uint8Array.from(o.signature)));
    return `edet://buy?${q.toString()}`;
}

function parseArb(raw: string | null): ArbTermsView | null | undefined {
    if (raw === null) return null;
    const json = unb64url(raw);
    if (json === null) return undefined;
    try {
        const a = JSON.parse(json) as Partial<ArbTermsView>;
        if (
            !Array.isArray(a.arbiters) ||
            !a.arbiters.every((x) => Number.isInteger(x) && x >= 0) ||
            !Number.isInteger(a.quorum) ||
            !Number.isInteger(a.window_epochs) ||
            typeof a.award_cap !== 'number' ||
            !Number.isFinite(a.award_cap)
        ) {
            return undefined;
        }
        return { arbiters: a.arbiters, quorum: a.quorum!, window_epochs: a.window_epochs!, award_cap: a.award_cap };
    } catch {
        return undefined;
    }
}

/**
 * Parse whatever was scanned or pasted. `null` when it is not a buyer's code
 * at all, or when any field is malformed — a code that half-parses is worth
 * nothing, since the signature is over all of it.
 */
export function parseOffer(raw: string): Offer | null {
    const m = raw.trim().match(SCHEME);
    if (!m) return null;
    const q = new URLSearchParams(m[1]);
    if (q.get('v') !== '1') return null;
    const chainId = q.get('c');
    const lane = q.get('t');
    const s = q.get('s') ?? '';
    const a = q.get('a') ?? '';
    const mat = q.get('m') ?? '';
    const n = (q.get('n') ?? '').toLowerCase();
    const e = q.get('e') ?? '';
    const k = (q.get('k') ?? '').toLowerCase();
    const g = (q.get('g') ?? '').toLowerCase();
    if (chainId === null || chainId === '') return null;
    if (lane !== 'sale' && lane !== 'accept') return null;
    let seller: Party;
    if (/^m\d{1,18}$/.test(s)) seller = asMember(Number(s.slice(1)));
    else if (/^k[0-9a-f]{64}$/i.test(s)) seller = asKey(Array.from(hexToBytes(s.slice(1).toLowerCase())));
    else return null;
    const amount = Number(a);
    if (!/^[0-9.eE+-]+$/.test(a) || !Number.isFinite(amount) || !(amount > 0)) return null;
    if (!/^\d{1,9}$/.test(mat)) return null;
    if (!HEX(32).test(n) || !/^\d{1,12}$/.test(e) || !HEX(64).test(k) || !HEX(128).test(g)) return null;
    const arb = lane === 'accept' ? parseArb(q.get('arb')) : null;
    if (arb === undefined) return null;
    return {
        chainId,
        lane,
        seller,
        buyerKeyHex: k,
        amount,
        maturityEpochs: Number(mat),
        arb,
        nonce: Array.from(hexToBytes(n)),
        notAfterEpoch: Number(e),
        signature: Array.from(hexToBytes(g)),
    };
}

/** Why a code was refused, in the order the checks run. */
export type OfferProblem = 'chain' | 'signature' | 'not-for-me' | 'expired';

/** A code this device checked and may sign. */
export interface VerifiedOffer {
    offer: Offer;
    tx: Tx;
    /** Computed here, over the rebuilt envelope. */
    digestHex: string;
    buyer: Party;
    /** The party this device signs as, exactly as the code names it. */
    me: Party;
}

/** What the checker knows about this device. */
export interface OfferContext {
    /** The chain this device signs for: the network's declaration, checked
     *  against the node, never the code's own claim. */
    chainId: string;
    epoch: number;
    /** This device's member id, if seated, and its signing key. */
    me: { member: number | null; keyHex: string };
}

/**
 * **Does this code say what it will sign?** The digest is recomputed from
 * the rebuilt envelope and the buyer's signature checked against it, so the
 * trade shown for review is the trade the buyer consented to and nothing
 * else. The seller named must be this device, and the window still open.
 */
export function verifyOffer(o: Offer, ctx: OfferContext): { ok: true; verified: VerifiedOffer } | { ok: false; problem: OfferProblem } {
    if (o.chainId !== ctx.chainId) return { ok: false, problem: 'chain' };
    const tx = offerTx(o);
    const digest = txDigestLocal(ctx.chainId, tx, o.nonce, o.notAfterEpoch);
    if (!verifyDigest(o.signature, digest, hexToBytes(o.buyerKeyHex))) return { ok: false, problem: 'signature' };
    const sellerId = partyMember(o.seller);
    const forMe =
        sellerId !== null ? ctx.me.member !== null && sellerId === ctx.me.member : partyKeyHex(o.seller) === ctx.me.keyHex.toLowerCase();
    if (!forMe) return { ok: false, problem: 'not-for-me' };
    if (o.notAfterEpoch < ctx.epoch) return { ok: false, problem: 'expired' };
    return {
        ok: true,
        verified: { offer: o, tx, digestHex: toHex(digest), buyer: asKey(Array.from(hexToBytes(o.buyerKeyHex))), me: o.seller },
    };
}

/**
 * The envelope the seller submits: both signatures over the one digest, in
 * the order the transition names its parties (`tx.sale` / `tx.accept` plans
 * sign in that order too).
 */
export function offerEnvelope(v: VerifiedOffer, myKey: number[], mySignature: number[]): SignedTx {
    const buyerKey = Array.from(hexToBytes(v.offer.buyerKeyHex));
    const mine: [number[], number[]] = [myKey, mySignature];
    const theirs: [number[], number[]] = [buyerKey, v.offer.signature];
    const [first, second] = v.offer.lane === 'sale' ? [mine, theirs] : [theirs, mine];
    return {
        tx: v.tx as unknown as Record<string, unknown>,
        nonce: v.offer.nonce,
        not_after_epoch: v.offer.notAfterEpoch,
        signers: [first[0], second[0]],
        signatures: [first[1], second[1]],
    };
}

// ------------------------------------------------------------- stores ------

/** A code this device produced and may need to show again. */
export interface OpenOffer {
    payload: string;
    lane: OfferLane;
    seller: Party;
    amount: number;
    maturityEpochs: number;
    notAfterEpoch: number;
    createdSecs: number;
    /** True when the pool took it on the seller's invitation, so their app
     *  already shows it and the code is only a fallback. */
    sent?: boolean;
}

const OFFERS_KEY = 'edet.offers';

function loadOffers(): OpenOffer[] {
    const raw = lsGet(OFFERS_KEY);
    if (!raw) return [];
    try {
        const list = JSON.parse(raw);
        return Array.isArray(list) ? list.filter((o) => typeof o?.payload === 'string') : [];
    } catch {
        return [];
    }
}

/**
 * Codes this device signed that a seller may still complete. Kept on the
 * device because the node never sees them: a member who has not scanned yet
 * needs the code shown again, and a code the holder loses is a purchase that
 * silently never happens.
 */
export const myOffers = writable<OpenOffer[]>(loadOffers());
myOffers.subscribe((list) => lsSet(OFFERS_KEY, JSON.stringify(list)));

export function rememberOffer(o: OpenOffer): void {
    myOffers.update((list) => [o, ...list.filter((x) => x.payload !== o.payload)]);
}

/** Forgetting is local: a seller who already holds the code can still complete it. */
export function forgetOffer(payload: string): void {
    myOffers.update((list) => list.filter((x) => x.payload !== payload));
}

/** Drop the codes whose window has closed; nothing can complete them now. */
export function pruneOffers(epoch: number): void {
    myOffers.update((list) => (list.some((o) => o.notAfterEpoch < epoch) ? list.filter((o) => o.notAfterEpoch >= epoch) : list));
}

/** Codes scanned on this device and not yet signed or discarded. Session only. */
export const scannedOffers = writable<VerifiedOffer[]>([]);

export function addScannedOffer(v: VerifiedOffer): void {
    scannedOffers.update((list) => (list.some((x) => x.digestHex === v.digestHex) ? list : [...list, v]));
}

export function dropScannedOffer(digestHex: string): void {
    scannedOffers.update((list) => list.filter((x) => x.digestHex !== digestHex));
}
