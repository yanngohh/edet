/**
 * Client-side verification of state-root inclusion proofs.
 *
 * The verifying half of `crates/state/src/root.rs`, which is the normative
 * source: every constant, preimage layout and fold rule here reimplements
 * that file byte for byte, and any format change lands there first. The
 * design is the paper's §Implementation, deciding
 * the paper's §Implementation.
 *
 * What a verified proof establishes: the exact byte string `value` was
 * committed at `key` in `section` of the state whose root is the 32 bytes it
 * was checked against — nothing less, nothing more. `value` is the record's
 * `bincode` encoding, produced by the Rust state machine, and this module
 * treats it as opaque: interpreting those bytes is the node's or an
 * arbitrator's job. A client-side decoder would be a second implementation
 * of the Rust type layout, wrong the day a field is added, and wrong
 * silently — while the proof's guarantee is about the bytes, not about any
 * reading of them.
 *
 * Why the client checks instead of trusting: the root is `Replica::app_hash`,
 * the value a validator set certifies every block. A member who keeps a
 * proof and the certified root it verifies against holds evidence of what
 * was committed that outlives any later story about the ledger — but only
 * if the check actually happens on the member's side. Hence this module is
 * pure functions over bytes: no network, no stores, nothing a transport or
 * UI refactor can quietly break.
 *
 * A proof carries hashes and a claimed position, never structure. Which
 * side each sibling is on, and how many siblings each fold consumes, is
 * DERIVED here from `(index, leaf_count)` and from the section's fixed
 * place in the top tree — and the position is committed inside the leaf
 * hash while the count is committed at the section root, so both are part
 * of what the root signed rather than claims the proof gets to make. A
 * proof that could declare its own structure could relocate a leaf, resize
 * its tree, or move hashes across the boundary between the two folds.
 *
 * Malformed input is `null`/`false`, never a throw: a proof is a wire
 * object from an untrusted source, and "this input crashed the verifier"
 * must not be an outcome it can choose.
 */
import { sha256 } from '@noble/hashes/sha2';

import { hexToBytes } from './crypto';

/**
 * The sections, in the exact order of `Section::ALL` in `root.rs`. A
 * section's position in this list is its index in the top tree, and it is
 * derived from the section's NAME here, never read from the proof.
 */
export const SECTIONS = ['members', 'contracts', 'proposals', 'validators', 'stakes', 'replay', 'ledger'] as const;
export type Section = (typeof SECTIONS)[number];

/**
 * The tag byte inside every leaf preimage, mirroring `Section::tag`.
 *
 * A SEPARATE map from the order above, and the two do NOT agree: `stakes`,
 * `replay` and `ledger` sit at positions 4, 5 and 6 and carry tags 5, 6 and
 * 4. The node assigns tags by an explicit `match` precisely so that
 * reordering the enum cannot silently renumber them, and a client that
 * derived one from the other would throw that guarantee away — computing
 * leaf hashes nobody committed to, while every members-section fixture went
 * on passing.
 *
 * The LENGTH of `SECTIONS` is load-bearing in its own right: it is the leaf
 * count of the top tree, so a list one entry longer than `Section::ALL` folds
 * every `section_path` at the wrong shape and refuses every genuine proof the
 * node serves — a shipped client that reads nothing, with both suites green,
 * because a hand-copied fixture goes stale in exactly the same way as the code
 * it pins. `just proof-fixture-check` is the gate: the fixture is GENERATED
 * from the normative implementation, so it cannot drift alongside this file.
 */
export const SECTION_TAG: Record<Section, number> = {
    members: 0,
    contracts: 1,
    proposals: 2,
    validators: 3,
    stakes: 5,
    replay: 6,
    ledger: 4,
};

/**
 * RFC 6962 leaf/inner separation, as in `root.rs`: without distinct
 * prefixes an inner node's preimage could be presented as a leaf, which is
 * a known inclusion-proof forgery.
 */
const LEAF_PREFIX = 0x00;
const INNER_PREFIX = 0x01;
/**
 * The section root is a third kind of node in the same tree, and takes its
 * own prefix and domain for the reason a leaf is separated from an inner
 * node. It is where a section's leaf COUNT is bound — see `sectionHash`.
 */
const SECTION_PREFIX = 0x02;
/** Pinned to `LEAF_DOMAIN` in `root.rs`; a bump there is a new format. */
const LEAF_DOMAIN = new TextEncoder().encode('edet-leaf-v3');
/** Pinned to `SECTION_DOMAIN` in `root.rs`. */
const SECTION_DOMAIN = new TextEncoder().encode('edet-section-v1');

/**
 * One record's proof, decoded from the wire. Field meanings are those of
 * `InclusionProof` in `root.rs`; byte fields arrive as hex strings and are
 * held decoded here.
 */
