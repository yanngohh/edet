<script lang="ts">
    import { _ } from 'svelte-i18n';

    import type { ContractView } from '../../lib/api';
    import type { PendingExtension } from '../../lib/extension';
    import { networkView } from '../../lib/node';
    import { amountsSealed } from '../../lib/display';
    import { epochDate } from '../../lib/epoch';
    import { formatNumber } from '../../common/functions';
    import MemberChip from '../../components/MemberChip.svelte';

    export let contract: ContractView;
    /**
     * A maturity the member is composing in a form under this card. The due
     * label follows it as they type, because the record cannot: an extension
     * is signed by both parties and the ledger moves nothing until it is.
     */
    export let maturityPreview: number | null = null;
    /** An extension of this contract waiting in the pending pool. */
    export let pendingExtension: PendingExtension | null = null;

    $: epoch = $networkView?.epoch ?? 0;
    $: overdue = contract.status === 'active' && epoch > contract.maturity_epoch;
    $: epochsLeft = contract.maturity_epoch - epoch;
    // The term the label shows beside the record: the form's while one is
    // open, else the parked request's; nothing when it equals the record.
    $: proposed = maturityPreview ?? pendingExtension?.newMaturity ?? null;
    $: previewed = proposed !== null && proposed !== contract.maturity_epoch ? proposed : null;
</script>

