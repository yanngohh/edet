<script lang="ts">
    import { _ } from 'svelte-i18n';
    import { getContext, onMount, onDestroy } from 'svelte';
    import type { AppClient, HolochainError, Record } from '@holochain/client';
    import { encodeHashToBase64 } from '@holochain/client';
    import { decode } from '@msgpack/msgpack';
    import { clientContext } from '../../contexts';
    import { errorStore } from '../../common/errorStore';
    import { extractHolochainErrorCode, formatNumber } from '../../common/functions';
    import { EPOCH_DURATION_MS } from '../../common/constants';
    import type { DebtContract } from './types';
    import AgentAvatar from './AgentAvatar.svelte';
    import CircularProgress from '@smui/circular-progress';
    import { Icon } from '@smui/common';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    // ── State ────────────────────────────────────────────────────────────────

    let loading = true;
    let contracts: DebtContract[] = [];

    // Archived contracts (lazy-loaded on demand)
    let archivedContracts: DebtContract[] = [];
    let loadingArchived = false;
    let archivedLoaded = false;
    let showSettled = true;
    let showExpired = true;
    let showArchived = false;

    // Current epoch (floor(unix_ms / EPOCH_DURATION_MS)).
    // Refreshed every 60 seconds so urgency indicators stay accurate if the tab is
    // left open across epoch boundaries (each epoch = 1 day in production).
    let currentEpoch = Math.floor(Date.now() / EPOCH_DURATION_MS);
    let epochRefreshInterval: ReturnType<typeof setInterval>;
    onMount(() => {
        epochRefreshInterval = setInterval(() => {
            currentEpoch = Math.floor(Date.now() / EPOCH_DURATION_MS);
        }, 60_000);
    });
    onDestroy(() => {
        if (epochRefreshInterval) clearInterval(epochRefreshInterval);
    });

    // ── Derived lists ────────────────────────────────────────────────────────

    $: activeContracts = contracts
        .filter(c => c.status?.type === 'Active')
        .sort((a, b) => (a.start_epoch + a.maturity) - (b.start_epoch + b.maturity));

    $: settledContracts = contracts
        .filter(c => c.status?.type === 'Transferred')
        .sort((a, b) => b.start_epoch - a.start_epoch);

    $: expiredContracts = contracts
        .filter(c => c.status?.type === 'Expired')
        .sort((a, b) => b.start_epoch - a.start_epoch);

    // ── Helpers ──────────────────────────────────────────────────────────────

    function deadlineEpoch(c: DebtContract): number {
        return c.start_epoch + c.maturity;
    }

    function epochsRemaining(c: DebtContract): number {
        return deadlineEpoch(c) - currentEpoch;
    }

    function urgencyClass(c: DebtContract): string {
        const remaining = epochsRemaining(c);
        if (remaining <= 7) return 'urgent';
        if (remaining <= 14) return 'warning';
        return 'ok';
    }

    function decodeContract(record: Record): DebtContract | null {
        try {
            const raw = (record.entry as any)?.Present?.entry;
            return raw ? decode(raw) as DebtContract : null;
        } catch {
            return null;
        }
    }

    // ── Data fetching ────────────────────────────────────────────────────────

    onMount(async () => {
        await loadContracts();
    });

    async function loadContracts() {
        loading = true;
        try {
            const records: Record[] = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_all_contracts_as_debtor_resolved',
                payload: client.myPubKey,
            });
            contracts = records
                .map(decodeContract)
                .filter((c): c is DebtContract => c !== null);
        } catch (e: any) {
            const he = e as HolochainError;
            const message = $_('errors.' + extractHolochainErrorCode(he.message), { default: he.message });
            errorStore.pushError($_('myContracts.errorFetch', { default: `Failed to load contracts: ${message}` }));
        }
        loading = false;
    }

    async function loadArchivedContracts() {
        if (loadingArchived) return;
        loadingArchived = true;
        try {
            const records: Record[] = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_archived_contracts',
                payload: client.myPubKey,
            });
            archivedContracts = records
                .map(decodeContract)
                .filter((c): c is DebtContract => c !== null);
            archivedLoaded = true;
        } catch (e: any) {
            const he = e as HolochainError;
            const message = $_('errors.' + extractHolochainErrorCode(he.message), { default: he.message });
            errorStore.pushError($_('myContracts.errorFetchArchived', { default: `Failed to load archived contracts: ${message}` }));
        }
        loadingArchived = false;
    }

    function toggleArchived() {
        showArchived = !showArchived;
        if (showArchived && !archivedLoaded) {
            loadArchivedContracts();
        }
    }
