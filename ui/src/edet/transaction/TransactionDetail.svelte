<script lang="ts">
    import { _ } from 'svelte-i18n';
    import {createEventDispatcher, getContext, onMount} from 'svelte';
    import '@smui/circular-progress';
    import {decode} from '@msgpack/msgpack';
    import type {AppClient, Record} from '@holochain/client';
    import {encodeHashToBase64} from "@holochain/client";
    import {clientContext} from '../../contexts';
    import type {Transaction} from './types';
    import { isDrainTransaction } from './types';
    import Snackbar, { Label } from '@smui/snackbar';
    import '@smui/icon-button';
    import {formatDateTime, formatNumber} from "../../common/functions.js";
    import {localizationSettings} from "../../common/localizationSettings";
    import Button from '@smui/button';
    import HelperText from '@smui/textfield/helper-text';
    import AgentAvatar from './AgentAvatar.svelte';
    import { copyToClipboard } from '../../common/clipboard';
    import IconButton from '@smui/icon-button';
    import { Icon } from '@smui/common';
    import Dialog, { Actions, Content as DialogContent, Title as DialogTitle } from '@smui/dialog';
    import WalletDetail from './WalletDetail.svelte';
    import { decodeHashFromBase64 } from '@holochain/client';

    const dispatch = createEventDispatcher();

    let client: AppClient = (getContext(clientContext) as any).getClient();

    export let record: Record;
    export let vitalityImpact: number = 0;
    /** When true, all moderation buttons are disabled (action in flight or already resolved). */
    export let disabled: boolean = false;

    // Pending confirmation action: 'accept' | 'reject' | 'cancel' | null
    let pendingConfirm: 'accept' | 'reject' | 'cancel' | null = null;

    let transaction: Transaction;
    let copySnackbar: Snackbar;

    $: transaction;

    $: if (record) {
        transaction = decode((record.entry as any).Present.entry) as Transaction;
    }

    $: myPubKey = encodeHashToBase64(client.myPubKey);
    $: isSeller = myPubKey === transaction?.seller.pubkey;
    $: isBuyer = myPubKey === transaction?.buyer.pubkey;
    $: isDrain = transaction ? isDrainTransaction(transaction) : false;

    // Moderation rights: Seller moderates both purchases and drains
    // (for drains, seller = beneficiary after role realignment)
    $: isModerator = isSeller;
    // Cancellation rights: Buyer (requester) can cancel both purchases and drains
    // (for drains, buyer = supporter/requester after role realignment)
    $: canCancel = isBuyer;
 
    // ── Buyer wallet lookup (Moderator only) ──────────────────────────────────
    let peerLookupOpen = false;
    let peerWalletRecord: Record | null = null;
    let peerLookupLoading = false;
 
    async function openBuyerLookup() {
        if (!isModerator || !transaction) return;
        peerLookupLoading = true;
        peerLookupOpen = true;
        try {
            const [, record]: [unknown, Record | null] = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_wallet_for_agent',
                payload: decodeHashFromBase64(transaction.buyer.pubkey),
            });
            peerWalletRecord = record;
        } catch (e) {
            console.error('Buyer wallet lookup failed:', e);
            peerWalletRecord = null;
        }
        peerLookupLoading = false;
    }

</script>

