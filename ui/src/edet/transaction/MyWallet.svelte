<script lang="ts">
    import { _ } from 'svelte-i18n';
    import {getContext, onMount, onDestroy} from 'svelte';
    import type {ActionHash, AppClient, Record} from '@holochain/client';
    import {encodeHashToBase64, SignalType} from '@holochain/client';
    import {clientContext} from '../../contexts';
    import type {TransactionSignal} from './types';
    import Fab from '@smui/fab';
    import {Icon} from '@smui/common';
    import WalletDetail from "./WalletDetail.svelte";
    import EditWallet from "./EditWallet.svelte";
    import CircularProgress from '@smui/circular-progress';
    import { errorStore } from '../../common/errorStore';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    let wallet: Record;
    let originalWalletHash: ActionHash;

    let owner = encodeHashToBase64(client.myPubKey);
    let editing = false;
    let loading = true;

    let pollTimer: ReturnType<typeof setTimeout> | null = null;
    let unsubscribeSignal: (() => void) | null = null;
    const POLL_INTERVAL_MS = 3000;

    onMount(async () => {
        await fetchOwnWallet();
        unsubscribeSignal = client.on('signal', signal => {
            if (signal.type == SignalType.App) {
                let appSignal = signal.value;
                if (appSignal.zome_name !== 'transaction') return;
                const payload = appSignal.payload as TransactionSignal;
                if (payload.type === 'EntryCreated' || payload.type === 'EntryUpdated') {
                    fetchOwnWallet();
                }
            }
        });
        startPolling();
    });

    onDestroy(() => {
        stopPolling();
        if (unsubscribeSignal) {
            unsubscribeSignal();
            unsubscribeSignal = null;
        }
    });

    function startPolling() {
        stopPolling();
        pollTimer = setTimeout(async function tick() {
            if (!editing) {
                await fetchOwnWallet();
            }
            pollTimer = setTimeout(tick, POLL_INTERVAL_MS);
        }, POLL_INTERVAL_MS);
    }

    function stopPolling() {
        if (pollTimer !== null) {
            clearTimeout(pollTimer);
            pollTimer = null;
        }
    }

    async function fetchOwnWallet() {
        try {
            [originalWalletHash, wallet] = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_wallet_for_agent',
                payload: owner,
            });
        } catch (e: any) {
            errorStore.pushError(`Failed to load wallet: ${e?.message ?? e}`, 'error');
        } finally {
            loading = false;
            editing = false;
        }
    }
</script>
{#if loading}
    <div class="center-container">    
        <CircularProgress class="circular-progress" indeterminate/>
    </div>
{:else}
    {#if editing}
    <EditWallet record={wallet} originalActionHash={originalWalletHash}
                on:edit-canceled={() => { editing = false }}
                on:wallet-updated={async () => await fetchOwnWallet()}></EditWallet>
    {:else}
    <div class="flex-column main-container">
        <h3 class="section-title">
            { $_("myWallet.title", {default: "Wallet Dashboard"})}
        </h3>
        <div class="flex-row">
            <span class="flex-1"><WalletDetail walletRecord={wallet}></WalletDetail></span>
        </div>
    </div>
    <Fab on:click={() => { editing = true } } class="fab-edit">
        <Icon class="material-icons">edit</Icon>
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

    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin-bottom: 24px;
        font-weight: 500;
    }

    :global(.fab-edit) {
        position: fixed !important;
        bottom: 24px;
        right: 24px;
    }
</style>
