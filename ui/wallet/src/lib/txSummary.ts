/**
 * Human-readable one-liner for a raw transaction (as it appears in a
 * pending signature request). Pure: the caller supplies the translator,
 * the member-name resolver, and the number formatter so the summary is
 * localized and reactive.
 */

export interface SummaryDeps {
    t: (key: string, opts?: { values?: Record<string, any>; default?: string }) => string;
    nameOf: (id: number | null | undefined) => string;
    fmt: (n: number, decimals?: number) => string;
    /**
     * An ABSOLUTE epoch as the day it is (`lib/epoch.ts`). Supplied like the
     * others rather than imported, so this stays pure: the epoch's length is
     * the node's answer, and a summary must not reach for a store to get it.
     */
    dateOf: (epoch: number) => string;
}

export function txSummary(tx: Record<string, any>, deps: SummaryDeps): string {
    const { t, nameOf, fmt, dateOf } = deps;
    const kind = Object.keys(tx)[0];
    const b = tx[kind] ?? {};
    /**
     * Name a PARTY. A trade may name a counterparty by key — the
     * newcomer this very trade will seat — and there is no member to look up
     * for somebody the ledger has not met yet, so they are named as what they
     * are. A summary that silently rendered them as an id would be describing
     * a different transaction than the one being signed.
     */
    const partyName = (p: any): string => {
        if (p && typeof p === 'object' && 'Key' in p) {
            const hex = (p.Key as number[]).map((x) => x.toString(16).padStart(2, '0')).join('');
            return t('txkind.newAccount', {
                values: { key: `${hex.slice(0, 8)}…` },
                default: `a new account (${hex.slice(0, 8)}…)`,
            });
        }
        return nameOf(p && typeof p === 'object' && 'Member' in p ? p.Member : p);
    };
    switch (kind) {
        case 'Accept':
            return t('txkind.accept', {
                values: {
                    debtor: partyName(b.debtor),
                    creditor: partyName(b.creditor),
                    amount: fmt(b.amount),
                    maturity: b.maturity_epochs,
                },
                default: `${partyName(b.debtor)} takes on a debt of ${fmt(b.amount)} to ${partyName(b.creditor)} (maturity ${b.maturity_epochs} epochs)`,
            });
        case 'Sale':
            return t('txkind.sale', {
                values: { seller: partyName(b.seller), buyer: partyName(b.buyer), amount: fmt(b.amount) },
                default: `Sale of ${fmt(b.amount)} from ${partyName(b.seller)} to ${partyName(b.buyer)} (cascade discharge first)`,
            });
        case 'Settle':
            return t('txkind.settle', {
                values: { contract: b.contract, amount: fmt(b.amount) },
                default: `Settle ${fmt(b.amount)} on contract #${b.contract}`,
            });
        case 'Extend':
            return t('txkind.extend', {
                values: { contract: b.contract, date: dateOf(b.new_maturity_epoch) },
                default: `Extend contract #${b.contract} to ${dateOf(b.new_maturity_epoch)}`,
            });
        case 'Transfer':
            return t('txkind.transfer', {
                values: { contract: b.contract, to: nameOf(b.new_debtor) },
                default: `Transfer contract #${b.contract} to ${nameOf(b.new_debtor)} as new debtor`,
            });
        case 'Cure':
            return t('txkind.cure', {
                values: { contract: b.contract, amount: fmt(b.amount) },
                default: `Cure ${fmt(b.amount)} of the default on contract #${b.contract}`,
            });
        case 'MarkExpired':
            return t('txkind.markExpired', {
                values: { contract: b.contract },
                default: `Mark contract #${b.contract} as defaulted`,
            });
        case 'ArbAttest':
            return t('txkind.arbAttest', {
                values: { contract: b.contract, arbiter: nameOf(b.arbiter), amount: fmt(b.amount) },
                default: `${nameOf(b.arbiter)} attests ${fmt(b.amount)} on contract #${b.contract}`,
            });
        case 'DeclareSupply':
            // Both directions of one act, and the wording has to carry which:
            // taking on a liability and setting one down are not the same
            // sentence, and a member should never sign the first thinking it
            // is the second.
            return (b as { supply?: number }).supply === 0
                ? t('txkind.withdrawSupply', { default: 'Stop underwriting' })
                : t('txkind.declareSupply', {
                      values: { amount: (b as { supply?: number }).supply ?? 0 },
                      default: `Underwrite up to ${(b as { supply?: number }).supply ?? 0}`,
                  });
        case 'Exit':
            return t('txkind.exit', {
                values: { member: nameOf(b.member) },
                default: `${nameOf(b.member)} exits the community`,
            });
        case 'RegisterGuardians':
            return t('txkind.registerGuardians', {
                values: { member: nameOf(b.member), count: (b.guardians ?? []).length, threshold: b.threshold },
                default: `Register ${(b.guardians ?? []).length} guardians (threshold ${b.threshold}) for ${nameOf(b.member)}`,
            });
        case 'RotateRequest':
            return t('txkind.rotateRequest', {
                values: { member: nameOf(b.member) },
                default: `Rotate ${nameOf(b.member)}'s membership to a new key (guardian recovery)`,
            });
        case 'RotateVeto':
            return t('txkind.rotateVeto', {
                values: { member: nameOf(b.member) },
                default: `Veto the pending key rotation of ${nameOf(b.member)}`,
            });
        case 'RotateFinalize':
            return t('txkind.rotateFinalize', {
                values: { member: nameOf(b.member) },
                default: `Finalize the key rotation of ${nameOf(b.member)}`,
            });
        case 'ListBeneficiaries':
            return t('txkind.listBeneficiaries', {
                values: { supporter: nameOf(b.supporter), count: (b.entries ?? []).length },
                default: `${nameOf(b.supporter)} lists ${(b.entries ?? []).length} beneficiaries`,
            });
        case 'ApproveSupporter':
            return t('txkind.approveSupporter', {
                values: { supporter: nameOf(b.supporter) },
                default: `${b.approved ? 'Approve' : 'Revoke'} supporter ${nameOf(b.supporter)}`,
            });
        case 'Propose':
            return t('txkind.propose', {
                values: { author: nameOf(b.author) },
                default: `${nameOf(b.author)} opens a governance proposal`,
            });
        case 'Assent':
            return t('txkind.assent', {
                values: { proposal: b.proposal },
                default: `Assent to proposal #${b.proposal}`,
            });
        default:
            return kind;
    }
}
