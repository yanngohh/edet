<script lang="ts">
    import {_} from 'svelte-i18n';
    import {getContext, onMount} from 'svelte';
    import type {AppClient, HolochainError} from '@holochain/client';
    import {encodeHashToBase64, decodeHashFromBase64} from '@holochain/client';
    import {clientContext} from '../../contexts';
    import CircularProgress from '@smui/circular-progress';
    import Button, { Label as ButtonLabel } from '@smui/button';
    import { Icon as ButtonIcon } from '@smui/button';
    import { Icon } from '@smui/common';
    import Fab from '@smui/fab';
    import Textfield from '@smui/textfield';
    import HelperText from '@smui/textfield/helper-text';
    import Slider from '@smui/slider';
    import AgentAvatar from '../transaction/AgentAvatar.svelte';
    import {formatNumber, isValidAddress, extractHolochainErrorCode} from '../../common/functions';
    import {errorStore} from '../../common/errorStore';
    import {localizationSettings} from '../../common/localizationSettings';
    import {BASE_CAPACITY, MAX_VOUCH_AMOUNT as MAX_VOUCH_AMOUNT_CONST} from '../../common/constants';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    const MAX_VOUCH_AMOUNT = MAX_VOUCH_AMOUNT_CONST; // mirrors Rust MAX_VOUCH_AMOUNT

    interface VouchRecord {
        original_hash: Uint8Array;
        previous_hash: Uint8Array;
        vouch: {
            sponsor: Uint8Array;
            entrant: Uint8Array;
            amount: number;
            status: { type: 'Active' | 'Slashed' | 'Released' };
            slashed_amount: number;
            is_genesis: boolean;
            expired_contract_hash: Uint8Array | null;
        };
    }

    let loading = true;
    let vouches: VouchRecord[] = [];
    let releasingIndex: number | null = null;
    // Index of the vouch waiting for release confirmation, or null if no confirmation pending.
    let confirmReleaseIndex: number | null = null;


    // Create-vouch form state
    let creating = false;
    let newAddress = '';
    let newAddressInvalid = false;
    let newAddressError = '';
    let newAmount = MAX_VOUCH_AMOUNT;
    let availableCapacity = MAX_VOUCH_AMOUNT; // max slider bound, fetched on mount
    let loadingCapacity = false;
    let submitting = false;

    $: newAddressSelf = isValidAddress(newAddress) && newAddress === encodeHashToBase64(client.myPubKey);
    $: newAddressInvalid = newAddress.length > 0 && (!isValidAddress(newAddress) || newAddressSelf);
    $: newAddressError = !isValidAddress(newAddress) && newAddress.length > 0
        ? $_('myVouchesGiven.invalidAddress')
        : newAddressSelf
            ? $_('myVouchesGiven.validationErrorSelfVouch')
            : '';
    $: isFormValid = isValidAddress(newAddress) && !newAddressSelf && newAmount > 0 && newAmount <= MAX_VOUCH_AMOUNT && !submitting;

    onMount(async () => {
        await fetchVouches();
        await fetchAvailableCapacity();
    });

    async function fetchAvailableCapacity() {
        loadingCapacity = true;
        try {
            // `get_credit_capacity` already includes the vouched amount as its base:
            //   Cap_i = V_staked + beta * ln(rel_rep) * saturation
            // where V_staked = get_vouched_capacity().
            // So `cap` already encompasses vouched capacity — adding vouchedCap again
            // would double-count it and allow the slider to exceed actual available capacity.
            const [cap, debt, locked]: [number, number, number] = await Promise.all([
                client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_credit_capacity',
                    payload: client.myPubKey,
                }),
                client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_total_debt',
                    payload: client.myPubKey,
                }),
                client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_total_locked_capacity',
                    payload: client.myPubKey,
                }),
            ]);
            const freeCapacity = Math.max(0, cap - debt - locked);
            availableCapacity = Math.min(MAX_VOUCH_AMOUNT, freeCapacity);
            if (newAmount > availableCapacity) {
                newAmount = availableCapacity;
            }
        } catch (e) {
            console.warn('Failed to fetch available capacity for vouch slider', e);
            availableCapacity = MAX_VOUCH_AMOUNT;
        }
        loadingCapacity = false;
    }

    async function fetchVouches() {
        loading = true;
        try {
            vouches = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_vouches_given',
                payload: null,
            });
        } catch (e: any) {
            console.error(e);
            const he = e as HolochainError;
            const msg = $_('errors.' + extractHolochainErrorCode(he.message), {default: he.message});
            const errorMsg = $_('myVouchesGiven.errorFetch', {values: {name: he.name, message: msg}});
            errorStore.pushError(errorMsg);
        }
        loading = false;
    }

    async function releaseVouch(index: number) {
        const v = vouches[index];
        confirmReleaseIndex = null;
        releasingIndex = index;
        try {
            await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'release_vouch',
                payload: {
                    original_vouch_hash: v.original_hash,
                    previous_vouch_hash: v.previous_hash,
                },
            });
            await fetchVouches();
        } catch (e: any) {
            console.error(e);
            const he = e as HolochainError;
            const msg = $_('errors.' + extractHolochainErrorCode(he.message), {default: he.message});
            const errorMsg = $_('myVouchesGiven.errorRelease', {values: {name: he.name, message: msg}});
            errorStore.pushError(errorMsg);
        }
        releasingIndex = null;
    }

    async function submitVouch() {
        if (!isFormValid) return;
        submitting = true;
        try {
            const agentPubKey = decodeHashFromBase64(newAddress);
            await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'create_vouch',
                payload: {
                    entrant: agentPubKey,
                    amount: newAmount,
                },
            });
            creating = false;
            newAddress = '';
            newAmount = MAX_VOUCH_AMOUNT;
            await fetchVouches();
            await fetchAvailableCapacity();
        } catch (e: any) {
            console.error(e);
            const he = e as HolochainError;
            const msg = $_('errors.' + extractHolochainErrorCode(he.message), {default: he.message});
            const errorMsg = $_('myVouchesGiven.errorCreate', {values: {name: he.name, message: msg}});
            errorStore.pushError(errorMsg);
        }
        submitting = false;
    }

    function cancelCreate() {
        creating = false;
        newAddress = '';
        newAmount = MAX_VOUCH_AMOUNT;
    }

    function statusLabel(v: VouchRecord): string {
        switch (v.vouch.status.type) {
            case 'Active': return $_('myVouchesGiven.statusActive');
            case 'Slashed': return $_('myVouchesGiven.statusSlashed');
            case 'Released': return $_('myVouchesGiven.statusReleased');
        }
    }

    function effectiveAmount(v: VouchRecord): number {
        return Math.max(0, v.vouch.amount - v.vouch.slashed_amount);
    }
