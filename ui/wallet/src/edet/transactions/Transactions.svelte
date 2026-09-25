<script lang="ts">
    /**
     * Transactions: the history you can read at a glance. Recording a
     * recording a trade is an action, so it lives behind the FAB (→ RecordTrade),
     * not inline above the list.
     */
    import { _ } from 'svelte-i18n';
    import Fab from '@smui/fab';
    import { Icon } from '@smui/common';

    import ContractCard from '../contracts/ContractCard.svelte';
    import OpenOffers from '../../components/OpenOffers.svelte';
    import RecordTrade from './RecordTrade.svelte';
    import { contractsList, listTruncation } from '../../lib/node';
    import { currentActorId, pendingIdentity } from '../../lib/actors';
    import { resumeActorChoice } from '../../common/onboardingStore';

    let creating = false;

    $: me = $currentActorId;
    // A key with no account yet records a purchase like anybody else; what
    // differs is how it reaches the seller (`lib/offer.ts`).
    $: keyed = me === null && $pendingIdentity !== null;
    $: history = me === null
        ? []
        : $contractsList
              .filter((c) => c.debtor === me || c.creditor === me)
              .sort((a, b) => b.id - a.id);
</script>

{#if creating}
    <RecordTrade on:close={() => (creating = false)} />
{:else}
    <div class="main-container flex-column">
        <OpenOffers />
        <h3 class="section-title">{$_('transactions.history', { default: 'Your transaction history' })}</h3>
        {#if $listTruncation['/contracts']}
            <p class="truncated">
                {$_('transactions.truncated', {
                    values: {
                        shown: $listTruncation['/contracts']?.shown,
                        total: $listTruncation['/contracts']?.total,
                    },
                    default: `This node sent ${$listTruncation['/contracts']?.shown} of ${$listTruncation['/contracts']?.total} trades on the ledger, so your history here may be incomplete.`,
                })}
            </p>
        {/if}
        {#if history.length === 0}
            <p class="empty">{$_('transactions.empty', { default: 'Nothing yet — tap + to record your first purchase.' })}</p>
        {:else}
            <div class="history-list">
                {#each history as c (c.id)}
                    <ContractCard contract={c} />
                {/each}
            </div>
        {/if}
    </div>

    <!--
        Shown with or without an identity. **This is the line where a visitor
        stops visiting**, so it is the right place to ask for one — hiding it
        instead left the app looking like a thing they were not allowed to use,
        when the truth is only that a trade needs a key to sign it.
    -->
    <Fab
        color="primary"
        class="fab-add"
        aria-label={$_('transactions.purchase', { default: 'Record a purchase' })}
        title={$_('transactions.purchase', { default: 'Record a purchase' })}
        on:click={() => (me === null && !keyed ? resumeActorChoice() : (creating = true))}
    >
        <Icon class="material-icons">add</Icon>
    </Fab>
{/if}

<style>
    .main-container {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
        gap: 16px;
        max-width: var(--edet-column);
    }
    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin: 8px 0 0;
        font-weight: 500;
    }
    .history-list {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .empty {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .truncated {
        margin: 0;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        line-height: 1.5;
        font-style: italic;
    }
</style>
