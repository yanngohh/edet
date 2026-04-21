<script lang="ts">
    import {_} from 'svelte-i18n';
    import {getContext, onMount, onDestroy, tick} from 'svelte';
    import type {AppClient, HolochainError, Record} from '@holochain/client';
    import {encodeHashToBase64} from '@holochain/client';
    import {clientContext} from '../../contexts';
    import { errorStore } from '../../common/errorStore';
    import type { TransactionSignal, PaginatedTransactionsResult } from './types';
    import {extractHolochainErrorCode, formatNumber, formatDateTime} from '../../common/functions';
    import '@smui/snackbar';
    import '@smui/fab';
    import '@smui/icon-button';
    import { Icon } from '@smui/common';
    import TransactionDetail from './TransactionDetail.svelte';
    import EditTransaction from "./EditTransaction.svelte";
    import CreateTransaction from "./CreateTransaction.svelte";
    import Fab from '@smui/fab';
    import Button from '@smui/button';
    import Snackbar, { Label } from '@smui/snackbar';
    import type { GetTransactionsCursor, Wallet } from './types';
    import CircularProgress from '@smui/circular-progress';
    import { now } from '../../common/functions';

    const PAGE_SIZE = 20;

    let client: AppClient = (getContext(clientContext) as any).getClient();

    let transactionsPending: Record[] = [];
    let transactionsFinalized: Record[] = [];

    let ownWallet: Wallet | null;
    let creating = false;
    let loading = true;
    let error: string = "";
    let unsubscribeSignal: (() => void) | undefined;

    // Pagination state
    let pendingCursor: number | null = null;
    let finalizedCursor: number | null = null;
    let pendingHasMore = true;
    let finalizedHasMore = true;
    let loadingMorePending = false;
    let loadingMoreFinalized = false;

    // IntersectionObserver sentinels
    let pendingSentinel: HTMLElement;
    let finalizedSentinel: HTMLElement;
    let pendingObserver: IntersectionObserver | null = null;
    let finalizedObserver: IntersectionObserver | null = null;

    $: ownWallet, creating, loading, error, transactionsPending, transactionsFinalized;

    let pollingInterval = 3000;
    let timeoutId: any;

    onMount(async () => {
        await fetchInitialTransactions();
        startPolling();
        unsubscribeSignal = client.on('signal', (signal: any) => {
            const s = signal as TransactionSignal;
            if (s.type === 'EntryCreated' || s.type === 'EntryUpdated') {
                if ('Transaction' in s.app_entry || (s.app_entry as any).type === 'Transaction') {
                    refreshFirstPage();
                }
            }
        });
        await tick();
        setupObservers();
    });

    onDestroy(() => {
        if (timeoutId) clearTimeout(timeoutId);
        if (unsubscribeSignal) unsubscribeSignal();
        if (pendingObserver) pendingObserver.disconnect();
        if (finalizedObserver) finalizedObserver.disconnect();
    });

    function setupObservers() {
        const options = { rootMargin: '200px' };

        if (pendingSentinel) {
            pendingObserver = new IntersectionObserver((entries) => {
                if (entries[0]?.isIntersecting && pendingHasMore && !loadingMorePending) {
                    loadMorePending();
                }
            }, options);
            pendingObserver.observe(pendingSentinel);
        }

        if (finalizedSentinel) {
            finalizedObserver = new IntersectionObserver((entries) => {
                if (entries[0]?.isIntersecting && finalizedHasMore && !loadingMoreFinalized) {
                    loadMoreFinalized();
                }
            }, options);
            finalizedObserver.observe(finalizedSentinel);
        }
    }

    async function startPolling() {
        timeoutId = setTimeout(async () => {
            if (!creating) {
                await refreshFirstPage();
            }
            startPolling();
        }, pollingInterval);
    }

    /** Fetch wallet + initial pages of both Pending and Finalized. */
    async function fetchInitialTransactions() {
        loading = true;
        transactionsPending = [];
        transactionsFinalized = [];
        pendingHasMore = true;
        finalizedHasMore = true;

        try {
            const timestamp = await now(client);

            const [_walletHash, walletRecord] = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_wallet_for_agent',
                payload: client.myPubKey,
            });
            ownWallet = walletRecord;

            // Self-heal: reconcile missing contracts (fire-and-forget)
            client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'reconcile_missing_contracts',
                payload: null,
            }).catch((e: any) => console.warn('reconcile_missing_contracts failed (non-critical):', e));

            const pendingResult: PaginatedTransactionsResult = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_transactions',
                payload: {
                    count: PAGE_SIZE,
                    from_timestamp: timestamp,
                    tag: { type: "Pending" },
                    direction: { type: "Descendent" },
                    drain_filter: 'BeneficiaryOnly',
                } as GetTransactionsCursor,
            });
            transactionsPending = pendingResult.records;
            pendingCursor = pendingResult.next_cursor;
            pendingHasMore = pendingResult.next_cursor !== null;

            const finalizedResult: PaginatedTransactionsResult = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_transactions',
                payload: {
                    count: PAGE_SIZE,
                    from_timestamp: timestamp,
                    tag: { type: "Finalized" },
                    direction: { type: "Descendent" },
                    drain_filter: 'BeneficiaryOnly',
                } as GetTransactionsCursor,
            });
            transactionsFinalized = finalizedResult.records;
            finalizedCursor = finalizedResult.next_cursor;
            finalizedHasMore = finalizedResult.next_cursor !== null;
        } catch (e: any) {
            console.error(e);
            let he = e as HolochainError;
            let message = $_("errors." + extractHolochainErrorCode(he.message), { default: he.message });
            error = $_("myTransactions.errorFetchTransactions", {values: {name: he.name, message }});
            errorStore.pushError(error);
        }

        loading = false;
    }

    /** Refresh only the first page (for polling / signal). */
    async function refreshFirstPage() {
        try {
            const timestamp = await now(client);

            const pendingResult: PaginatedTransactionsResult = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_transactions',
                payload: {
                    count: PAGE_SIZE,
                    from_timestamp: timestamp,
                    tag: { type: "Pending" },
                    direction: { type: "Descendent" },
                    drain_filter: 'BeneficiaryOnly',
                } as GetTransactionsCursor,
            });

            // For Pending: REPLACE entirely rather than merge.
            // Pending transactions are few (manual moderation backlog) and must reflect
            // the live state precisely — any item that left the Pending index (accepted,
            // rejected, or canceled) must disappear from the list immediately.
            // Items older than PAGE_SIZE are kept from the current array so we don't lose
            // them, but they are re-validated: any item that now appears as Finalized is dropped.
            const freshPendingHashes = new Set(
                pendingResult.records.map(r => encodeHashToBase64(r.signed_action.hashed.hash))
            );
            if (pendingCursor === null) {
                // All pending fit in one page — full replace is safe
                transactionsPending = pendingResult.records;
            } else {
                // Multi-page: keep older items, but prune any that have been moderated
                const olderPending = transactionsPending.filter(
                    r => !freshPendingHashes.has(encodeHashToBase64(r.signed_action.hashed.hash))
                );
                transactionsPending = [...pendingResult.records, ...olderPending];
            }
            pendingCursor = pendingResult.next_cursor;
            pendingHasMore = pendingResult.next_cursor !== null;

            const finalizedResult: PaginatedTransactionsResult = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_transactions',
                payload: {
                    count: PAGE_SIZE,
                    from_timestamp: timestamp,
                    tag: { type: "Finalized" },
                    direction: { type: "Descendent" },
                    drain_filter: 'BeneficiaryOnly',
                } as GetTransactionsCursor,
            });
            // For Finalized: prepend new items only, preserving already-loaded pages.
            const freshFinalizedHashes = new Set(
                finalizedResult.records.map(r => encodeHashToBase64(r.signed_action.hashed.hash))
            );
            const olderFinalized = transactionsFinalized.filter(
                r => !freshFinalizedHashes.has(encodeHashToBase64(r.signed_action.hashed.hash))
            );
            transactionsFinalized = [...finalizedResult.records, ...olderFinalized];
        } catch (e: any) {
            // Silent refresh — don't show error banner to user
            console.error('Silent refresh failed:', e);
        }
    }

    /** Load next page of pending transactions. */
    async function loadMorePending() {
        if (!pendingHasMore || loadingMorePending || pendingCursor === null) return;
        loadingMorePending = true;

        try {
            const result: PaginatedTransactionsResult = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_transactions',
                payload: {
                    count: PAGE_SIZE,
                    from_timestamp: pendingCursor,
                    tag: { type: "Pending" },
                    direction: { type: "Descendent" },
                    drain_filter: 'BeneficiaryOnly',
                } as GetTransactionsCursor,
            });

            // Dedup and append
            const existingHashes = new Set(transactionsPending.map(r => encodeHashToBase64(r.signed_action.hashed.hash)));
            const newRecords = result.records.filter(r => !existingHashes.has(encodeHashToBase64(r.signed_action.hashed.hash)));
            transactionsPending = [...transactionsPending, ...newRecords];
            pendingCursor = result.next_cursor;
            pendingHasMore = result.next_cursor !== null;
        } catch (e: any) {
            console.error('Load more pending failed:', e);
        }

        loadingMorePending = false;
    }

    /** Load next page of finalized transactions. */
    async function loadMoreFinalized() {
        if (!finalizedHasMore || loadingMoreFinalized || finalizedCursor === null) return;
        loadingMoreFinalized = true;

        try {
            const result: PaginatedTransactionsResult = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_transactions',
                payload: {
                    count: PAGE_SIZE,
                    from_timestamp: finalizedCursor,
                    tag: { type: "Finalized" },
                    direction: { type: "Descendent" },
                    drain_filter: 'BeneficiaryOnly',
                } as GetTransactionsCursor,
            });

            const existingHashes = new Set(transactionsFinalized.map(r => encodeHashToBase64(r.signed_action.hashed.hash)));
            const newRecords = result.records.filter(r => !existingHashes.has(encodeHashToBase64(r.signed_action.hashed.hash)));
            transactionsFinalized = [...transactionsFinalized, ...newRecords];
            finalizedCursor = result.next_cursor;
            finalizedHasMore = result.next_cursor !== null;
        } catch (e: any) {
            console.error('Load more finalized failed:', e);
        }

        loadingMoreFinalized = false;
    }