export interface InclusionProof {
    section: Section;
    /** The leaf's position among its section's leaves — its key's rank. */
    index: number;
    /** How many leaves the section held. */
    leafCount: number;
    key: Uint8Array;
    /** The record's `bincode` bytes. Opaque here — see the module comment. */
    value: Uint8Array;
    /**
     * The salt disclosed for this one leaf in this one epoch. It lets the
     * leaf hash be recomputed without `root_salt`, and unblinds nothing
     * else — see `leaf_salt` in `root.rs`.
     */
    leafSalt: Uint8Array;
    /** Sibling hashes, leaf up to its section's leaves root. */
    path: Uint8Array[];
    /** Sibling hashes, that section's root up to the state root. */
    sectionPath: Uint8Array[];
}

/** A u64 as 8 big-endian bytes — `be_len`/`to_be_bytes` in `root.rs`. */
function be64(n: number): Uint8Array {
    const out = new Uint8Array(8);
    new DataView(out.buffer).setBigUint64(0, BigInt(n));
    return out;
}

/**
 * The leaf preimage, laid out exactly as `leaf_hash` in `root.rs`: every
 * fixed-width component at a fixed offset, every variable-length component
 * preceded by its u64 big-endian length. The framing is what makes the
 * preimage injective — the key/value boundary cannot slide — and `index`
 * being inside it is what commits the leaf's position, so a path cannot be
 * replayed at another one.
 *
 * The section's leaf COUNT is NOT here: it is bound once, at the section
 * root (`sectionHash`). That is what lets a leaf survive its section
 * growing, which is what makes the node's tree incremental — and the count
 * is still part of what the root signed, so a proof cannot resize its own
 * tree.
 */
function leafHash(salt: Uint8Array, tag: number, index: number, key: Uint8Array, value: Uint8Array): Uint8Array {
    const h = sha256.create();
    h.update(Uint8Array.of(LEAF_PREFIX));
    h.update(LEAF_DOMAIN);
    h.update(salt);
    h.update(Uint8Array.of(tag));
    h.update(be64(index));
    h.update(be64(key.length));
    h.update(key);
    h.update(be64(value.length));
    h.update(value);
    return h.digest();
}

/**
 * A section's root — `section_hash` in `root.rs`: its tag, its leaf COUNT,
 * and the root of its leaves.
 *
 * The count is bound here and nowhere else. Which side each sibling sits on
 * is a function of the index alone, so two different counts can fold a leaf
 * to the same leaves root — index 0 does so at three leaves and at four.
 * This is what separates them.
 */
function sectionHash(tag: number, count: number, leavesRoot: Uint8Array): Uint8Array {
    const h = sha256.create();
    h.update(Uint8Array.of(SECTION_PREFIX));
    h.update(SECTION_DOMAIN);
    h.update(Uint8Array.of(tag));
    h.update(be64(count));
    h.update(leavesRoot);
    return h.digest();
}

function innerHash(left: Uint8Array, right: Uint8Array): Uint8Array {
    const h = sha256.create();
    h.update(Uint8Array.of(INNER_PREFIX));
    h.update(left);
    h.update(right);
    return h.digest();
}

/**
 * Fold one node up a promotion tree of `count` leaves from position
 * `index`, consuming exactly the siblings the tree's shape dictates —
 * `fold_path` in `root.rs`, shape rule and all. `null` when the claimed
 * shape and the supplied siblings disagree: an out-of-range index, a
 * missing sibling, or one left over. A leftover sibling is a forgery
 * attempt, not padding.
 *
 * Odd levels PROMOTE the last node: it rises a level and consumes no
 * sibling. Promotion rather than Bitcoin's duplication (CVE-2012-2459), so
 * two distinct trees never share a root.
 */
function foldPath(leaf: Uint8Array, index: number, count: number, siblings: Uint8Array[]): Uint8Array | null {
    if (index >= count) {
        return null;
    }
    let acc = leaf;
    let t = index;
    let m = count;
    let i = 0;
    while (m > 1) {
        if (t % 2 === 0) {
            if (t + 1 < m) {
                if (i >= siblings.length) {
                    return null;
                }
                acc = innerHash(acc, siblings[i]);
                i += 1;
            }
            // else: the last node of an odd level, promoted with no sibling.
        } else {
            if (i >= siblings.length) {
                return null;
            }
            acc = innerHash(siblings[i], acc);
            i += 1;
        }
        t = Math.floor(t / 2);
        m = Math.ceil(m / 2);
    }
    return i === siblings.length ? acc : null;
}

/** Decode a hex field; `null` for a non-string, bad hex, or a wrong length. */
function bytesFromHex(v: unknown, exact?: number): Uint8Array | null {
    if (typeof v !== 'string') {
        return null;
    }
    let bytes: Uint8Array;
    try {
        bytes = hexToBytes(v);
    } catch {
        return null;
    }
    if (exact !== undefined && bytes.length !== exact) {
        return null;
    }
    return bytes;
}

