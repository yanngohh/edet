/**
 * The digest this device signs, computed HERE.
 *
 * **A wallet that asks a node what to sign is not a wallet.** The app embeds
 * no node: it reads one the member CHOOSES, and `custom` in `networks.ts` is
 * any URL a member can be talked into typing. A client that fetched its signing
 * payload from that node would be handed whatever the node liked — the digest
 * of `Accept { debtor: victim, creditor: attacker, amount: 10000 }` under a
 * nonce and window of the node's choosing — and would return a valid signature
 * over it. `/tx/check` is no defence, because the same node answers it. The
 * trust the client extends a node is that it can withhold and omit and never
 * forge, and asking it what to sign gives that away.
 *
 * So this file is the normative encoding, re-implemented. That is a real cost —
 * two encoders that must agree byte for byte — and it is paid the way this tree
 * pays such costs everywhere else: with a cross-pin whose vectors are generated
 * by the Rust side (`crates/node/examples/tx_digest_fixture.rs`), pinned here
 * (`__tests__/tx-digest.test.ts`), and gated in CI (`just tx-digest-check`).
 * Drift in either encoder goes red instead of quietly opening the hole.
 *
 * The encoding is `bincode` 1.3 with its default configuration. The normative
 * statement of it — and the ONE place the Rust side names that dependency — is
 * `crates/state/src/codec.rs`; this is its reimplementation:
 *
 *   - integers fixed-width, little-endian; `f64` as IEEE-754 LE;
 *   - an enum variant as its declaration index, `u32` LE, then its fields;
 *   - `Vec`, `String`, `BTreeSet`, `BTreeMap` prefixed by a `u64` LE count;
 *   - a fixed-size array (`[u8; 32]`) as its bytes, with NO prefix;
 *   - `Option` as one byte, 0 or 1, then the value;
 *   - structs and tuples as their fields in declaration order.
 *
 * And the digest is `sha256(TX_DOMAIN || len(chain_id) u64 LE || chain_id ||
 * codec::encode((tx, nonce, not_after_epoch)))` — `block.rs::tx_digest`. The length
 * prefix on the chain id is not decoration: without it `"ab"` followed by one
 * payload collides with `"a"` followed by another that starts with `b`.
 */

import { sha256 } from '@noble/hashes/sha256';

import type { ArbTermsView, Key, Party, Tx } from './api';

/** `crates/node/src/block.rs::TX_DOMAIN`. */
const TX_DOMAIN = 'edet-tx-v1';

/** A growable little-endian byte writer — the whole of the encoder's state. */
class Out {
    private parts: Uint8Array[] = [];
    private len = 0;

    bytes(b: Uint8Array | number[]): void {
        const a = b instanceof Uint8Array ? b : Uint8Array.from(b);
        this.parts.push(a);
        this.len += a.length;
    }

    u8(n: number): void {
        this.bytes(Uint8Array.of(n & 0xff));
    }

    u32(n: number): void {
        const b = new Uint8Array(4);
        new DataView(b.buffer).setUint32(0, n, true);
        this.bytes(b);
    }

    /**
     * A `u64`, written from a JS number. Exact for everything the alphabet
     * carries — contract and member ids, epochs, powers, counts — and it
     * REFUSES anything past `Number.MAX_SAFE_INTEGER` rather than writing a
     * rounded value: a silently rounded id would produce a digest that signs a
     * transaction nobody meant.
     */
    u64(n: number): void {
        if (!Number.isSafeInteger(n) || n < 0) {
            throw new TxEncodeError(`${n} is not a u64 this encoder can represent exactly`);
        }
        const b = new Uint8Array(8);
        new DataView(b.buffer).setBigUint64(0, BigInt(n), true);
        this.bytes(b);
    }

    f64(n: number): void {
        const b = new Uint8Array(8);
        new DataView(b.buffer).setFloat64(0, n, true);
        this.bytes(b);
    }

    bool(v: boolean): void {
        this.u8(v ? 1 : 0);
    }

    /** A bincode enum variant tag: its declaration index as a `u32` LE. */
    variant(index: number): void {
        this.u32(index);
    }

    /** A length-prefixed sequence: `u64` count, then the elements. */
    seq<T>(items: readonly T[], write: (x: T) => void): void {
        this.u64(items.length);
        for (const x of items) write(x);
    }

