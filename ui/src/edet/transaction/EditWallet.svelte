<script lang="ts">
    import {_} from 'svelte-i18n';
    import {createEventDispatcher, getContext, onMount} from 'svelte';
    import type {AppClient, ActionHash, HolochainError, Record} from '@holochain/client';
    import {decode} from '@msgpack/msgpack';
    import {clientContext} from '../../contexts';
    import type {Wallet} from './types';
    import {extractHolochainErrorCode} from '../../common/functions';
    import { errorStore } from '../../common/errorStore';
    import '@smui/slider';
    import Button from '@smui/button';
    import Slider from '@smui/slider';
    import { Icon } from '@smui/common';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    // Client-side validation: accept threshold must be strictly less than reject threshold.
    // Rust integrity allows accept == reject but it produces a "reject everything" wallet.
    $: thresholdsCollapsed = wallet?.auto_accept_threshold >= wallet?.auto_reject_threshold;

    const dispatch = createEventDispatcher();

    export let record: Record;
    export let originalActionHash: ActionHash;

    let wallet: Wallet;
    let initialWallet: Wallet;

    if (record !== undefined) {
        wallet = decode((record.entry as any).Present.entry) as Wallet;
        initialWallet = decode((record.entry as any).Present.entry) as Wallet;
    }

    async function updateWallet() {
        try {
            const updateRecord: Record = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'update_wallet',
                payload: {
                    original_wallet_hash: originalActionHash,
                    previous_wallet_hash: record.signed_action.hashed.hash,
                    updated_wallet: wallet
                }
            });
            dispatch('wallet-updated', {actionHash: updateRecord.signed_action.hashed.hash});
        } catch (e: any) {
            console.error(e);
            let he = e as HolochainError;
            let message = $_("errors." + extractHolochainErrorCode(he.message), { default: he.message });
            const errorMsg = $_("editWallet.errorEditWallet", {values: {name: he.name, message}});
            errorStore.pushError(errorMsg);
        }
    }

