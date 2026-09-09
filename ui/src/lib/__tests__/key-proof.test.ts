/**
 * The key-addressed viewer credential, cross-pinned against the node.
 *
 * This is the only credential onboarding can present — a device restoring
 * from a phrase, or waiting to be admitted, holds a key and no member id, so
 * neither the id-addressed header nor a bearer token is available to it. If
 * the two sides ever disagree about the signed bytes, onboarding stops
 * working entirely, and it fails as an opaque `forbidden` rather than as
 * anything that points at the message format. Hence a fixed vector.
 *
 * Pinned against `crates/node/src/serve/auth.rs::viewer_auth_message`, i.e.
 * `"edet-view-v2\n<chain>\n<METHOD> <path>\n<unix_secs>\n<nonce hex>"`, signed
 * directly with Ed25519 (no separate digest step — Ed25519 hashes internally).
 *
 * The nonce is the half that makes the credential single-use: the node admits
 * each verified signature once inside its window, and Ed25519 is
 * deterministic, so two honest reads of one path in one second would otherwise
 * be one credential.
 */
import { describe, expect, it } from 'vitest';
import * as ed from '@noble/ed25519';

import { keyProof, viewerAuthMessage, viewerChain } from '../session';
import { bytesToHex, derivePublicKey } from '../crypto';

describe('key-addressed viewer proof', () => {
    // Fixed seed, so the whole vector is reproducible.
    const seed = new Uint8Array(32).fill(7);
    const pubkey = derivePublicKey(seed);

    const NONCE = '0123456789abcdef0123456789abcdef';

    it('signs exactly the bytes the node recomputes', () => {
        viewerChain.set('edet-dev');
        expect(new TextDecoder().decode(viewerAuthMessage('GET', '/whois/abc', 1700000000, NONCE))).toBe(
            `edet-view-v2\nedet-dev\nGET /whois/abc\n1700000000\n${NONCE}`,
        );
    });

    it('produces a signature that verifies under the key it names', async () => {
        const ts = 1700000000;
        const path = `/whois/${bytesToHex(pubkey)}`;
        const proof = keyProof(pubkey, seed, 'GET', path, ts);

        // The node names the key from this header and checks the signature
        // against that key alone, so the two must agree.
        expect(proof.key).toBe(bytesToHex(pubkey));
        expect(proof.ts).toBe(ts);
        expect(proof.nonce).toMatch(/^[0-9a-f]{32}$/);
        expect(
            await ed.verifyAsync(proof.sig, viewerAuthMessage('GET', path, ts, proof.nonce), proof.key),
        ).toBe(true);
    });

    it('does not verify against a different path — the binding the node relies on', async () => {
        // A captured proof must not be replayable onto another endpoint;
        // `verify_signed_headers` gets that property purely from the path
        // being inside the signed bytes, so it has to actually be there.
        const ts = 1700000000;
        const proof = keyProof(pubkey, seed, 'GET', '/whois/abc', ts);
        expect(
            await ed.verifyAsync(proof.sig, viewerAuthMessage('GET', '/member/3', ts, proof.nonce), proof.key),
        ).toBe(false);
    });

    it('does not verify against a different timestamp', async () => {
        const proof = keyProof(pubkey, seed, 'GET', '/whois/abc', 1700000000);
        expect(
            await ed.verifyAsync(proof.sig, viewerAuthMessage('GET', '/whois/abc', 1700000001, proof.nonce), proof.key),
        ).toBe(false);
    });

    /** Two proofs over the identical read are two credentials, which is what
     *  the node's replay cache needs them to be. */
    it('mints a fresh nonce per proof, so two identical reads are two credentials', () => {
        const a = keyProof(pubkey, seed, 'GET', '/whois/abc', 1700000000);
        const b = keyProof(pubkey, seed, 'GET', '/whois/abc', 1700000000);
        expect(a.nonce).not.toBe(b.nonce);
        expect(a.sig).not.toBe(b.sig);
    });
});