{#if transaction !== undefined}
    <div class="transaction-card flex-column">
        <div class="card-header flex-row align-vcenter justify-between">
            <div class="timestamp">
                {$localizationSettings ? formatDateTime(record.signed_action.hashed.content.timestamp) : ''}
            </div>
            <div class="status-chip {transaction.status.type.toLowerCase()}">
                {transaction.status.type}
            </div>
            {#if transaction.is_trial}
                <div class="status-chip trial flex-row align-vcenter">
                    <Icon class="material-icons" style="font-size: 14px; margin-right: 4px;">flash_on</Icon>
                    {$_("transactionDetail.trial")}
                </div>
            {/if}
            {#if isDrainTransaction(transaction)}
                <div class="status-chip drain flex-row align-vcenter">
                    <Icon class="material-icons" style="font-size: 14px; margin-right: 4px;">favorite</Icon>
                    {$_("transactionDetail.support")}
                </div>
            {/if}
        </div>


        <div class="card-body flex-row align-vcenter">
            <div class="parties flex-column flex-2">
                {#if encodeHashToBase64(client.myPubKey) === transaction.seller.pubkey}
                    <div class="party-row flex-row align-vcenter">
                        <span class="label">{$_("transactionDetail.buyer")}:</span>
                        <AgentAvatar agentPubKey={transaction.buyer.pubkey} size={24} />
                        <span class="address-text">{transaction.buyer.pubkey}</span>
                        <div class="flex-row">
                            <IconButton class="material-icons small-icon"
                                title={$_("transactionDetail.viewBuyerMetrics", {default: "View buyer metrics"})}
                                on:click={openBuyerLookup}>
                                account_circle
                            </IconButton>
                            <IconButton class="material-icons small-icon"
                                title={$_("transactionDetail.copyAddress")}
                                on:click={() => { copyToClipboard(transaction.buyer.pubkey); copySnackbar.open(); }}>
                                content_copy
                            </IconButton>
                        </div>
                    </div>
                {:else}
                    <div class="party-row flex-row align-vcenter">
                        <span class="label">{$_("transactionDetail.seller")}:</span>
                        <AgentAvatar agentPubKey={transaction.seller.pubkey} size={24} />
                        <span class="address-text">{transaction.seller.pubkey}</span>
                        <IconButton class="material-icons small-icon"
                            title={$_("transactionDetail.copyAddress")}
                            on:click={() => { copyToClipboard(transaction.seller.pubkey); copySnackbar.open(); }}>content_copy</IconButton>
                    </div>
                {/if}
            </div>
 
            <Snackbar bind:this={copySnackbar} leading>
                <Label>{$_("transactionDetail.addressCopied")}</Label>
            </Snackbar>

            <div class="amount-section flex-column align-end flex-1">
                <div class="amount {(transaction.status.type === 'Canceled' || transaction.status.type === 'Rejected') ? 'canceled' : (isSeller ? 'success' : 'danger')}">
                    {isSeller ? '-' : '+'}{$localizationSettings ? formatNumber(transaction.debt, 2) : ''}
                </div>
                {#if transaction.status.type === 'Pending' && isModerator && vitalityImpact > 0}
                    <div class="vitality-impact flex-row align-vcenter">
                        <Icon class="material-icons vitality-icon">favorite</Icon>
                        <span class="impact-text">
                            {$_("transactionDetail.vitality_improvement", {values: {value: formatNumber(vitalityImpact, 1)}})}
                        </span>
                    </div>
                {/if}
            </div>
        </div>

        {#if transaction.description}
            <div class="description-row flex-row">
                <span class="description-text">{transaction.description}</span>
            </div>
        {/if}

        {#if transaction.status.type === "Pending"}
            <div class="actions-row flex-row justify-end">
                {#if pendingConfirm}
                    <!-- Inline confirmation: role="alertdialog" so screen readers announce it
                         immediately when it appears. aria-describedby links to the message. -->
                    <div role="alertdialog"
                        aria-modal="false"
                        aria-describedby="confirm-message"
                        class="confirm-row flex-row align-vcenter justify-end" style="gap: 8px; width: 100%;">
                        <span id="confirm-message" class="confirm-message">
                            {#if pendingConfirm === 'accept'}
                                {$_("transactionDetail.confirmAcceptance", {default: "Confirm Acceptance? This cannot be undone."})}
                            {:else if pendingConfirm === 'reject'}
                                {$_("transactionDetail.confirmRejection", {default: "Confirm Rejection? This cannot be undone."})}
                            {:else if pendingConfirm === 'cancel'}
                                {$_("transactionDetail.confirmCancellation", {default: "Confirm Cancellation? This cannot be undone."})}
                            {/if}
                        </span>
                        <Button variant="outlined" style="margin-right: 8px;"
                            on:click={() => pendingConfirm = null} disabled={disabled}>
                            {$_("transactionDetail.cancel", {default: "Cancel"})}
                        </Button>
                        <Button variant="raised"
                            autofocus
                            class="confirm-button {pendingConfirm}"
                            disabled={disabled}
                            on:click={() => {
                                const action = pendingConfirm;
                                pendingConfirm = null;
                                if (action === 'accept') dispatch('transaction-accepted');
                                else if (action === 'reject') dispatch('transaction-rejected');
                            else if (action === 'cancel') dispatch('transaction-canceled');
                        }}>
                        {$_("transactionDetail.confirmYes", {default: "Confirm"})}
                    </Button>
                    </div><!-- /confirm-row alertdialog -->
                {:else if isModerator}
                    <Button on:click={() => pendingConfirm = 'accept'} variant="raised" disabled={disabled}>
                        {#if disabled}
                            <Icon class="material-icons" style="font-size:16px;margin-right:4px;animation:spin 1s linear infinite">hourglass_empty</Icon>
                        {/if}
                        {$_("transactionDetail.accept")}
                    </Button>
                    <Button on:click={() => pendingConfirm = 'reject'} variant="outlined" style="margin-left: 8px;" disabled={disabled}>
                        {$_("transactionDetail.reject")}
                    </Button>
                {:else if canCancel}
                    <Button on:click={() => pendingConfirm = 'cancel'} variant="outlined" disabled={disabled}>
                        {$_("transactionDetail.cancel")}
                    </Button>
                {/if}
            </div>
        {/if}
 
        <!-- ── Buyer Wallet Lookup Dialog ────────────────────────────────────── -->
        <Dialog bind:open={peerLookupOpen} aria-labelledby="peer-lookup-title" class="metrics-dialog">
            <DialogTitle id="peer-lookup-title">
                <Icon class="material-icons" style="vertical-align:middle;margin-right:6px;">account_circle</Icon>
                {$_("transactionDetail.buyerMetricsTitle", {default: "Buyer Metrics"})}
                <span style="font-size:0.75rem;font-weight:400;margin-left:8px;opacity:0.6;">{transaction.buyer.pubkey.slice(0, 12)}…</span>
            </DialogTitle>
            <DialogContent>
                {#if peerLookupLoading}
                    <div style="display:flex;justify-content:center;padding:32px;">
                        <span class="material-icons" style="animation:spin 1s linear infinite;">hourglass_empty</span>
                    </div>
                {:else if peerWalletRecord}
                    <WalletDetail walletRecord={peerWalletRecord} showActions={false} />
                {:else}
                    <p style="color:var(--mdc-theme-text-secondary-on-surface);text-align:center;padding:24px;">
                        <Icon class="material-icons" style="display:block;font-size:40px;margin-bottom:8px;">person_off</Icon>
                        {$_("transactionDetail.buyerNoWallet", {default: "This buyer has not yet published a wallet."})}
                    </p>
                {/if}
            </DialogContent>
            <Actions>
                <Button on:click={() => peerLookupOpen = false}>
                    {$_("transactionDetail.close", {default: "Close"})}
                </Button>
            </Actions>
        </Dialog>
    </div>
{/if}

<style>
    .transaction-card {
        background: var(--mdc-theme-surface, #fff);
        color: var(--mdc-theme-on-surface, #000);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 8px;
        padding: 16px;
        margin-bottom: 12px;
        box-shadow: 0 2px 4px rgba(0,0,0,0.05);
        transition: box-shadow 0.2s;
    }

    :global(.dark-theme) .transaction-card {
        background: #1e1e1e; /* Slightly lighter than pure black background for elevation */
        border-color: rgba(255, 255, 255, 0.1);
        box-shadow: 0 4px 6px rgba(0,0,0,0.3);
    }

    .transaction-card:hover {
        box-shadow: 0 4px 8px rgba(0,0,0,0.1);
    }

    :global(.dark-theme) .transaction-card:hover {
        box-shadow: 0 6px 12px rgba(0,0,0,0.4);
    }

    .card-header {
        margin-bottom: 12px;
        border-bottom: 1px solid var(--mdc-theme-text-hint-on-background, #f0f0f0);
        padding-bottom: 8px;
    }

    :global(.dark-theme) .card-header {
        border-bottom-color: rgba(255, 255, 255, 0.05);
    }

    .timestamp {
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }

    .status-chip {
        padding: 4px 12px;
        border-radius: 16px;
        font-size: 0.75rem;
        font-weight: 600;
        text-transform: uppercase;
        border: 1px solid transparent;
        margin-left: 12px;
    }

    .status-chip.accepted { 
        background: var(--mdc-theme-secondary, #e8f5e9); 
        color: var(--mdc-theme-on-secondary, #2e7d32); 
    }
    .status-chip.rejected { 
        background: var(--mdc-theme-error, #ffebee); 
        color: var(--mdc-theme-on-error, #c62828); 
    }
    .status-chip.pending { 
        background: var(--mdc-theme-primary, #fff3e0); 
        color: var(--mdc-theme-on-primary, #ef6c00); 
        opacity: 0.8;
    }
    .status-chip.canceled { 
        background: var(--mdc-theme-surface, #f5f5f5); 
        color: var(--mdc-theme-text-disabled-on-surface, #616161);
        border-color: var(--mdc-theme-text-hint-on-background, #ccc);
    }
    .status-chip.trial {
        background: var(--mdc-theme-secondary, #e3f2fd);
        color: var(--mdc-theme-primary, #1976d2);
        border: 1px dashed var(--mdc-theme-primary);
    }
    .status-chip.drain {
        background: #f3e5f5;
        color: #7b1fa2;
        border: 1px solid #ce93d8;
    }
    :global(.dark-theme) .status-chip.drain {
        background: #4a1060;
        color: #e1bee7;
        border-color: #ab47bc;
    }
    .status-chip.drain-depth {
        background: #eceff1;
        color: #455a64;
        border: 1px solid #b0bec5;
    }
    :global(.dark-theme) .status-chip.drain-depth {
        background: #263238;
        color: #b0bec5;
        border-color: #546e7a;
    }
    .drain-info-bar {
        margin: 6px 0 10px;
        padding: 6px 10px;
        background: #f8f0ff;
        border-left: 3px solid #ab47bc;
        border-radius: 0 4px 4px 0;
        font-size: 0.85rem;
        color: #6a1b9a;
    }
    :global(.dark-theme) .drain-info-bar {
        background: #3a1a50;
        border-left-color: #ce93d8;
        color: #e1bee7;
    }
    .drain-info-text {
        font-style: italic;
    }

    .party-row {
        gap: 8px;
        margin-top: 4px;
    }

    .label {
        font-weight: 600;
        font-size: 0.9rem;
        color: var(--mdc-theme-text-secondary-on-background, #444);
        min-width: 60px;
    }

    .address-text {
        font-family: monospace;
        font-size: 0.85rem;
        color: var(--mdc-theme-on-surface, #000);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        max-width: 600px;
        opacity: 0.9;
    }

    :global(.dark-theme) .address-text {
        color: #e0e0e0;
        opacity: 1;
    }

    .amount {
        font-size: 1.25rem;
        font-weight: 700;
    }

    .amount.danger { color: var(--mdc-theme-error, #d32f2f); }
    .amount.success { color: var(--mdc-theme-secondary, #388e3c); }
    .amount.canceled { 
        color: var(--mdc-theme-text-disabled-on-surface, #9e9e9e); 
        opacity: 0.5;
    }

    .description-row {
        margin-top: 12px;
        padding: 8px 12px;
        background: var(--mdc-theme-background, #f9f9f9);
        border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
        border-radius: 4px;
    }

    :global(.dark-theme) .description-row {
        background: rgba(255, 255, 255, 0.05);
        border-color: rgba(255, 255, 255, 0.1);
    }

    .description-text {
        font-style: italic;
        font-size: 0.9rem;
        color: var(--mdc-theme-text-secondary-on-surface, #555);
    }

    :global(.dark-theme) .description-text {
        color: #ccc;
    }

    .actions-row {
        margin-top: 16px;
        padding-top: 12px;
        border-top: 1px solid var(--mdc-theme-text-hint-on-background, #f0f0f0);
    }

    :global(.small-icon) {
        font-size: 16px !important;
        width: 24px !important;
        height: 24px !important;
        padding: 4px !important;
        color: var(--mdc-theme-text-secondary-on-surface, #666) !important;
    }

    :global(.dark-theme) :global(.small-icon) {
        color: rgba(255, 255, 255, 0.7) !important;
    }

    .vitality-impact {
        margin-top: 4px;
        color: var(--mdc-theme-secondary, #388e3c);
        font-size: 0.8rem;
        font-weight: 600;
    }

    .vitality-icon {
        font-size: 14px !important;
        margin-right: 4px;
        color: var(--mdc-theme-secondary, #388e3c);
    }

    :global(.metrics-dialog .mdc-dialog__surface) {
        max-width: 900px !important;
        width: 95vw !important;
    }

    @keyframes spin {
        from { transform: rotate(0deg); }
        to   { transform: rotate(360deg); }
    }

    .confirm-message {
        margin-right: 12px;
        align-self: center;
        font-size: 0.875rem;
        color: var(--mdc-theme-error, #d32f2f);
        font-weight: 500;
    }

    :global(.dark-theme) .confirm-message {
        color: #ff5252; /* Brighter red for dark theme to ensure visibility */
    }

    :global(.confirm-button.accept) {
        background-color: var(--mdc-theme-secondary, #388e3c) !important;
        color: white !important;
    }

    :global(.confirm-button.reject),
    :global(.confirm-button.cancel) {
        background-color: var(--mdc-theme-error, #d32f2f) !important;
        color: white !important;
    }
</style>