</script>
<div class="main-container">
    <div class="edit-wallet-card card elevation-z2" 
         style="--accept-pct: {wallet.auto_accept_threshold * 100}%; --reject-pct: {wallet.auto_reject_threshold * 100}%">
        <div class="card-header flex-row align-vcenter">
            <Icon class="material-icons header-icon">tune</Icon>
            <div class="flex-column">
                <h4 class="rule-title">{$_("editWallet.acceptanceRule")}</h4>
                <p class="rule-description">{$_("editWallet.acceptanceRuleHelperText")}</p>
            </div>
        </div>

        <div class="threshold-management">
            <div class="threshold-labels flex-row">
                <div class="threshold-label safe flex-column align-vcenter">
                    <Icon class="material-icons success-text">verified_user</Icon>
                    <span>{$_("editWallet.safeTransactions")}</span>
                </div>
                
                <div class="threshold-label manual flex-column align-vcenter flex-1">
                    <Icon class="material-icons manual-moderation-icon">visibility</Icon>
                    <span>{$_("editWallet.manualModeration")}</span>
                </div>

                <div class="threshold-label unsafe flex-column align-vcenter">
                    <Icon class="material-icons error-text">warning</Icon>
                    <span>{$_("editWallet.unsafeTransactions")}</span>
                </div>
            </div>

            <div class="slider-wrapper">
                <Slider range class="wallet-thresholds"
                        max={1}
                        min={0}
                        step={0.00000000000001}
                        bind:end={wallet.auto_reject_threshold}
                        bind:start={wallet.auto_accept_threshold}>
                </Slider>
            </div>
            
            <div class="legend flex-row flex-hcenter">
                <div class="legend-item flex-row align-vcenter">
                    <div class="dot auto-accept"></div>
                    <span>{$_("editWallet.automaticAcceptance", {default: "Automatic Acceptance"})}</span>
                </div>
                <div class="legend-item flex-row align-vcenter">
                    <div class="dot manual"></div>
                    <span>{$_("editWallet.manualModeration")}</span>
                </div>
                <div class="legend-item flex-row align-vcenter">
                    <div class="dot auto-reject"></div>
                    <span>{$_("editWallet.automaticRefusal", {default: "Automatic Refusal"})}</span>
                </div>
            </div>
        </div>

        {#if thresholdsCollapsed}
            <div class="threshold-warning">
                <Icon class="material-icons" style="font-size:1.1rem;vertical-align:middle;margin-right:4px">warning</Icon>
                {$_("editWallet.validationErrorThresholdsEqual")}
            </div>
        {/if}

        <div class="actions-row flex-row flex-hcenter">
            <Button
                    class="action-btn"
                    disabled={!record}
                    on:click={() => dispatch('edit-canceled')}
                    variant="outlined"
            >{$_("editWallet.cancel")}</Button>
            <Button
                    class="action-btn"
                    disabled={JSON.stringify(wallet) == JSON.stringify(initialWallet) || thresholdsCollapsed}
                    on:click={() => updateWallet()}
                    variant="raised"
            >{$_("editWallet.save")}</Button>
        </div>
    </div>
</div>

<style>
    .main-container {
        padding: 24px;
        width: 100%;
        display: flex;
        justify-content: center;
    }

    .threshold-warning {
        background: rgba(255, 152, 0, 0.12);
        border: 1px solid rgba(255, 152, 0, 0.4);
        border-radius: 8px;
        padding: 8px 12px;
        margin-bottom: 8px;
        font-size: 0.85rem;
        color: #e65100;
    }
    :global(.dark-theme) .threshold-warning {
        color: #ffb74d;
        border-color: rgba(255, 183, 77, 0.4);
        background: rgba(255, 183, 77, 0.08);
    }

    .edit-wallet-card {
        background: var(--mdc-theme-surface, #fff);
        padding: 32px;
        border-radius: 16px;
        max-width: 800px;
        width: 100%;
        box-shadow: 0 10px 25px rgba(0,0,0,0.1);
    }

    :global(.dark-theme) .edit-wallet-card {
        background: #1e1e1e;
        border: 1px solid rgba(255,255,255,0.05);
        box-shadow: 0 15px 35px rgba(0,0,0,0.5);
    }

    .card-header {
        margin-bottom: 32px;
        gap: 16px;
    }

    :global(.edit-wallet-card .header-icon) {
        font-size: 36px !important;
        color: var(--mdc-theme-primary);
        opacity: 0.8;
    }

    .rule-title {
        margin: 0;
        font-size: 1.5rem;
        font-weight: 700;
        color: var(--mdc-theme-on-surface);
    }

    .rule-description {
        margin: 4px 0 0 0;
        font-size: 0.9rem;
        color: var(--mdc-theme-text-secondary-on-surface);
        line-height: 1.5;
    }

    .threshold-management {
        background: rgba(0,0,0,0.02);
        padding: 32px 16px;
        border-radius: 12px;
        margin-bottom: 32px;
    }

    :global(.dark-theme) .threshold-management {
        background: rgba(255,255,255,0.02);
    }

    .threshold-labels {
        margin-bottom: 24px;
        padding: 0 4px;
    }

    .threshold-label {
        width: 120px;
        font-size: 0.7rem;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: var(--mdc-theme-text-secondary-on-surface);
        gap: 8px;
        text-align: center;
    }

    .threshold-label.flex-1 {
        opacity: 0.8;
    }

    .slider-wrapper {
        padding: 0 8px;
        margin-bottom: 32px;
    }

    :global(.wallet-thresholds) {
        width: 100% !important;
    }

    /* Slider track color accents */
    :global(.wallet-thresholds .mdc-slider__track--inactive) {
        background-color: var(--mdc-theme-on-surface, #000) !important;
        opacity: 0.1 !important;
    }

    :global(.dark-theme .wallet-thresholds .mdc-slider__track--inactive) {
        opacity: 0.2 !important;
    }

    :global(.wallet-thresholds .mdc-slider__track--active_fill) {
        background-color: var(--mdc-theme-primary) !important;
        opacity: 0.8 !important;
    }

    .legend {
        gap: 24px;
        flex-wrap: wrap;
    }

    .legend-item {
        gap: 8px;
        font-size: 0.8rem;
        font-weight: 500;
        color: var(--mdc-theme-text-secondary-on-surface);
    }

    .dot {
        width: 10px;
        height: 10px;
        border-radius: 50%;
    }

    .dot.auto-accept { background-color: #4caf50; }
    .dot.manual { background-color: var(--mdc-theme-primary, #9c27b0); }
    .dot.auto-reject { background-color: #f44336; }

    .actions-row {
        gap: 16px;
    }

    :global(.action-btn) {
        min-width: 140px !important;
        height: 48px !important;
        border-radius: 24px !important;
    }

    :global(.edit-wallet-card .manual-moderation-icon) {
        color: var(--mdc-theme-primary, #9c27b0) !important;
    }

    :global(.edit-wallet-card .success-text) { color: #4caf50 !important; }
    :global(.edit-wallet-card .error-text) { color: #f44336 !important; }
</style>
