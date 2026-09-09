import { describe, expect, it } from 'vitest';

import { declineMessage } from '../submit';
import { bytesToHex } from '../crypto';

/**
 * Cross-pin against `crates/node/src/serve/pending.rs::decline_message`:
 *
 *   pub fn decline_message(digest: &[u8; 32]) -> [u8; 32] {
 *       let mut bytes = Vec::with_capacity(32 + 12);
 *       bytes.extend_from_slice(digest);
 *       bytes.extend_from_slice(b"edet-decline");
 *       sha256(&bytes)
 *   }
 *
 * i.e. sha256(digest ++ b"edet-decline"), where `sha256` is plain SHA-256
 * (crates/node/src/block.rs::sha256, a thin wrapper over `Sha256::finalize`).
 * `submit.ts::declineMessage` must produce byte-for-byte the same output —
 * this test pins that against an independently computed vector so the two
 * implementations can never silently drift apart.
 *
 * Fixed digest: the 32 bytes 0x00..0x1f (`00 01 02 ... 1f`).
 * Vector computed independently:
 *   python3 -c "
 *     import hashlib
 *     digest = bytes(range(32))
 *     print(hashlib.sha256(digest + b'edet-decline').hexdigest())
 *   "
 */
describe('declineMessage cross-pin (node/pending.rs::decline_message)', () => {
    const digestHex = Array.from({ length: 32 }, (_, i) => i.toString(16).padStart(2, '0')).join('');
    const expectedHex = 'ad8631756224977aaa36906d859f311af2caea7635032ce909f8db9bf3438a92';

    it('matches the independently computed sha256(digest ++ b"edet-decline") vector', () => {
        expect(digestHex).toBe('000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f');
        const msg = declineMessage(digestHex);
        expect(msg).toHaveLength(32);
        expect(bytesToHex(msg)).toBe(expectedHex);
    });
});