</script>

{#if loading}
    <div class="center">
        <CircularProgress indeterminate style="width:48px;height:48px;" />
    </div>
{:else}
    <div class="contracts-page">
        <div class="page-header flex-row justify-end">
            <div class="epoch-indicator">
                <i class="material-icons" aria-hidden="true">schedule</i>
                <span>{$_('myContracts.currentEpoch', { default: 'Current Epoch' })}: {currentEpoch}</span>
            </div>
        </div>

        <!-- ── Active Contracts ────────────────────────────────────────── -->
        <div class="section-container">
            <h4 class="section-title">
                <i class="material-icons section-icon active-icon" aria-hidden="true">schedule</i>
                {$_('myContracts.activeTitle', { default: 'Active Debt Obligations' })}
                {#if activeContracts.length > 0}
                    <span class="badge badge-active">{activeContracts.length}</span>
                {/if}
            </h4>
            <p class="helper-text">
                {$_('myContracts.activeHelperText', { default: 'Debt you currently owe. Each contract must be transferred before its deadline by selling goods or services to a new buyer.' })}
            </p>

            {#if activeContracts.length === 0}
                <div class="empty-msg">
                    <span>{$_('myContracts.noActive', { default: 'No active debt obligations. All clear!' })}</span>
                </div>
            {:else}
                <div class="card-list">
                    {#each activeContracts as contract (encodeHashToBase64(contract.transaction_hash))}
                        <div class="contract-card {urgencyClass(contract)}">
                            <div class="card-left">
                                <AgentAvatar agentPubKey={contract.creditor} size={36} />
                                <div class="card-text">
                                    <span class="card-label">{$_('myContracts.creditor', { default: 'Creditor' })}</span>
                                    <span class="card-addr">{contract.creditor}</span>
                                </div>
                            </div>
                            <div class="card-center">
                                <span class="amount-remaining">{formatNumber(contract.amount, 2)}</span>
                                {#if contract.original_amount > contract.amount}
                                    <span class="amount-original">
                                        {$_('myContracts.of', { default: 'of' })} {formatNumber(contract.original_amount, 2)}
                                    </span>
                                {/if}
                                {#if contract.is_trial}
                                    <span class="badge badge-trial">{$_('myContracts.trial', { default: 'Trial' })}</span>
                                {/if}
                            </div>
                            {#if contract.co_signers && contract.co_signers.length > 0}
                                <div class="cosigners-row">
                                    <span class="cosigners-label">{$_('myContracts.coSigners', { default: 'Co-signers' })}:</span>
                                    {#each contract.co_signers as [cosignerKey, weight]}
                                        <span class="cosigner-entry" title="{cosignerKey} ({(weight * 100).toFixed(0)}%)">
                                            <AgentAvatar agentPubKey={cosignerKey} size={20} />
                                            <span class="cosigner-weight">{(weight * 100).toFixed(0)}%</span>
                                        </span>
                                    {/each}
                                </div>
                            {/if}
                            <div class="card-right">
                                <span class="deadline-epoch">
                                    {$_('myContracts.deadline', { default: 'Deadline' })}: {$_('myContracts.epoch', { default: 'epoch' })} {deadlineEpoch(contract)}
                                </span>
                                {#if epochsRemaining(contract) > 0}
                                    <span class="deadline-remaining {urgencyClass(contract)}">
                                        {epochsRemaining(contract)} {$_('myContracts.epochsLeft', { default: 'epochs left' })}
                                    </span>
                                {:else if epochsRemaining(contract) === 0}
                                    <span class="deadline-remaining urgent">
                                        {$_('myContracts.dueToday', { default: 'Due this epoch!' })}
                                    </span>
                                {:else}
                                    <span class="deadline-remaining urgent">
                                        {$_('myContracts.overdue', { default: 'Overdue' })} ({Math.abs(epochsRemaining(contract))} {$_('myContracts.epochsAgo', { default: 'epochs ago' })})
                                    </span>
                                {/if}
                            </div>
                        </div>
                    {/each}
                </div>
            {/if}
        </div>

        <!-- ── Settled Contracts ──────────────────────────────────────── -->
        <div class="section-container" style="margin-top: 32px;">
            <div class="section-header-row"
                 on:click={() => showSettled = !showSettled}
                 on:keydown={e => e.key === 'Enter' && (showSettled = !showSettled)}
                 role="button" tabindex="0">
                <h4 class="section-title" style="margin:0;flex:1;">
                    <i class="material-icons section-icon settled-icon">check_circle</i>
                    {$_('myContracts.settledTitle', { default: 'Settled Contracts' })}
                    {#if settledContracts.length > 0}
                        <span class="badge badge-settled">{settledContracts.length}</span>
                    {/if}
                </h4>
                <Icon class="material-icons">{showSettled ? 'expand_less' : 'expand_more'}</Icon>
            </div>

            {#if showSettled}
                <p class="helper-text">
                    {$_('myContracts.settledHelperText', { default: 'Debt you successfully transferred by selling goods or services. These contribute positively to your trust score.' })}
                </p>
                {#if settledContracts.length === 0}
                    <div class="empty-msg">
                        {$_('myContracts.noSettled', { default: 'No settled contracts yet.' })}
                    </div>
                {:else}
                    <div class="card-list">
                        {#each settledContracts as contract (encodeHashToBase64(contract.transaction_hash))}
                            <div class="contract-card settled">
                                <div class="card-left">
                                    <AgentAvatar agentPubKey={contract.creditor} size={32} />
                                    <div class="card-text">
                                        <span class="card-label">{$_('myContracts.creditor', { default: 'Creditor' })}</span>
                                        <span class="card-addr">{contract.creditor}</span>
                                    </div>
                                </div>
                                <div class="card-center">
                                    <span class="amount-settled">{formatNumber(contract.original_amount, 2)}</span>
                                    {#if contract.is_trial}
                                        <span class="badge badge-trial">{$_('myContracts.trial', { default: 'Trial' })}</span>
                                    {/if}
                                </div>
                                <div class="card-right">
                                    <span class="badge badge-settled-status">
                                        <i class="material-icons" style="font-size:12px;vertical-align:middle;">check_circle</i>
                                        {$_('myContracts.transferred', { default: 'Transferred' })}
                                    </span>
                                    <span class="card-epoch">
                                        {$_('myContracts.startEpoch', { default: 'Started epoch' })} {contract.start_epoch}
                                    </span>
                                </div>
                            </div>
                        {/each}
                    </div>
                {/if}
            {/if}
        </div>

        <!-- ── Expired Contracts ──────────────────────────────────────── -->
        <div class="section-container" style="margin-top: 32px;">
            <div class="section-header-row"
                 on:click={() => showExpired = !showExpired}
                 on:keydown={e => e.key === 'Enter' && (showExpired = !showExpired)}
                 role="button" tabindex="0">
                <h4 class="section-title" style="margin:0;flex:1;">
                    <i class="material-icons section-icon expired-icon">error</i>
                    {$_('myContracts.expiredTitle', { default: 'Expired Contracts' })}
                    {#if expiredContracts.length > 0}
                        <span class="badge badge-expired">{expiredContracts.length}</span>
                    {/if}
                </h4>
                <Icon class="material-icons">{showExpired ? 'expand_less' : 'expand_more'}</Icon>
            </div>

            {#if showExpired}
                <p class="helper-text">
                    {$_('myContracts.expiredHelperText', { default: 'Debt that reached maturity without being transferred. Each default reduces your trust score and triggers slashing for your sponsors.' })}
                </p>
                {#if expiredContracts.length === 0}
                    <div class="empty-msg">
                        <span>{$_('myContracts.noExpired', { default: 'No defaults — great track record!' })}</span>
                    </div>
                {:else}
                    <div class="card-list">
                        {#each expiredContracts as contract (encodeHashToBase64(contract.transaction_hash))}
                            <div class="contract-card expired">
                                <div class="card-left">
                                    <AgentAvatar agentPubKey={contract.creditor} size={32} />
                                    <div class="card-text">
                                        <span class="card-label">{$_('myContracts.creditor', { default: 'Creditor' })}</span>
                                        <span class="card-addr">{contract.creditor}</span>
                                    </div>
                                </div>
                                <div class="card-center">
                                    <span class="amount-expired">{formatNumber(contract.original_amount, 2)}</span>
                                    {#if contract.is_trial}
                                        <span class="badge badge-trial">{$_('myContracts.trial', { default: 'Trial' })}</span>
                                    {/if}
                                </div>
                                <div class="card-right">
                                    <span class="badge badge-expired-status">
                                        <i class="material-icons" style="font-size:12px;vertical-align:middle;">error</i>
                                        {$_('myContracts.defaulted', { default: 'Defaulted' })}
                                    </span>
                                    <span class="card-epoch">
                                        {$_('myContracts.startEpoch', { default: 'Started epoch' })} {contract.start_epoch}
                                    </span>
                                </div>
                            </div>
                        {/each}
                    </div>
                {/if}
            {/if}
        </div>

        <!-- ── Archived Contracts ─────────────────────────────────────── -->
        <div class="section-container" style="margin-top: 32px;">
            <div class="section-header-row"
                 on:click={toggleArchived}
                 on:keydown={e => e.key === 'Enter' && toggleArchived()}
                 role="button" tabindex="0">
                <h4 class="section-title" style="margin:0;flex:1;">
                    <i class="material-icons section-icon">archive</i>
                    {$_('myContracts.archivedTitle', { default: 'Archived Contracts' })}
                    {#if archivedLoaded && archivedContracts.length > 0}
                        <span class="badge badge-neutral">{archivedContracts.length}</span>
                    {/if}
                </h4>
                <Icon class="material-icons">{showArchived ? 'expand_less' : 'expand_more'}</Icon>
            </div>

            {#if showArchived}
                <p class="helper-text">
                    {$_('myContracts.archivedHelperText', { default: 'Settled or expired contracts older than 30 epochs, moved to cold storage to keep active queries fast.' })}
                </p>
                {#if loadingArchived}
                    <div class="center"><CircularProgress indeterminate style="width:32px;height:32px;" /></div>
                {:else if archivedContracts.length === 0}
                    <div class="empty-msg">
                        {$_('myContracts.noArchived', { default: 'No archived contracts.' })}
                    </div>
                {:else}
                    <div class="card-list">
                        {#each archivedContracts as contract (encodeHashToBase64(contract.transaction_hash))}
                            <div class="contract-card archived">
                                <div class="card-left">
                                    <AgentAvatar agentPubKey={contract.creditor} size={28} />
                                    <div class="card-text">
                                        <span class="card-addr">{contract.creditor}</span>
                                    </div>
                                </div>
                                <div class="card-center">
                                    <span class="amount-archived">{formatNumber(contract.original_amount, 2)}</span>
                                    {#if contract.is_trial}
                                        <span class="badge badge-trial">{$_('myContracts.trial', { default: 'Trial' })}</span>
                                    {/if}
                                </div>
                                <div class="card-right">
                                    <span class="badge badge-neutral">{$_('myContracts.status' + (contract.status?.type ?? ''), { default: contract.status?.type ?? '?' })}</span>
                                    <span class="card-epoch">
                                        {$_('myContracts.startEpoch', { default: 'Started epoch' })} {contract.start_epoch}
                                    </span>
                                </div>
                            </div>
                        {/each}
                    </div>
                {/if}
            {/if}
        </div>

    </div>
{/if}

<style>
    .contracts-page {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
    }

    .page-header {
        margin-bottom: 24px;
        padding-bottom: 12px;
        border-bottom: 1px solid var(--mdc-theme-text-hint-on-background, #eee);
    }

    :global(.dark-theme) .page-header {
        border-bottom-color: rgba(255, 255, 255, 0.1);
    }

    .epoch-indicator {
        display: flex;
        align-items: center;
        gap: 6px;
        background: var(--mdc-theme-background, #f5f5f5);
        padding: 6px 12px;
        border-radius: 20px;
        font-size: 0.85rem;
        font-weight: 600;
        color: var(--mdc-theme-text-primary-on-background);
        border: 1px solid var(--mdc-theme-text-hint-on-background, #ddd);
    }

    :global(.dark-theme) .epoch-indicator {
        background: rgba(255, 255, 255, 0.05);
        border-color: rgba(255, 255, 255, 0.1);
    }

    .epoch-indicator i {
        font-size: 16px;
        color: var(--mdc-theme-primary);
    }

    .center {
        display: flex;
        justify-content: center;
        align-items: center;
        padding: 48px 0;
    }

    /* ── Section structure ─────────────────────────────────────────── */
    .section-container {
        width: 100%;
    }

    .section-header-row {
        display: flex;
        flex-direction: row;
        align-items: center;
        cursor: pointer;
        user-select: none;
        gap: 8px;
    }

    .section-title {
        display: flex;
        align-items: center;
        gap: 8px;
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin-bottom: 8px;
        font-weight: 500;
        font-size: 1rem;
        width: 100%;
    }

    .section-icon {
        font-size: 18px;
    }

    .active-icon   { color: var(--mdc-theme-error, #d32f2f); }
    .settled-icon  { color: #2e7d32; }
    .expired-icon  { color: #e65100; }

    .helper-text {
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        margin-bottom: 12px;
        line-height: 1.4;
    }

    .empty-msg {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 8px;
        padding: 24px;
        text-align: center;
        color: var(--mdc-theme-text-secondary-on-background, #888);
        background: var(--mdc-theme-background, #f5f5f5);
        border-radius: 8px;
        font-style: italic;
    }

    :global(.dark-theme) .empty-msg {
        background: rgba(255, 255, 255, 0.04);
        color: #aaa;
    }

    /* ── Contract cards ────────────────────────────────────────────── */
    .card-list {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .contract-card {
        display: flex;
        flex-direction: row;
        align-items: center;
        gap: 12px;
        padding: 12px 16px;
        border-radius: 8px;
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0,0,0,0.1));
        background: var(--mdc-theme-surface, #fff);
        transition: box-shadow 0.15s;
    }

    .contract-card:hover {
        box-shadow: 0 2px 8px rgba(0,0,0,0.08);
    }

    :global(.dark-theme) .contract-card {
        background: #1e1e1e;
        border-color: rgba(255,255,255,0.08);
    }

    /* Urgency tints for active cards */
    .contract-card.urgent {
        border-left: 4px solid var(--mdc-theme-error, #d32f2f);
        background: #fff8f8;
    }
    :global(.dark-theme) .contract-card.urgent {
        background: rgba(211,47,47,0.06);
    }

    .contract-card.warning {
        border-left: 4px solid #e65100;
        background: #fff8f0;
    }
    :global(.dark-theme) .contract-card.warning {
        background: rgba(230,81,0,0.06);
    }

    .contract-card.ok {
        border-left: 4px solid var(--mdc-theme-primary);
    }

    .contract-card.settled {
        border-left: 4px solid #2e7d32;
        opacity: 0.85;
    }

    .contract-card.expired {
        border-left: 4px solid #e65100;
        opacity: 0.85;
    }

    .contract-card.archived {
        opacity: 0.65;
    }

    /* ── Card layout slots ─────────────────────────────────────────── */
    .card-left {
        display: flex;
        align-items: center;
        gap: 8px;
        flex: 0 0 auto;
        min-width: 160px;
    }

    .card-text {
        display: flex;
        flex-direction: column;
    }

    .card-label {
        font-size: 0.7rem;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }

    .card-addr {
        font-size: 0.75rem;
        font-family: monospace;
        color: var(--mdc-theme-text-primary-on-background);
        word-break: break-all;
        max-width: 280px;
    }

    .card-center {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        flex: 1;
        gap: 4px;
    }

    .card-right {
        display: flex;
        flex-direction: column;
        align-items: flex-end;
        gap: 4px;
        flex: 0 0 auto;
    }

    .card-epoch {
        font-size: 0.72rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }

    /* ── Amount display ────────────────────────────────────────────── */
    .amount-remaining {
        font-size: 1.1rem;
        font-weight: 700;
        color: var(--mdc-theme-error, #d32f2f);
    }

    .amount-original {
        font-size: 0.72rem;
        color: var(--mdc-theme-text-secondary-on-surface, #999);
    }

    .amount-settled {
        font-size: 1rem;
        font-weight: 600;
        color: #2e7d32;
    }

    .amount-expired {
        font-size: 1rem;
        font-weight: 600;
        color: #e65100;
    }

    .amount-archived {
        font-size: 0.9rem;
        font-weight: 500;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }

    /* ── Deadline display ──────────────────────────────────────────── */
    .deadline-epoch {
        font-size: 0.75rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }

    .deadline-remaining {
        font-size: 0.75rem;
        font-weight: 600;
    }
    .deadline-remaining.ok      { color: var(--mdc-theme-primary); }
    .deadline-remaining.warning { color: #e65100; }
    .deadline-remaining.urgent  { color: var(--mdc-theme-error, #d32f2f); }

    /* ── Badges ────────────────────────────────────────────────────── */
    .badge {
        display: inline-flex;
        align-items: center;
        gap: 3px;
        font-size: 0.65rem;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.4px;
        padding: 2px 7px;
        border-radius: 10px;
    }

    .badge-active {
        background: var(--mdc-theme-error, #d32f2f);
        color: #fff;
    }

    .badge-settled {
        background: #2e7d32;
        color: #fff;
    }

    .badge-settled-status {
        background: #e8f5e9;
        color: #2e7d32;
        border: 1px solid #a5d6a7;
    }
    :global(.dark-theme) .badge-settled-status {
        background: rgba(46,125,50,0.15);
        border-color: rgba(46,125,50,0.3);
    }

    .badge-expired {
        background: #e65100;
        color: #fff;
    }

    .badge-expired-status {
        background: #fbe9e7;
        color: #bf360c;
        border: 1px solid #ffab91;
    }
    :global(.dark-theme) .badge-expired-status {
        background: rgba(230,81,0,0.15);
        border-color: rgba(230,81,0,0.3);
    }

    .badge-trial {
        background: #fff3e0;
        color: #e65100;
        border: 1px solid #ffcc80;
    }
    :global(.dark-theme) .badge-trial {
        background: rgba(230,81,0,0.12);
        border-color: rgba(230,81,0,0.3);
    }

    .badge-neutral {
        background: var(--mdc-theme-background, #eee);
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        border: 1px solid rgba(0,0,0,0.1);
    }
    :global(.dark-theme) .badge-neutral {
        background: rgba(255,255,255,0.07);
        border-color: rgba(255,255,255,0.1);
        color: #bbb;
    }

    /* ── Co-signers ─────────────────────────────────────────────────── */
    .cosigners-row {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 6px;
        padding: 4px 0 0 0;
        font-size: 0.75rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        border-top: 1px solid rgba(0,0,0,0.06);
        margin-top: 6px;
    }
    .cosigners-label {
        font-weight: 600;
        margin-right: 2px;
    }
    .cosigner-entry {
        display: inline-flex;
        align-items: center;
        gap: 3px;
        background: var(--mdc-theme-background, #f5f5f5);
        border-radius: 12px;
        padding: 1px 6px 1px 2px;
        font-size: 0.7rem;
    }
    .cosigner-weight {
        font-weight: 600;
        color: var(--mdc-theme-primary, #6200ee);
    }
</style>
