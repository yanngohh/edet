<script lang="ts">
    /**
     * What an action will reserve, said BEFORE it is signed.
     *
     * The wording here is load-bearing and must never call this a fee. edet
     * charges none: the amount is encumbered against the submitter's own
     * capacity, released after `bond_release_epochs`, and credited to
     * nobody. Telling a member they are "paying" would describe a different
     * system than the one they are using — and would be the one thing the
     * ledger promises never to do.
     *
     * Three states, in the order a member meets them:
     *
     *   - the class is free forever (`amount === 0`) — settling, curing,
     *     transferring, leaving, releasing a vouch, the cranks. Worth saying
     *     out loud rather than staying silent: it is the reason a defaulted
     *     member can always settle its way out;
     *   - the free allowance covers it, which is where ordinary use lives;
     *   - the allowance is spent, so this one reserves capacity — with an
     *     affordability check against the member's own headroom, because at
     *     that point the action is about to be refused and saying so first
     *     is the difference between a wallet that explains and one that
     *     merely reports `ET-BND-001`.
     *
     * The amount comes from the node's dry-run (`quoteBond`), never from a
     * schedule duplicated client-side. The member's POSITION comes from
     * their own gated member view. The two halves are deliberately separate
     * — see `TxBondView`'s doc comment for why the node will not report the
     * second one over an unauthenticated dry-run.
     */
    import { _ } from 'svelte-i18n';

    import * as api from '../lib/api';
    import { quoteBond } from '../lib/submit';
    import { myMember, paramsView } from '../lib/node';
    import { amountsSealed, formatAmount } from '../lib/display';
    import { formatNumber } from '../common/functions';

    /**
     * The action about to be taken. Only its transition CLASS is read — the
     * schedule prices the variant, not the field values — so a page may pass
     * its plan as far as the form has been filled in, and the quote is
     * correct from the moment the page opens rather than after the last
     * field. Null renders nothing.
     */
    export let plan: api.TxPlan | null = null;

    let bond: api.TxBondView | null = null;
    let quotedFor: string | null = null;

    // Re-quote only when the transition CLASS changes — the bond depends on
    // the variant, not on the field values a member is still typing.
    $: cls = plan ? Object.keys(plan.tx)[0] ?? null : null;
    $: if (cls && cls !== quotedFor) {
        quotedFor = cls;
        bond = null;
        quoteBond(plan!).then((b) => {
            // Guard against an out-of-order reply landing after the form has
            // already moved to a different action.
            if (cls === quotedFor) bond = b;
        });
    }

    $: pos = $myMember?.operation_bond;
    $: allowance = $paramsView?.bond_free_allowance;
    $: freeLeft = pos?.free_remaining;
    // Absent position (no session token yet, or a viewer without full
    // access) reads as unknown, never as zero: claiming someone has no
    // allowance left when we simply cannot see it would scare them off an
    // action that is in fact free.
    $: coveredByAllowance = freeLeft !== undefined && freeLeft > 0;
    $: shortfall = bond !== null && pos !== undefined && !coveredByAllowance && bond.amount > pos.headroom;
</script>

{#if bond}
    <div class="bond-notice" class:short={shortfall} role="note">
        <i class="material-icons bond-icon" aria-hidden="true">
            {shortfall ? 'error_outline' : bond.amount === 0 ? 'check_circle_outline' : 'lock_clock'}
        </i>
        <span class="bond-text">
            {#if bond.amount === 0}
                {$_('bond.alwaysFree', {
                    default: 'Always free. This action never sets aside any of your capacity — settling, curing and leaving never can.',
                })}
            {:else if coveredByAllowance}
                {$_('bond.covered', {
                    values: { left: freeLeft, total: allowance ?? '' },
                    default: `Free — covered by your activity allowance (${freeLeft} of ${allowance ?? '?'} left this epoch).`,
                })}
            {:else if shortfall && pos}
                {$_('bond.short', {
                    values: {
                        amount: formatAmount(formatNumber, bond.amount, $amountsSealed),
                        headroom: formatAmount(formatNumber, pos.headroom, $amountsSealed),
                    },
                    default: `This would set aside ${formatAmount(formatNumber, bond.amount, $amountsSealed)}, but only ${formatAmount(formatNumber, pos.headroom, $amountsSealed)} of your capacity is free to set aside. Settling what you owe frees it sooner.`,
                })}
            {:else}
                {$_('bond.reserves', {
                    values: {
                        amount: formatAmount(formatNumber, bond.amount, $amountsSealed),
                        epochs: bond.release_epochs,
                    },
                    default: `Sets aside ${formatAmount(formatNumber, bond.amount, $amountsSealed)} of your capacity for ${bond.release_epochs} epoch(s), then it comes back to you. Nothing is charged and nobody receives it.`,
                })}
            {/if}
        </span>
    </div>
{/if}

<style>
    .bond-notice {
        display: flex;
        align-items: flex-start;
        gap: 8px;
        padding: 8px 12px;
        border-radius: 0 4px 4px 0;
        background: rgba(21, 101, 192, 0.06);
        border-left: 3px solid rgba(21, 101, 192, 0.45);
        font-size: 0.82rem;
        line-height: 1.45;
        color: var(--mdc-theme-text-secondary-on-surface, #555);
    }
    .bond-notice.short {
        background: rgba(211, 47, 47, 0.06);
        border-left-color: rgba(211, 47, 47, 0.5);
        color: #b71c1c;
    }
    .bond-icon {
        font-size: 18px;
        flex-shrink: 0;
        color: #1565c0;
    }
    .bond-notice.short .bond-icon {
        color: #d32f2f;
    }
    :global(.dark-theme) .bond-notice {
        background: rgba(21, 101, 192, 0.14);
        border-left-color: #64b5f6;
        color: #cfd8dc;
    }
    :global(.dark-theme) .bond-icon {
        color: #64b5f6;
    }
    :global(.dark-theme) .bond-notice.short {
        background: rgba(211, 47, 47, 0.14);
        border-left-color: #e57373;
        color: #ef9a9a;
    }
    :global(.dark-theme) .bond-notice.short .bond-icon {
        color: #e57373;
    }
    .bond-text {
        overflow-wrap: anywhere;
    }
</style>