</script>



{#if loading}
    <div class="center-container">
        <CircularProgress class="circular-progress" indeterminate />
    </div>
{:else if creating}
    <div class="main-container flex-column">
        <div class="edit-card card">
            <h4 class="flex-row align-vcenter header-row" style="gap: 12px;">
                <span class="form-title">{$_('myVouchesGiven.addVouch')}</span>
            </h4>
            <div class="helper-text">
                {$_('myVouchesGiven.addVouchHelperText')}
            </div>

            <div class="form-field">
                <div class="flex-row align-vcenter" style="gap: 12px;">
                    {#if isValidAddress(newAddress)}
                        <AgentAvatar agentPubKey={newAddress} size={28} />
                    {/if}
                    <Textfield
                        label={$_('myVouchesGiven.entrantAddress')}
                        bind:value={newAddress}
                        invalid={newAddressInvalid}
                        class="address-field"
                        style="width: 100%;"
                    >
                        <HelperText class="error" persistent slot="helper">{newAddressError}</HelperText>
                    </Textfield>
                </div>
            </div>

            <div class="form-field amount-section">
                <!-- The for attribute associates this label with the slider's underlying
                     <input type="range"> so screen readers announce "Amount" when focused. -->
                <label for="vouch-amount-slider" class="field-label">{$_('myVouchesGiven.amount')}</label>
                <div class="amount-display">
                    {$localizationSettings ? formatNumber(newAmount, 0) : Math.round(newAmount).toString()}
                    <span class="amount-max"> / {$localizationSettings ? formatNumber(availableCapacity, 0) : Math.round(availableCapacity).toString()}</span>
                </div>
                {#if loadingCapacity}
                    <CircularProgress indeterminate style="height:18px;width:18px;margin-top:8px;" />
                {:else if availableCapacity <= 0}
                    <div class="no-capacity-msg">{$_('myVouchesGiven.noCapacityAvailable')}</div>
                {:else}
                    <div class="slider-row">
                        <Slider
                            input$id="vouch-amount-slider"
                            class="flex-1"
                            min={0}
                            max={availableCapacity}
                            step={1}
                            bind:value={newAmount}
                        />
                    </div>
                    <div class="amount-helper">{$_('myVouchesGiven.amountHelper', {values: {max: $localizationSettings ? formatNumber(availableCapacity, 0) : Math.round(availableCapacity).toString()}})}</div>
                {/if}
            </div>

            <div class="footer-actions flex-row justify-end align-vcenter" style="margin-top: 32px; gap: 12px;">
                <Button
                    variant="outlined"
                    on:click={cancelCreate}
                    disabled={submitting}
                >
                    <ButtonLabel>{$_('myVouchesGiven.cancel')}</ButtonLabel>
                </Button>
                <Button
                    variant="raised"
                    disabled={!isFormValid || availableCapacity <= 0}
                    on:click={submitVouch}
                >
                    {#if submitting}
                        <CircularProgress indeterminate style="height:18px;width:18px;margin-right:8px;" />
                    {:else}
                        <ButtonIcon class="material-icons" style="font-size:18px;">verified_user</ButtonIcon>
                    {/if}
                    <ButtonLabel>{$_('myVouchesGiven.vouch')}</ButtonLabel>
                </Button>
            </div>
        </div>
    </div>
{:else}
    <div class="flex-column main-container">
        <h4 class="flex-row section-title">
            {$_('myVouchesGiven.title')}
        </h4>

        <div class="helper-text">
            {$_('myVouchesGiven.helperText')}
        </div>

        {#if vouches.length === 0}
            <div class="empty-msg">{$_('myVouchesGiven.none')}</div>
        {:else}
            <div class="vouch-list flex-column">
                {#each vouches as v, i}
                    {@const entrantB64 = encodeHashToBase64(v.vouch.entrant)}
                    <div class="vouch-item card flex-column" class:is-active={v.vouch.status.type === 'Active'} class:is-slashed={v.vouch.status.type === 'Slashed'} class:is-released={v.vouch.status.type === 'Released'}>
                        <div class="flex-row align-vcenter" style="gap: 16px;">
                            <AgentAvatar agentPubKey={entrantB64} size={48} />
                            <div class="flex-column flex-1" style="overflow: hidden;">
                                <div class="flex-row align-vcenter justify-between">
                                    <span class="role-label">{$_('myVouchesGiven.entrantLabel')}</span>
                                    <span class="status-badge status-{v.vouch.status.type.toLowerCase()}">{statusLabel(v)}</span>
                                </div>
                                <span class="address-text">{entrantB64}</span>
                                <div class="flex-row align-vcenter amounts-row" style="gap: 24px; margin-top: 8px;">
                                    <div class="flex-column">
                                        <span class="amount-label">{$_('myVouchesGiven.vouchedAmount')}</span>
                                        <span class="amount-value">{$localizationSettings ? formatNumber(v.vouch.amount, 2) : v.vouch.amount.toFixed(2)}</span>
                                    </div>
                                    {#if v.vouch.slashed_amount > 0}
                                        <div class="flex-column">
                                            <span class="amount-label slashed">{$_('myVouchesGiven.slashedAmount')}</span>
                                            <span class="amount-value slashed">{$localizationSettings ? formatNumber(v.vouch.slashed_amount, 2) : v.vouch.slashed_amount.toFixed(2)}</span>
                                        </div>
                                    {/if}
                                    <div class="flex-column">
                                        <span class="amount-label">{$_('myVouchesGiven.effectiveAmount')}</span>
                                        <span class="amount-value effective">{$localizationSettings ? formatNumber(effectiveAmount(v), 2) : effectiveAmount(v).toFixed(2)}</span>
                                    </div>
                                </div>
                                {#if v.vouch.is_genesis}
                                    <span class="genesis-badge">{$_('myVouchesGiven.genesisBadge')}</span>
                                {/if}
                            </div>
                        </div>

                        {#if v.vouch.status.type === 'Active'}
                            <div class="flex-row justify-end" style="margin-top: 16px;">
                                {#if confirmReleaseIndex === i}
                                    <!-- Inline confirmation row -->
                                    <span style="margin-right: 12px; align-self: center; font-size: 0.875rem; color: var(--mdc-theme-error);">
                                        {$_('myVouchesGiven.confirmRelease', {default: 'Release this vouch? This cannot be undone.'})}
                                    </span>
                                    <Button variant="outlined" style="margin-right: 8px;"
                                        on:click={() => confirmReleaseIndex = null}>
                                        {$_('common.cancel', {default: 'Cancel'})}
                                    </Button>
                                    <Button variant="raised"
                                        style="background-color: var(--mdc-theme-error); color: white;"
                                        disabled={releasingIndex === i}
                                        on:click={() => releaseVouch(i)}>
                                        {#if releasingIndex === i}
                                            <CircularProgress indeterminate style="height:18px;width:18px;margin-right:8px;" />
                                        {/if}
                                        {$_('myVouchesGiven.confirmReleaseYes', {default: 'Yes, release'})}
                                    </Button>
                                {:else}
                                    <Button
                                        variant="outlined"
                                        style="color: var(--mdc-theme-error); border-color: var(--mdc-theme-error);"
                                        disabled={releasingIndex === i}
                                        on:click={() => confirmReleaseIndex = i}
                                    >
                                        <ButtonIcon class="material-icons" style="font-size:18px;">lock_open</ButtonIcon>
                                        <ButtonLabel>{$_('myVouchesGiven.releaseVouch')}</ButtonLabel>
                                    </Button>
                                {/if}
                            </div>
                        {/if}
                    </div>
                {/each}
            </div>
        {/if}
    </div>
    <Fab on:click={() => { creating = true; }} class="fab-add">
        <Icon class="material-icons">add</Icon>
    </Fab>
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
        margin-bottom: 8px;
        font-weight: 500;
    }

    .helper-text {
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        margin-bottom: 24px;
        line-height: 1.4;
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

    .vouch-list {
        gap: 16px;
    }

    .vouch-item {
        background: var(--mdc-theme-surface, #fff);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 12px;
        padding: 20px 24px;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.05);
    }

    :global(.dark-theme) .vouch-item {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
        box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    }

    .vouch-item.is-active {
        border-left: 4px solid var(--mdc-theme-secondary, #4caf50);
    }

    .vouch-item.is-slashed {
        border-left: 4px solid var(--mdc-theme-error, #f44336);
    }

    .vouch-item.is-released {
        border-left: 4px solid var(--mdc-theme-text-hint-on-background, #ccc);
        opacity: 0.7;
    }

    .role-label {
        font-size: 0.75rem;
        text-transform: uppercase;
        font-weight: 700;
        letter-spacing: 0.5px;
        color: var(--mdc-theme-primary);
    }

    .address-text {
        font-family: monospace;
        font-size: 0.9rem;
        color: var(--mdc-theme-on-surface);
        text-overflow: ellipsis;
        overflow: hidden;
        white-space: nowrap;
        opacity: 0.9;
        margin-top: 2px;
    }

    .status-badge {
        font-size: 0.7rem;
        padding: 2px 8px;
        border-radius: 10px;
        font-weight: 700;
        text-transform: uppercase;
    }

    .status-active {
        background: rgba(76, 175, 80, 0.15);
        color: #388e3c;
    }

    :global(.dark-theme) .status-active {
        background: rgba(76, 175, 80, 0.2);
        color: #81c784;
    }

    .status-slashed {
        background: rgba(244, 67, 54, 0.15);
        color: #d32f2f;
    }

    :global(.dark-theme) .status-slashed {
        background: rgba(244, 67, 54, 0.2);
        color: #e57373;
    }

    .status-released {
        background: rgba(0, 0, 0, 0.08);
        color: #888;
    }

    :global(.dark-theme) .status-released {
        background: rgba(255, 255, 255, 0.08);
        color: #aaa;
    }

    .amounts-row {
        flex-wrap: wrap;
    }

    .amount-label {
        font-size: 0.7rem;
        text-transform: uppercase;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        font-weight: 600;
    }

    .amount-label.slashed {
        color: var(--mdc-theme-error, #d32f2f);
    }

    .amount-value {
        font-size: 1.1rem;
        font-weight: 700;
        color: var(--mdc-theme-on-surface);
    }

    .amount-value.slashed {
        color: var(--mdc-theme-error, #d32f2f);
    }

    .amount-value.effective {
        color: var(--mdc-theme-secondary, #4caf50);
    }

    :global(.dark-theme) .amount-value.effective {
        color: #81c784;
    }

    .genesis-badge {
        font-size: 0.7rem;
        background: rgba(255, 152, 0, 0.15);
        color: #e65100;
        padding: 2px 8px;
        border-radius: 10px;
        font-weight: 700;
        text-transform: uppercase;
        margin-top: 6px;
        display: inline-block;
        width: fit-content;
    }

    :global(.dark-theme) .genesis-badge {
        background: rgba(255, 152, 0, 0.2);
        color: #ffb74d;
    }

    /* Create form styles */
    .edit-card {
        width: 100%;
        background: var(--mdc-theme-surface, #fff);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 12px;
        padding: 24px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.05);
        box-sizing: border-box;
    }

    :global(.dark-theme) .edit-card {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }

    .header-row {
        margin-bottom: 8px;
        color: var(--mdc-theme-primary);
    }

    .form-title {
        font-weight: 700;
        font-size: 1.1rem;
    }

    .form-field {
        width: 100%;
    }

    .amount-section {
        margin-top: 28px;
    }

    :global(.address-field .mdc-text-field) {
        background-color: transparent !important;
    }

    .field-label {
        display: block;
        font-size: 0.75rem;
        font-weight: 700;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        text-transform: uppercase;
        letter-spacing: 0.8px;
        margin-bottom: 6px;
    }

    .amount-display {
        font-size: 1.6rem;
        font-weight: 700;
        color: var(--mdc-theme-primary);
        line-height: 1.2;
        margin-bottom: 12px;
    }

    .amount-max {
        font-size: 1rem;
        font-weight: 400;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }

    .slider-row {
        display: flex;
        flex-direction: row;
        align-items: center;
        overflow: hidden;
        width: 100%;
    }

    .amount-helper {
        font-size: 0.75rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
        margin-top: 4px;
    }

    .no-capacity-msg {
        font-size: 0.85rem;
        color: var(--mdc-theme-error, #d32f2f);
        padding: 8px 0;
        font-style: italic;
    }

    .footer-actions {
        border-top: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0,0,0,0.08));
        padding-top: 16px;
    }

    :global(.fab-add) {
        position: fixed !important;
        bottom: 24px;
        right: 24px;
    }
</style>