</script>

{#if loading}
    <div class="center-container">    
        <CircularProgress class="circular-progress" indeterminate/>
    </div>
{:else if creating}
    <div class="main-container">
        <CreateTransaction on:transaction-created={async () => { await fetchInitialTransactions(); creating = false; }} on:create-canceled={() => { creating = false; } }></CreateTransaction>
    </div>
{:else}
    <div class="flex-column main-container">
        {#if ownWallet === null}
            <div class="flex-2 warning-msg">
                <div class="flex-row warning-title align-vcenter">
                    <Icon class="material-icons">warning</Icon> 
                    <span style="margin-left: 8px;">{$_("myTransactions.warningNoWalletTitle")}</span>
                </div>
                <div class="flex-row warning-info">{$_("myTransactions.warningNoWalletInfo")}</div>
            </div>
        {/if}


        <div class="section-container">
            <h4 class="section-title">
                { $_("myTransactions.pending")}
            </h4>
            <div class="helper-text">
                {$_('myTransactions.pendingHelperText')}
            </div>
            <div class="transaction-list">
                {#if transactionsPending.length === 0}
                    <div class="empty-msg">{$_("myTransactions.nonePending")}</div>
                {:else}
                    {#each transactionsPending as transaction (encodeHashToBase64(transaction.signed_action.hashed.hash))}
                        <EditTransaction
                            record={transaction}
                            on:transaction-updated={async (e) => {
                                // Optimistic removal: immediately drop the card from the list
                                // using the original action hash from the event. This prevents
                                // the stale Pending card from lingering after accept/reject.
                                const originalHash = e.detail?.originalHash;
                                if (originalHash) {
                                    const hashB64 = encodeHashToBase64(originalHash);
                                    transactionsPending = transactionsPending.filter(
                                        r => encodeHashToBase64(r.signed_action.hashed.hash) !== hashB64
                                    );
                                }
                                // Background sync to pick up the new Finalized record
                                await refreshFirstPage();
                            }}
                        ></EditTransaction>
                    {/each}
                {/if}
                <!-- Infinite scroll sentinel -->
                <div bind:this={pendingSentinel} class="scroll-sentinel">
                    {#if loadingMorePending}
                        <div class="loading-more"><CircularProgress class="circular-progress-small" indeterminate/></div>
                    {/if}
                </div>
            </div>
        </div>

        <div class="section-container" style="margin-top: 32px;">
            <h4 class="section-title">
                { $_("myTransactions.finalized")}
            </h4>
            <div class="helper-text">
                {$_('myTransactions.finalizedHelperText')}
            </div>
            <div class="transaction-list">
                {#if transactionsFinalized.length === 0}
                    <div class="empty-msg">{$_("myTransactions.noneFinalized")}</div>
                {:else}
                    {#each transactionsFinalized as transaction (encodeHashToBase64(transaction.signed_action.hashed.hash))}
                        <TransactionDetail record={transaction}></TransactionDetail>
                    {/each}
                {/if}
                <!-- Infinite scroll sentinel -->
                <div bind:this={finalizedSentinel} class="scroll-sentinel">
                    {#if loadingMoreFinalized}
                        <div class="loading-more"><CircularProgress class="circular-progress-small" indeterminate/></div>
                    {/if}
                </div>
            </div>
        </div>
    </div>
    {#if ownWallet !== null}
        <Fab on:click={() => { creating = true } } class="fab-add">
            <Icon class="material-icons">add</Icon>
        </Fab>
    {/if}
{/if}

<style>
    .main-container {
        width: 100%;
        margin: 0;
        padding: 16px;
        box-sizing: border-box;
    }

    .section-container {
        width: 100%;
    }

    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin-bottom: 8px;
        font-weight: 500;
    }

    .helper-text {
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        margin-bottom: 24px;
        line-height: 1.4;
    }

    .transaction-list {
        display: flex;
        flex-direction: column;
    }

    .empty-msg {
        padding: 24px;
        text-align: center;
        color: var(--mdc-theme-text-secondary-on-background, #888);
        background: var(--mdc-theme-background, #f5f5f5);
        border-radius: 8px;
        font-style: italic;
    }

    :global(.dark-theme) .empty-msg {
        background: rgba(255, 255, 255, 0.05);
        color: #aaa;
    }

    .warning-msg {
        background: #fff3e0;
        border: 1px solid #ffe0b2;
        border-radius: 8px;
        padding: 16px;
        margin-bottom: 24px;
    }

    :global(.dark-theme) .warning-msg {
        background: #2c1a00;
        border-color: #4b3d2b;
    }

    .warning-title {
        color: #e65100;
        font-weight: 600;
        margin-bottom: 8px;
    }

    :global(.dark-theme) .warning-title {
        color: #ffb74d;
    }

    .warning-info {
        color: #ef6c00;
        font-size: 0.9rem;
    }

    :global(.dark-theme) .warning-info {
        color: #ffe0b2;
    }

    :global(.fab-add) {
        position: fixed !important;
        bottom: 24px;
        right: 24px;
    }

    .scroll-sentinel {
        min-height: 1px;
        width: 100%;
    }

    .loading-more {
        display: flex;
        justify-content: center;
        padding: 16px 0;
    }

    :global(.circular-progress-small) {
        width: 24px !important;
        height: 24px !important;
    }
</style>
