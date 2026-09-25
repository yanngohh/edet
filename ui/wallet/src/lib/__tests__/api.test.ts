import { describe, expect, it } from 'vitest';

import { asKey, asMember, signPlanWithDigest, tx, proposalKinds, type ContractView } from '../api';
import { derivePublicKey, hexToBytes, verifyDigest } from '../crypto';

/** Arbitrary deterministic test seed for member `id`. */
const testSeed = (id: number): Uint8Array => new Uint8Array(32).fill((id + 10) & 0xff);

const contract = (over: Partial<ContractView> = {}): ContractView => ({
    id: 7,
    debtor: 2,
    creditor: 5,
    outstanding: 30,
    original: 30,
    status: 'active',
    maturity_epoch: 40,
    created_epoch: 10, accepted_epoch: 10,
    insured: true,
    arb: null,
    arb_attested: [],
    arb_awarded: false,
    ...over,
});

describe('tx builders', () => {
    it('accept is co-signed by debtor and creditor with serde field names', () => {
        const plan = tx.accept({ debtor: asMember(0), creditor: asMember(1), amount: 30, maturityEpochs: 30 });
        expect(plan.tx).toEqual({
            Accept: { debtor: { Member: 0 }, creditor: { Member: 1 }, amount: 30, maturity_epochs: 30, arb: null },
        });
        expect(plan.signers).toEqual([{ Member: 0 }, { Member: 1 }]);
    });

    it('sale is co-signed by seller and buyer', () => {
        const plan = tx.sale({ seller: asMember(3), buyer: asMember(4), amount: 12.5, maturityEpochs: 30 });
        expect(plan.tx).toEqual({
            Sale: { seller: { Member: 3 }, buyer: { Member: 4 }, amount: 12.5, maturity_epochs: 30 },
        });
        expect(plan.signers).toEqual([{ Member: 3 }, { Member: 4 }]);
    });

    /**
     * **A trade may name a counterparty by KEY, and that is what creates their
     * account.** There is no create-account transaction to build any
     * more: the key travels in the transaction itself, its holder signs like
     * any other party, and the row appears with the trade. The wire shape is
     * serde's external tagging, which is why this pins the literal.
     */
    it('a trade can name a counterparty who has no account yet', () => {
        const key = derivePublicKey(testSeed(9));
        const plan = tx.accept({ debtor: asKey(key), creditor: asMember(1), amount: 400, maturityEpochs: 30 });
        expect(plan.tx).toEqual({
            Accept: { debtor: { Key: key }, creditor: { Member: 1 }, amount: 400, maturity_epochs: 30, arb: null },
        });
        expect(plan.signers).toEqual([{ Key: key }, { Member: 1 }]);
    });

    it('settle / extend / cure sign both contract parties', () => {
        const c = contract();
        expect(tx.settle(c, 10).signers).toEqual([asMember(2), asMember(5)]);
        expect(tx.extend(c, 50).signers).toEqual([asMember(2), asMember(5)]);
        expect(tx.cure(c, 10).signers).toEqual([asMember(2), asMember(5)]);
        expect(tx.extend(c, 50).tx).toEqual({ Extend: { contract: 7, new_maturity_epoch: 50 } });
    });

    /// The creditor signs too. A transfer discharges the old debtor, and the
    /// node requires their consent whenever the successor would be uninsured —
    /// which the client cannot determine, so it always collects it. Over-
    /// collecting is harmless; under-collecting is a refused transaction the
    /// member cannot diagnose.
    it('transfer signs both debtors and the creditor', () => {
        const plan = tx.transfer(contract(), 9);
        expect(plan.tx).toEqual({ Transfer: { contract: 7, new_debtor: 9 } });
        expect(plan.signers).toEqual([asMember(2), asMember(5), asMember(9)]);
    });

    it('markExpired and rotateFinalize are permissionless', () => {
        expect(tx.markExpired(contract()).signers).toEqual([]);
        expect(tx.rotateFinalize(3).signers).toEqual([]);
    });

    it('rotateRequest requires the guardian threshold, not every guardian', () => {
        const plan = tx.rotateRequest(4, [Array(32).fill(1)], [7, 8, 9], 2);
        expect(plan.signers).toEqual([asMember(7), asMember(8), asMember(9)]);
        expect(plan.minSigs).toBe(2);
        // Threshold is clamped to the guardian count.
        expect(tx.rotateRequest(4, [Array(32).fill(1)], [7], 5).minSigs).toBe(1);
    });

    /**
     * **There is no create-account builder, and the alphabet no longer has the
     * transition.** One would be unbillable by construction, so it could
     * only ever be bounded by a ledger-wide per-epoch counter — a censorship
     * lever rather than a quota. A wallet that still offered it would build a
     * transaction every node refuses at decode.
     */
    it('has no account-creation builder at all', () => {
        expect((tx as Record<string, unknown>).openAccount).toBeUndefined();
    });

    it('arbAttest is signed by the arbiter alone', () => {
        const plan = tx.arbAttest(contract(), 8, 15);
        expect(plan.tx).toEqual({ ArbAttest: { contract: 7, arbiter: 8, amount: 15 } });
        expect(plan.signers).toEqual([asMember(8)]);
    });

    it('approveSupporter is signed by the beneficiary (moderation gate)', () => {
        const plan = tx.approveSupporter(4, 2, true);
        expect(plan.tx).toEqual({ ApproveSupporter: { beneficiary: 4, supporter: 2, approved: true } });
        expect(plan.signers).toEqual([asMember(4)]);
    });

    it('governance kinds use serde external tagging', () => {
        expect(proposalKinds.paramChange('XStar', 0.6)).toEqual({ ParamChange: { key: 'XStar', value: 0.6 } });
        expect(proposalKinds.validatorPower(2, 0)).toEqual({ ValidatorPower: { member: 2, power: 0 } });
    });
});

describe('signPlanWithDigest', () => {
    const digest = hexToBytes('ab'.repeat(32));
    const nonce = Array(16).fill(1);
    const notAfterEpoch = 30;

    it('signs with each unique signer and real ed25519 signatures', () => {
        const signed = signPlanWithDigest(
            { tx: { Exit: { member: 9 } }, signers: [asMember(1), asMember(1), asMember(2)] },
            digest,
            nonce,
            notAfterEpoch,
            (p) => testSeed(('Member' in p ? p.Member : 0) as number),
        );
        expect(signed.nonce).toEqual(nonce);
        expect(signed.not_after_epoch).toBe(notAfterEpoch);
        expect(signed.signers).toEqual([derivePublicKey(testSeed(1)), derivePublicKey(testSeed(2))]);
        expect(signed.signatures).toHaveLength(2);
        expect(verifyDigest(signed.signatures[0], digest, signed.signers[0])).toBe(true);
        expect(verifyDigest(signed.signatures[1], digest, signed.signers[1])).toBe(true);
        // Cross-check: signature 0 does not verify under signer 1's key.
        expect(verifyDigest(signed.signatures[0], digest, signed.signers[1])).toBe(false);
    });

    it('resolves seeds through the provided seedOf', () => {
        const custom = new Uint8Array(32).fill(77);
        const signed = signPlanWithDigest(
            { tx: { Exit: { member: 9 } }, signers: [asMember(3)] },
            digest,
            nonce,
            notAfterEpoch,
            () => custom,
        );
        expect(signed.signers).toEqual([derivePublicKey(custom)]);
        expect(verifyDigest(signed.signatures[0], digest, signed.signers[0])).toBe(true);
    });
});
