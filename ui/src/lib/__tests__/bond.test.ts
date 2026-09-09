/**
 * The bond quote the wallet shows before an action is signed.
 *
 * Two things are worth pinning. First, the amount must come from the node —
 * a schedule duplicated in TypeScript would drift from
 * `edet_state::bond::bond_multiple` and leave the wallet quoting a figure
 * the gate does not use. Second, the cache must be keyed on exactly what the
 * schedule keys on (the transition class) and on the unit it multiplies, or
 * it either re-quotes on every keystroke or serves a stale amount after a
 * redenomination.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../crypto', async () => {
    const actual = await vi.importActual<typeof import('../crypto')>('../crypto');
    return { ...actual, randomNonce: () => Array(16).fill(0) };
});

import * as api from '../api';
import { quoteBond } from '../submit';
import { networkView, paramsView } from '../node';
import type { ParamsView } from '../api';

const params = (over: Partial<ParamsView> = {}): ParamsView =>
    ({
        governed: [],
        v_base: 100,
        cap_max: 1000,
        v_epoch: 10,
        dust: 0.01,
        trial_cap: 5,
        epoch_secs: 60,
        min_maturity_epochs: 1,
        gov_cooldown_epochs: 1,
        last_redenom_epoch: null,
        bond_unit: 2,
        bond_free_allowance: 32,
        bond_release_epochs: 1,
        bond_forfeit_epochs: 3,
        ...over,
    }) as ParamsView;

// The quote cache is module-level and deliberately outlives a single action
// page, so it also outlives a single test here. Each case below therefore
// uses a transition class no other case touches — anything else would have
// one test served by another's cached quote.
describe('quoteBond', () => {
    beforeEach(() => {
        vi.restoreAllMocks();
        // `defaultNotAfterEpoch` refuses to invent an epoch client-side, so
        // the polled network view has to exist before any write path runs.
        networkView.set({ epoch: 5 } as never);
        paramsView.set(params());
    });

    it('quotes the amount the node reports, never a locally computed one', async () => {
        const spy = vi
            .spyOn(api, 'checkTx')
            .mockResolvedValue({ ok: true, bond: { amount: 8, release_epochs: 1 } });

        const q = await quoteBond(
            api.tx.registerGuardians({ member: 3, guardians: [4, 5], threshold: 2, vetoWindowEpochs: 30 }),
        );
        expect(q).toEqual({ amount: 8, release_epochs: 1 });
        expect(spy).toHaveBeenCalledTimes(1);
    });

    it('caches by transition class, so filling in a form does not re-quote', async () => {
        const spy = vi
            .spyOn(api, 'checkTx')
            .mockResolvedValue({ ok: true, bond: { amount: 2, release_epochs: 1 } });

        // Same class, different field values — exactly what a form produces
        // as the member types. `bond_multiple` switches on the variant only,
        // so one request must serve them all.
        await quoteBond(api.tx.declareSupply(1, 0));
        await quoteBond(api.tx.declareSupply(1, 50));
        await quoteBond(api.tx.declareSupply(3, 900));
        expect(spy).toHaveBeenCalledTimes(1);

        // A different class is a different price: it must ask again.
        await quoteBond(api.tx.listBeneficiaries(2, [[3, 1]]));
        expect(spy).toHaveBeenCalledTimes(2);
    });

    it('re-quotes when the unit moves, rather than showing a stale amount', async () => {
        const spy = vi
            .spyOn(api, 'checkTx')
            .mockResolvedValue({ ok: true, bond: { amount: 2, release_epochs: 1 } });

        await quoteBond(api.tx.exit(1));
        expect(spy).toHaveBeenCalledTimes(1);

        // A redenomination (or a governed change to BondFraction) moves
        // `bond_unit`, and the quoted amount is a multiple of it. The cache
        // key folds the unit in precisely so this does not go unnoticed.
        spy.mockResolvedValue({ ok: true, bond: { amount: 6, release_epochs: 1 } });
        paramsView.set(params({ bond_unit: 6 }));
        expect(await quoteBond(api.tx.exit(1))).toEqual({ amount: 6, release_epochs: 1 });
        expect(spy).toHaveBeenCalledTimes(2);
    });

    it('shows nothing rather than guessing when the node reports no bond', async () => {
        // A node predating the disclosure, and an unreachable one. Inventing
        // a number for either would be worse than staying quiet: the wallet
        // would be asserting a cost the ledger never quoted.
        vi.spyOn(api, 'checkTx').mockResolvedValue({ ok: true });
        expect(await quoteBond(api.tx.settle({ id: 1, debtor: 1, creditor: 2 } as never, 5))).toBeNull();

        vi.spyOn(api, 'checkTx').mockRejectedValue(new Error('offline'));
        expect(await quoteBond(api.tx.markExpired({ id: 2, debtor: 1, creditor: 2 } as never))).toBeNull();
    });
});
