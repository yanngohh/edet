import { describe, expect, it } from 'vitest';

import { asKey, asMember } from '../api';
import { bytesToHex, derivePublicKey, hexToBytes, signDigest } from '../crypto';
import { encodeOffer, offerEnvelope, offerTx, parseOffer, verifyOffer, type Offer, type OfferContext } from '../offer';
import { toHex, txDigestLocal } from '../txdigest';

const CHAIN = 'edet-test-1';
const buyerSeed = new Uint8Array(32).fill(7);
const sellerSeed = new Uint8Array(32).fill(9);
const buyerKey = derivePublicKey(buyerSeed);
const buyerKeyHex = bytesToHex(Uint8Array.from(buyerKey));
const sellerKeyHex = bytesToHex(Uint8Array.from(derivePublicKey(sellerSeed)));

function signed(partial: Partial<Offer> = {}): Offer {
    const base: Offer = {
        chainId: CHAIN,
        lane: 'sale',
        seller: asMember(3),
        buyerKeyHex,
        amount: 50.25,
        maturityEpochs: 30,
        arb: null,
        nonce: Array.from({ length: 16 }, (_, i) => i + 1),
        notAfterEpoch: 20_700,
        signature: [],
        ...partial,
    };
    const digest = txDigestLocal(base.chainId, offerTx(base), base.nonce, base.notAfterEpoch);
    return { ...base, signature: signDigest(digest, buyerSeed) };
}

const ctx: OfferContext = { chainId: CHAIN, epoch: 20_680, me: { member: 3, keyHex: sellerKeyHex } };

describe('a buyer code', () => {
    it('round-trips through its text form', () => {
        const o = signed();
        expect(parseOffer(encodeOffer(o))).toEqual(o);
    });

    it('carries arbitration terms on the accept lane, and a seller named by key', () => {
        const o = signed({
            lane: 'accept',
            seller: asKey(Array.from(hexToBytes(sellerKeyHex))),
            arb: { arbiters: [4, 5], quorum: 2, window_epochs: 10, award_cap: 40 },
        });
        expect(parseOffer(encodeOffer(o))).toEqual(o);
        expect(offerTx(o)).toEqual({
            Accept: {
                debtor: asKey(buyerKey),
                creditor: o.seller,
                amount: 50.25,
                maturity_epochs: 30,
                arb: { arbiters: [4, 5], quorum: 2, window_epochs: 10, award_cap: 40 },
            },
        });
    });

    it('keeps an awkward chain id and an awkward amount exact', () => {
        const o = signed({ chainId: 'coöp ledger/2026&co', amount: 0.1 + 0.2 });
        const back = parseOffer(encodeOffer(o));
        expect(back?.chainId).toBe('coöp ledger/2026&co');
        expect(back?.amount).toBe(0.1 + 0.2);
    });

    it('is not something else', () => {
        expect(parseOffer('')).toBeNull();
        expect(parseOffer(`edet://join?c=x&k=${buyerKeyHex}`)).toBeNull();
        expect(parseOffer('https://example.com/?v=1')).toBeNull();
        const text = encodeOffer(signed());
        expect(parseOffer(text.replace('v=1', 'v=2'))).toBeNull();
        expect(parseOffer(text.replace('t=sale', 't=gift'))).toBeNull();
        expect(parseOffer(text.replace(/g=[0-9a-f]+/, 'g=abc'))).toBeNull();
        expect(parseOffer(text.replace(/a=[0-9.]+/, 'a=-5'))).toBeNull();
        expect(parseOffer(text.replace(/s=m3/, 's=q3'))).toBeNull();
    });
});

describe('checking a buyer code', () => {
    it('accepts a code signed for this seller on this chain', () => {
        const o = signed();
        const r = verifyOffer(o, ctx);
        expect(r.ok).toBe(true);
        if (!r.ok) return;
        expect(r.verified.me).toEqual(asMember(3));
        expect(r.verified.buyer).toEqual(asKey(buyerKey));
        expect(r.verified.digestHex).toBe(toHex(txDigestLocal(CHAIN, offerTx(o), o.nonce, o.notAfterEpoch)));
    });

    it('accepts a seller named by key when that key is this device', () => {
        const o = signed({ seller: asKey(Array.from(hexToBytes(sellerKeyHex))) });
        expect(verifyOffer(o, { ...ctx, me: { member: null, keyHex: sellerKeyHex } }).ok).toBe(true);
    });

    it('refuses a code for another chain before reading anything else', () => {
        expect(verifyOffer(signed({ chainId: 'other' }), ctx)).toEqual({ ok: false, problem: 'chain' });
    });

    it('refuses a code whose fields no longer match its signature', () => {
        const o = signed();
        expect(verifyOffer({ ...o, amount: 500.25 }, ctx)).toEqual({ ok: false, problem: 'signature' });
        expect(verifyOffer({ ...o, seller: asMember(4) }, { ...ctx, me: { member: 4, keyHex: sellerKeyHex } })).toEqual({
            ok: false,
            problem: 'signature',
        });
        expect(verifyOffer({ ...o, notAfterEpoch: o.notAfterEpoch + 1 }, ctx)).toEqual({ ok: false, problem: 'signature' });
    });

    it('refuses a code signed by a key other than the buyer it names', () => {
        const o = signed();
        const digest = txDigestLocal(CHAIN, offerTx(o), o.nonce, o.notAfterEpoch);
        expect(verifyOffer({ ...o, signature: signDigest(digest, sellerSeed) }, ctx)).toEqual({ ok: false, problem: 'signature' });
    });

    it('refuses a code that names a different seller', () => {
        expect(verifyOffer(signed(), { ...ctx, me: { member: 4, keyHex: sellerKeyHex } })).toEqual({ ok: false, problem: 'not-for-me' });
        expect(verifyOffer(signed(), { ...ctx, me: { member: null, keyHex: sellerKeyHex } })).toEqual({ ok: false, problem: 'not-for-me' });
    });

    it('refuses a code whose window has closed', () => {
        expect(verifyOffer(signed({ notAfterEpoch: 20_679 }), ctx)).toEqual({ ok: false, problem: 'expired' });
        expect(verifyOffer(signed({ notAfterEpoch: 20_680 }), ctx).ok).toBe(true);
    });
});

describe('the envelope the seller submits', () => {
    it('carries both signatures over the one digest, parties in the transition order', () => {
        const o = signed();
        const r = verifyOffer(o, ctx);
        if (!r.ok) throw new Error(r.problem);
        const myKey = derivePublicKey(sellerSeed);
        const mySig = signDigest(hexToBytes(r.verified.digestHex), sellerSeed);
        const env = offerEnvelope(r.verified, myKey, mySig);
        expect(env.tx).toEqual(offerTx(o));
        expect(env.nonce).toEqual(o.nonce);
        expect(env.not_after_epoch).toBe(o.notAfterEpoch);
        // Sale names seller then buyer.
        expect(env.signers).toEqual([myKey, buyerKey]);
        expect(env.signatures).toEqual([mySig, o.signature]);
    });

    it('puts the debtor first on the accept lane', () => {
        const o = signed({ lane: 'accept', arb: { arbiters: [4], quorum: 1, window_epochs: 5, award_cap: 10 } });
        const r = verifyOffer(o, ctx);
        if (!r.ok) throw new Error(r.problem);
        const myKey = derivePublicKey(sellerSeed);
        const env = offerEnvelope(r.verified, myKey, [1]);
        expect(env.signers).toEqual([buyerKey, myKey]);
        expect(env.signatures).toEqual([o.signature, [1]]);
    });
});
