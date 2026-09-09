import { describe, expect, it } from 'vitest';

import { buildAdmitQrPayload, parseAdmitInput } from '../invite';

const KEY = 'ab'.repeat(32);

describe('admission QR payloads', () => {
    it('round-trips key and chain id through the payload', () => {
        const link = buildAdmitQrPayload('edet-dev-1', KEY);
        expect(parseAdmitInput(link)).toEqual({ keyHex: KEY, chainId: 'edet-dev-1' });
    });

    it('percent-encodes chain ids the URI would otherwise break on', () => {
        const chain = 'coöp ledger/2026&co';
        expect(parseAdmitInput(buildAdmitQrPayload(chain, KEY))).toEqual({ keyHex: KEY, chainId: chain });
    });

    it('lowercases the key on both ends', () => {
        expect(buildAdmitQrPayload('c', KEY.toUpperCase())).toContain(KEY);
        const link = `edet://join?c=c&k=${KEY.toUpperCase()}`;
        expect(parseAdmitInput(link)?.keyHex).toBe(KEY);
    });

    it('accepts a bare 64-hex key, with whitespace and any case', () => {
        expect(parseAdmitInput(`  ${KEY.toUpperCase()}  `)).toEqual({ keyHex: KEY, chainId: null });
    });

    it('accepts a link with no chain id as chain-unknown', () => {
        expect(parseAdmitInput(`edet://join?k=${KEY}`)).toEqual({ keyHex: KEY, chainId: null });
    });

    it('rejects everything else', () => {
        expect(parseAdmitInput('')).toBeNull();
        expect(parseAdmitInput('ab'.repeat(31))).toBeNull(); // short key
        expect(parseAdmitInput(`0x${'ab'.repeat(20)}`)).toBeNull(); // wallet address, not a key
        expect(parseAdmitInput(`edet://join?c=only-chain`)).toBeNull(); // link without key
        expect(parseAdmitInput(`edet://join?k=${'zz'.repeat(32)}`)).toBeNull(); // non-hex key
        expect(parseAdmitInput(`https://example.com/?k=${KEY}`)).toBeNull(); // foreign scheme
    });
});

import { buildPayQrPayload, inviteMessage, inviteWire, mintInvite, parsePayInput } from '../invite';
import { bytesToHex, derivePublicKey, verifyDigest } from '../crypto';

describe('the seller invitation', () => {
    it('signs the cross-pinned message bytes', () => {
        // The same fixed inputs as `serve/pending.rs`'s
        // `invite_message_matches_the_cross_pinned_vector`.
        const msg = inviteMessage('edet-dev-1', '01'.repeat(32), 1_700_000_000, '0a'.repeat(16));
        expect(bytesToHex(msg)).toBe(
            '656465742d696e766974652d76310a656465742d6465762d310a303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130313031303130310a313730303030303030300a3061306130613061306130613061306130613061306130613061306130613061',
        );
    });

    it('mints an invitation the inviting key verifies', () => {
        const seed = new Uint8Array(32).fill(5);
        const inv = mintInvite(seed, 'edet-dev-1', 1_800_000_000);
        expect(inv.keyHex).toBe(bytesToHex(Uint8Array.from(derivePublicKey(seed))));
        const msg = inviteMessage('edet-dev-1', inv.keyHex, inv.notAfterSecs, inv.nonceHex);
        expect(verifyDigest(inviteWire(inv).signature, msg, inviteWire(inv).key)).toBe(true);
        // Not for another chain, and not with the expiry moved.
        expect(verifyDigest(inviteWire(inv).signature, inviteMessage('other', inv.keyHex, inv.notAfterSecs, inv.nonceHex), inviteWire(inv).key)).toBe(false);
        expect(verifyDigest(inviteWire(inv).signature, inviteMessage('edet-dev-1', inv.keyHex, inv.notAfterSecs + 1, inv.nonceHex), inviteWire(inv).key)).toBe(false);
    });

    it('rides the pay QR and comes back whole', () => {
        const inv = mintInvite(new Uint8Array(32).fill(6), 'coöp ledger/2026&co', 1_800_000_000);
        const address = `0x${'ab'.repeat(20)}`;
        const payload = buildPayQrPayload('coöp ledger/2026&co', address, inv);
        expect(parsePayInput(payload)).toEqual({ address, chainId: 'coöp ledger/2026&co', invite: inv });
    });

    it('is a bare address without an invitation, which is not a pay payload', () => {
        const address = `0x${'ab'.repeat(20)}`;
        expect(buildPayQrPayload('c', address, null)).toBe(address);
        expect(parsePayInput(address)).toBeNull();
    });

    it('drops a damaged invitation but keeps the address', () => {
        const inv = mintInvite(new Uint8Array(32).fill(7), 'c', 1_800_000_000);
        const address = `0x${'cd'.repeat(20)}`;
        const payload = buildPayQrPayload('c', address, inv).replace(/s=[0-9a-f]+/, 's=abc');
        expect(parsePayInput(payload)).toEqual({ address, chainId: 'c', invite: null });
        expect(parsePayInput(`edet://pay?c=c&a=notanaddress`)).toBeNull();
        expect(parsePayInput(`edet://join?c=c&k=${'ab'.repeat(32)}`)).toBeNull();
    });
});