    /** A fixed-size byte array: the bytes, no prefix. */
    fixed(bytes: Key, expected: number): void {
        if (bytes.length !== expected) {
            throw new TxEncodeError(`expected ${expected} bytes, got ${bytes.length}`);
        }
        this.bytes(bytes);
    }

    finish(): Uint8Array {
        const out = new Uint8Array(this.len);
        let at = 0;
        for (const p of this.parts) {
            out.set(p, at);
            at += p.length;
        }
        return out;
    }
}

/**
 * The encoder refused to encode. Thrown rather than returned, and never
 * caught into a fallback: a transaction whose digest cannot be computed is one
 * this device must not sign, and asking the node for it is the hole this file
 * exists to keep shut.
 */
export class TxEncodeError extends Error {
    constructor(message: string) {
        super(`transaction encoding: ${message}`);
        this.name = 'TxEncodeError';
    }
}

/** `Party`: `Member(MemberId)` is 0, `Key([u8; 32])` is 1. */
function party(o: Out, p: Party): void {
    if ('Member' in p) {
        o.variant(0);
        o.u64(p.Member);
    } else {
        o.variant(1);
        o.fixed(p.Key, 32);
    }
}

/**
 * `Option<ArbTerms>`. `arbiters` is a `BTreeSet<MemberId>`, so it encodes as a
 * length-prefixed sequence in ASCENDING order — a set's iteration order is part
 * of its encoding, and a client that sent them in click order would compute a
 * different digest from the one the node checks.
 */
function arbTerms(o: Out, t: ArbTermsView | null): void {
    if (t === null || t === undefined) {
        o.u8(0);
        return;
    }
    o.u8(1);
    const arbiters = [...t.arbiters].sort((a, b) => a - b);
    o.seq(arbiters, (a) => o.u64(a));
    o.u32(t.quorum);
    o.u64(t.window_epochs);
    o.f64(t.award_cap);
}

/** `ParamKey`, in declaration order (`crates/state/src/types.rs`). */
const PARAM_KEYS = ['RiskK', 'SealAmounts', 'BondFraction', 'StakeDecay', 'SeedRate', 'InsuredHorizon'] as const;

/** `ProposalKind`, in declaration order. */
function proposalKind(o: Out, kind: Record<string, unknown>): void {
    const [name, body] = Object.entries(kind)[0] ?? [];
    const fields = (body ?? {}) as Record<string, unknown>;
    switch (name) {
        case 'ParamChange': {
            o.variant(0);
            const key = PARAM_KEYS.indexOf(fields.key as (typeof PARAM_KEYS)[number]);
            if (key < 0) throw new TxEncodeError(`unknown ParamKey ${String(fields.key)}`);
            o.variant(key);
            o.f64(fields.value as number);
            return;
        }
        case 'Redenominate':
            o.variant(1);
            o.u64(fields.num as number);
            o.u64(fields.den as number);
            return;
        case 'Suspend':
            o.variant(2);
            o.u64(fields.member as number);
            return;
        case 'Unsuspend':
            o.variant(3);
            o.u64(fields.member as number);
            return;
        case 'ValidatorPower':
            o.variant(4);
            o.u64(fields.member as number);
            o.u64(fields.power as number);
            return;
        case 'SeedAmendment':
            o.variant(5);
            o.f64(fields.amount as number);
            return;
        default:
            throw new TxEncodeError(`unknown ProposalKind ${String(name)}`);
    }
}

/**
 * `Tx`, in declaration order (`crates/state/src/tx.rs`). The order IS the
 * encoding, so this switch is written in that order and not alphabetically:
 * reordering it silently changes what every signature covers.
 */
export function encodeTx(tx: Tx): Uint8Array {
    const o = new Out();
    writeTx(o, tx);
    return o.finish();
}