<div class="contract-card" class:overdue class:expired={contract.status === 'expired'}>
    <div class="contract-head">
        <span class="contract-id">#{contract.id}</span>
        <span class="contract-parties">
            {#if contract.debtor !== undefined}
                <MemberChip memberId={contract.debtor} size={20} />
            {:else}
                <span class="member-chip-hidden">{$_('contracts.hiddenParty', { default: '(hidden)' })}</span>
            {/if}
            <i class="material-icons arrow" aria-hidden="true">arrow_forward</i>
            {#if contract.creditor !== undefined}
                <MemberChip memberId={contract.creditor} size={20} />
            {:else}
                <span class="member-chip-hidden">{$_('contracts.hiddenParty', { default: '(hidden)' })}</span>
            {/if}
        </span>
        <span class="pills">
            <span class="pill status-{contract.status}">
                {$_('contracts.status.' + contract.status, { default: contract.status })}
            </span>
            <!--
                Insured or not is shown on EVERY claim, not only the unusual
                one. Marking only the exception teaches a member that the
                unmarked case is "normal" rather than "backed", and the whole
                difference between the two tiers is who bears the loss.
            -->
            {#if contract.insured}
                <span
                    class="pill insured"
                    title={$_('contracts.insuredTitle', {
                        default:
                            'The community stands behind this: it reserved capacity when it was accepted, and if it defaults the underwriters whose backing carried it become your debtors instead.',
                    })}
                >
                    {$_('contracts.insured', { default: 'insured' })}
                </span>
            {:else}
                <span
                    class="pill uninsured"
                    title={$_('contracts.uninsuredTitle', {
                        default:
                            'Nobody but the creditor is behind this one. It reserved no capacity and triggers no community recourse if it defaults — which is how trade with someone new normally begins.',
                    })}
                >
                    {$_('contracts.uninsured', { default: 'uninsured' })}
                </span>
            {/if}
            {#if contract.arb}
                <span class="pill arb" title={$_('contracts.arbTitle', { default: 'Arbitration panel agreed at acceptance' })}>
                    {$_('contracts.arb', { default: 'arbitration' })}
                </span>
            {/if}
        </span>
    </div>

    <div class="contract-body">
        <span
            class="amount"
            title={$amountsSealed
                ? $_('contracts.sealedTitle', {
                      default: 'Rounded up to conceal the exact figure — not the precise amount.',
                  })
                : undefined}
        >
            {#if $amountsSealed}<span class="sealed-mark" aria-hidden="true">≈</span>{/if}
            {formatNumber(contract.outstanding)}
            {#if contract.outstanding !== contract.original}
                <span class="of-original">/ {formatNumber(contract.original)}</span>
            {/if}
        </span>
        <span class="maturity" class:overdue-text={overdue && previewed === null} class:preview={previewed !== null}>
            {#if contract.status === 'active'}
                {#if previewed !== null}
                    {$_('contracts.duePreview', {
                        values: { date: $epochDate(contract.maturity_epoch), next: $epochDate(previewed), left: previewed - epoch },
                        default: `due ${$epochDate(contract.maturity_epoch)} → ${$epochDate(previewed)} (${previewed - epoch} days left)`,
                    })}
                {:else if overdue}
                    {$_('contracts.overdue', {
                        values: { epochs: epoch - contract.maturity_epoch },
                        default: `overdue by ${epoch - contract.maturity_epoch} epochs`,
                    })}
                {:else}
                    {$_('contracts.due', {
                        values: { date: $epochDate(contract.maturity_epoch), left: epochsLeft },
                        default: `due ${$epochDate(contract.maturity_epoch)} (${epochsLeft} days left)`,
                    })}
                {/if}
            {:else}
                {$_('contracts.createdAt', {
                    values: { date: $epochDate(contract.created_epoch) },
                    default: `created ${$epochDate(contract.created_epoch)}`,
                })}
            {/if}
        </span>
    </div>

    {#if pendingExtension}
        <p class="pending-note">
            <i class="material-icons" aria-hidden="true">hourglass_empty</i>
            <span>
                {#if pendingExtension.awaitingMe}
                    {$_('contracts.extendAsked', {
                        values: { date: $epochDate(pendingExtension.newMaturity) },
                        default: `The other party asks to extend this to ${$epochDate(pendingExtension.newMaturity)} — sign or decline it under Requests.`,
                    })}
                {:else}
                    {$_('contracts.extendWaiting', {
                        values: { date: $epochDate(pendingExtension.newMaturity) },
                        default: `An extension to ${$epochDate(pendingExtension.newMaturity)} is waiting for the other party's signature — track or withdraw it under Requests.`,
                    })}
                {/if}
            </span>
        </p>
    {/if}

    <slot />
</div>

<style>
    .member-chip-hidden {
        font-size: 0.85rem;
        font-style: italic;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .contract-card {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-left: 4px solid var(--mdc-theme-primary);
        border-radius: 8px;
        padding: 12px 16px;
        background: var(--mdc-theme-surface, #fff);
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    /* The three sides the theme owns; the left one belongs to the status
       modifiers below. This one happened to render correctly — `.overdue` ties
       on specificity and wins on source order — which is exactly why it is
       worth making structural rather than leaving it to the order of the file. */
    :global(.dark-theme) .contract-card {
        background: #1e1e1e;
        border-top-color: rgba(255, 255, 255, 0.1);
        border-right-color: rgba(255, 255, 255, 0.1);
        border-bottom-color: rgba(255, 255, 255, 0.1);
    }
    .contract-card.overdue {
        border-left-color: #f9a825;
    }
    .contract-card.expired {
        border-left-color: #d32f2f;
    }
    .contract-head {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
    }
    .contract-id {
        font-family: monospace;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .contract-parties {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .arrow {
        font-size: 16px;
        color: var(--mdc-theme-text-secondary-on-surface, #999);
    }
    .pills {
        margin-left: auto;
        display: inline-flex;
        gap: 6px;
        flex-wrap: wrap;
    }
    .pill {
        font-size: 0.72rem;
        padding: 2px 8px;
        border-radius: 999px;
        border: 1px solid currentColor;
        text-transform: lowercase;
    }
    .insured { background: #e8f5e9; color: #2e7d32; }
    .uninsured { background: #fff3e0; color: #e65100; }
    :global(.dark-theme) .insured { background: #1b3d1e; color: #a5d6a7; }
    :global(.dark-theme) .uninsured { background: #3d2a12; color: #ffcc80; }
    .status-active { color: #2e7d32; }
    .status-expired { color: #d32f2f; }
    .status-settled, .status-cured { color: #616161; }
    .status-transferred { color: #1565c0; }
    .pill.arb { color: #ef6c00; }
    :global(.dark-theme) .status-active { color: #81c784; }
    :global(.dark-theme) .status-expired { color: #e57373; }
    :global(.dark-theme) .status-settled, :global(.dark-theme) .status-cured { color: #bdbdbd; }
    :global(.dark-theme) .status-transferred { color: #64b5f6; }
    :global(.dark-theme) .pill.arb { color: #ffb74d; }
    .contract-body {
        display: flex;
        align-items: baseline;
        gap: 14px;
        flex-wrap: wrap;
    }
    .amount {
        font-size: 1.25rem;
        font-weight: 600;
    }
    .of-original {
        font-size: 0.85rem;
        font-weight: 400;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .sealed-mark {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        margin-right: 2px;
    }
    .maturity {
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .overdue-text {
        color: #f9a825;
        font-weight: 600;
    }
    .maturity.preview {
        color: var(--mdc-theme-primary);
        font-weight: 600;
    }
    .pending-note {
        margin: 0;
        display: flex;
        gap: 0.5em;
        align-items: flex-start;
        font-size: 0.82rem;
        line-height: 1.45;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .pending-note .material-icons {
        font-size: 1.15em;
        flex: none;
        margin-top: 0.1em;
        color: var(--mdc-theme-primary);
    }
</style>