/** Decode an array of 32-byte hex hashes; `null` if any element is not one. */
function hashPath(v: unknown): Uint8Array[] | null {
    if (!Array.isArray(v)) {
        return null;
    }
    const out: Uint8Array[] = [];
    for (const el of v) {
        const h = bytesFromHex(el, 32);
        if (h === null) {
            return null;
        }
        out.push(h);
    }
    return out;
}

/** A u64 off the wire: a non-negative integer, exactly representable. */
function u64(v: unknown): number | null {
    return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 ? v : null;
}

/**
 * Decode a wire proof. Total: `null` for anything malformed — a missing
 * field, bad hex, a hash that is not 32 bytes, a negative or non-integer
 * position, an unknown section — never a throw. The section name is matched
 * case-insensitively and never defaulted: a section this verifier does not
 * know is a proof it cannot check, not a proof about members.
 */
export function parseProof(json: unknown): InclusionProof | null {
    if (typeof json !== 'object' || json === null || Array.isArray(json)) {
        return null;
    }
    const o = json as Record<string, unknown>;

    if (typeof o.section !== 'string') {
        return null;
    }
    const name = o.section.toLowerCase();
    const sectionIndex = (SECTIONS as readonly string[]).indexOf(name);
    if (sectionIndex < 0) {
        return null;
    }

    const index = u64(o.index);
    const leafCount = u64(o.leaf_count);
    const key = bytesFromHex(o.key);
    const value = bytesFromHex(o.value);
    const leafSalt = bytesFromHex(o.leaf_salt, 32);
    const path = hashPath(o.path);
    const sectionPath = hashPath(o.section_path);
    if (
        index === null ||
        leafCount === null ||
        key === null ||
        value === null ||
        leafSalt === null ||
        path === null ||
        sectionPath === null
    ) {
        return null;
    }

    return { section: SECTIONS[sectionIndex], index, leafCount, key, value, leafSalt, path, sectionPath };
}

/**
 * Check a proof against a root — `verify` in `root.rs`. Needs nothing else:
 * no node, no ledger, no secret.
 *
 * Two folds and one section hash, each against a shape the proof does not
 * get to choose: the leaf's fold is derived from its committed `index` and
 * the `leafCount` that must then hash into the section root, and the top
 * fold from the section's fixed position in the seven-section tree. Each
 * fold consumes exactly the siblings its shape dictates, so a hash cannot
 * be moved across the boundary between them, and a path cannot be replayed
 * at another position or against a tree of another size.
 *
 * The structural checks are repeated here even though `parseProof` already
 * makes them, so a proof constructed by hand gets no laxer treatment than
 * one off the wire.
 */
export function verifyProof(rootHex: string, proof: InclusionProof): boolean {
    const root = bytesFromHex(rootHex, 32);
    if (root === null) {
        return false;
    }
    if (u64(proof.index) === null || u64(proof.leafCount) === null) {
        return false;
    }
    if (proof.leafSalt.length !== 32) {
        return false;
    }
    if (proof.path.some((h) => h.length !== 32) || proof.sectionPath.some((h) => h.length !== 32)) {
        return false;
    }
    const sectionIndex = (SECTIONS as readonly string[]).indexOf(proof.section);
    if (sectionIndex < 0) {
        return false;
    }

    const leaf = leafHash(proof.leafSalt, SECTION_TAG[proof.section], proof.index, proof.key, proof.value);
    const leavesRoot = foldPath(leaf, proof.index, proof.leafCount, proof.path);
    if (leavesRoot === null) {
        return false;
    }
    const sectionRoot = sectionHash(SECTION_TAG[proof.section], proof.leafCount, leavesRoot);
    const top = foldPath(sectionRoot, sectionIndex, SECTIONS.length, proof.sectionPath);
    if (top === null) {
        return false;
    }
    return constantTimeEqual(top, root);
}

/** Decode and check in one step; `false` for malformed input. */
export function verifyProofJson(rootHex: string, json: unknown): boolean {
    const proof = parseProof(json);
    return proof === null ? false : verifyProof(rootHex, proof);
}

/**
 * Constant-time-ish comparison. The verifier holds no secret, so this is
 * hygiene rather than a hard requirement — but a compare that returns at
 * the first differing byte is not a habit worth exporting.
 */
function constantTimeEqual(a: Uint8Array, b: Uint8Array): boolean {
    if (a.length !== b.length) {
        return false;
    }
    let diff = 0;
    for (let i = 0; i < a.length; i++) {
        diff |= a[i] ^ b[i];
    }
    return diff === 0;
}