function writeTx(o: Out, tx: Tx): void {
    const anyTx = tx as Record<string, Record<string, unknown>>;
    const name = Object.keys(anyTx)[0];
    const f = anyTx[name] ?? {};
    switch (name) {
        case 'RegisterGuardians':
            o.variant(0);
            o.u64(f.member as number);
            // `guardians: Vec<MemberId>` on the wire — a Vec, not a set, so it
            // is encoded exactly as given rather than sorted.
            o.seq(f.guardians as number[], (g) => o.u64(g));
            o.u32(f.threshold as number);
            o.u64(f.veto_window_epochs as number);
            return;
        case 'RotateRequest':
            o.variant(1);
            o.u64(f.member as number);
            o.seq(f.new_keys as Key[], (k) => o.fixed(k, 32));
            return;
        case 'RotateVeto':
            o.variant(2);
            o.u64(f.member as number);
            return;
        case 'RotateFinalize':
            o.variant(3);
            o.u64(f.member as number);
            return;
        case 'SetConsensusKey': {
            o.variant(4);
            o.u64(f.member as number);
            const key = f.key as Key | null;
            if (key === null || key === undefined) {
                o.u8(0);
            } else {
                o.u8(1);
                o.fixed(key, 32);
            }
            return;
        }
        case 'Exit':
            o.variant(5);
            o.u64(f.member as number);
            return;
        case 'ListBeneficiaries':
            o.variant(6);
            o.u64(f.supporter as number);
            o.seq(f.entries as [number, number][], ([id, w]) => {
                o.u64(id);
                o.f64(w);
            });
            return;
        case 'ApproveSupporter':
            o.variant(7);
            o.u64(f.beneficiary as number);
            o.u64(f.supporter as number);
            o.bool(f.approved as boolean);
            return;
        case 'Sale':
            o.variant(8);
            party(o, f.seller as Party);
            party(o, f.buyer as Party);
            o.f64(f.amount as number);
            o.u64(f.maturity_epochs as number);
            return;
        case 'DeclareSupply':
            o.variant(9);
            o.u64(f.member as number);
            o.f64(f.supply as number);
            return;
        case 'Accept':
            o.variant(10);
            party(o, f.debtor as Party);
            party(o, f.creditor as Party);
            o.f64(f.amount as number);
            o.u64(f.maturity_epochs as number);
            arbTerms(o, (f.arb ?? null) as ArbTermsView | null);
            return;
        case 'Transfer':
            o.variant(11);
            o.u64(f.contract as number);
            o.u64(f.new_debtor as number);
            return;
        case 'Settle':
            o.variant(12);
            o.u64(f.contract as number);
            o.f64(f.amount as number);
            return;
        case 'Extend':
            o.variant(13);
            o.u64(f.contract as number);
            o.u64(f.new_maturity_epoch as number);
            return;
        case 'MarkExpired':
            o.variant(14);
            o.u64(f.contract as number);
            return;
        case 'Cure':
            o.variant(15);
            o.u64(f.contract as number);
            o.f64(f.amount as number);
            return;
        case 'ArbAttest':
            o.variant(16);
            o.u64(f.contract as number);
            o.u64(f.arbiter as number);
            o.f64(f.amount as number);
            return;
        case 'Propose':
            o.variant(17);
            o.u64(f.author as number);
            proposalKind(o, f.kind as Record<string, unknown>);
            return;
        case 'Assent':
            o.variant(18);
            o.u64(f.member as number);
            o.u64(f.proposal as number);
            return;
        case 'ForfeitBonds':
            o.variant(19);
            o.u64(f.member as number);
            return;
        default:
            throw new TxEncodeError(`unknown transaction ${String(name)}`);
    }
}

/**
 * The bytes this device signs: `sha256(TX_DOMAIN || len(chain_id) || chain_id
 * || bincode((tx, nonce, not_after_epoch)))`.
 *
 * `nonce` is a fixed `[u8; 16]`, so it carries no length prefix.
 */
export function txDigestLocal(chainId: string, tx: Tx, nonce: number[], notAfterEpoch: number): Uint8Array {
    if (nonce.length !== 16) throw new TxEncodeError(`a nonce is 16 bytes, got ${nonce.length}`);
    const o = new Out();
    o.bytes(new TextEncoder().encode(TX_DOMAIN));
    const chain = new TextEncoder().encode(chainId);
    o.u64(chain.length);
    o.bytes(chain);
    writeTx(o, tx);
    o.fixed(nonce, 16);
    o.u64(notAfterEpoch);
    return sha256(o.finish());
}

/** Lowercase hex, the shape `/pending` serves a digest in. */
export function toHex(bytes: Uint8Array): string {
    return Array.from(bytes)
        .map((b) => b.toString(16).padStart(2, '0'))
        .join('');
}
