<script lang="ts">
    import {_} from 'svelte-i18n';
    import {createEventDispatcher, getContext, onMount} from 'svelte';
    import type {AppClient, ActionHash, HolochainError, Record as HolochainRecord} from '@holochain/client';
    import {encodeHashToBase64, decodeHashFromBase64} from "@holochain/client";
    import {decode} from '@msgpack/msgpack';
    import {clientContext} from '../../contexts';
    import {formatPercentage, isValidAddress, extractHolochainErrorCode} from "../../common/functions";
    import {BREAKDOWN_SUM_TOLERANCE_LOWER, BREAKDOWN_SUM_TOLERANCE_UPPER, BREAKDOWN_ROUNDING_PRECISION} from "../../common/constants";
    import {localizationSettings} from "../../common/localizationSettings";
    import type {SupportBreakdown} from './types';
    import { errorStore } from '../../common/errorStore';
    import Button, { Icon as ButtonIcon, Label as ButtonLabel } from '@smui/button';
    import IconButton, { Icon } from '@smui/icon-button';
    import '@smui/slider';
    import Textfield from "@smui/textfield";
    import HelperText from '@smui/textfield/helper-text';
    import Slider from '@smui/slider';
    import AgentAvatar from '../transaction/AgentAvatar.svelte';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    const dispatch = createEventDispatcher();

    // Debounce timers for checkCapacity calls, keyed by index.
    // Prevents spamming zome calls on every keystroke.
    const CAPACITY_DEBOUNCE_MS = 300;
    let capacityDebounceTimers: Record<number, ReturnType<typeof setTimeout>> = {};

    export let record: HolochainRecord | undefined;
    export let originalRecordHash: ActionHash | undefined;

    let supportBreakdown: SupportBreakdown = {
        owner: encodeHashToBase64(client.myPubKey),
        addresses: [],
        coefficients: []
    };
    let initialSupportBreakdown: SupportBreakdown  = {
        owner: encodeHashToBase64(client.myPubKey),
        addresses: [],
        coefficients: []
    };
    type InputRepr = {
        address: string,
        invalid: boolean,
        validationError: string
    }

    let inputs: InputRepr[] = [];
    let capacities: number[] = [];

    if (record) {
        supportBreakdown = decode((record.entry as any).Present.entry) as SupportBreakdown;
        initialSupportBreakdown = decode((record.entry as any).Present.entry) as SupportBreakdown;
    }

    // Ensure owner is in the addresses list
    const ownerAddress = encodeHashToBase64(client.myPubKey);
    if (!supportBreakdown.addresses.includes(ownerAddress)) {
        supportBreakdown.addresses.unshift(ownerAddress);
        // If it was empty, give 100%, otherwise 0 (user must adjust)
        supportBreakdown.coefficients.unshift(supportBreakdown.coefficients.length === 0 ? 1.0 : 0.0);
    }

    let zeroIndexes = supportBreakdown.coefficients.map((c, i) => c == 0 ? i : -1 ).filter(i => i > 0);
    let removedAddresses = initialSupportBreakdown.addresses.filter((_address, index) => zeroIndexes.includes(index));

    initialSupportBreakdown.addresses = initialSupportBreakdown.addresses.filter((_address, index) => !zeroIndexes.includes(index));
    initialSupportBreakdown.coefficients = initialSupportBreakdown.coefficients.filter((_coefficient, index) => !zeroIndexes.includes(index));
    
    // Refresh supportBreakdown lists after potential owner addition
    supportBreakdown.addresses = [...supportBreakdown.addresses]
    supportBreakdown.coefficients = [...supportBreakdown.coefficients];
    inputs = supportBreakdown.addresses.map(address => { return { address, invalid: false, validationError: "" } });
    capacities = new Array(inputs.length).fill(-1);

    let isEditSupportBreakdownValid: boolean;

    $: supportBreakdown, isEditSupportBreakdownValid, inputs;
    $: (() => {
        inputs = inputs.map((input, index, inputs) => {
            let address = input.address;
            if (!isValidAddress(address)) {
                return {
                    address,
                    invalid: true,
                    validationError: $_("editSupportBreakdown.validationErrorAddressFormat")
                }
            } else if (inputs.slice(0, Math.max(0, index)).some(previous => previous.address.trim() == input.address.trim())) {
                return {
                    address,
                    invalid: true,
                    validationError: $_("editSupportBreakdown.validationErrorAddressDuplicate")
                }
            } else {
                supportBreakdown.addresses[index] = address;
                return {
                    address,
                    invalid: false,
                    validationError: ""
                }
            } 
        });

        // Run correction before validation to ensure we're checking the final state
        for (let i = 0; i < inputs.length; i++) {
            if (!inputs[i].invalid) {
                checkCapacity(i, inputs[i].address);
            }
            onSliderChange(i)
        }

        const ownerInBreakdown = supportBreakdown.addresses.includes(ownerAddress);
        const coeffSum = supportBreakdown.coefficients.reduce((a, b) => a + b, 0);
        const sumValid = coeffSum >= BREAKDOWN_SUM_TOLERANCE_LOWER && coeffSum <= BREAKDOWN_SUM_TOLERANCE_UPPER;
        
        // Check if anything actually changed (addresses or coefficients)
        const addressesChanged = JSON.stringify(initialSupportBreakdown.addresses) != JSON.stringify(supportBreakdown.addresses.filter(a => a != ""));
        const coefficientsChanged = JSON.stringify(initialSupportBreakdown.coefficients) != JSON.stringify(supportBreakdown.coefficients.filter((_, i) => supportBreakdown.addresses[i] != ""));
        
        isEditSupportBreakdownValid = ownerInBreakdown && sumValid && (addressesChanged || coefficientsChanged) && inputs.every(input => !input.invalid);
    })();

    onMount(() => {
        // Check capacities for existing addresses
        inputs.forEach((input, i) => {
            if (input.address && !input.invalid) {
                checkCapacity(i, input.address);
            }
        });
    });

    async function checkCapacity(index: number, address: string) {
        if (!isValidAddress(address)) return;
        // Debounce: cancel any pending call for this index and schedule a new one
        if (capacityDebounceTimers[index] !== undefined) {
            clearTimeout(capacityDebounceTimers[index]);
        }
        capacityDebounceTimers[index] = setTimeout(async () => {
            delete capacityDebounceTimers[index];
            try {
                const agentPubKey = decodeHashFromBase64(address);
                const cap = await client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_credit_capacity',
                    payload: agentPubKey
                });
                capacities[index] = cap;
            } catch (e) {
                console.warn("Failed to get capacity for", address, e);
                capacities[index] = -1;
            }
        }, CAPACITY_DEBOUNCE_MS);
    }

    

    function deleteSupport(index: number) {
        if (supportBreakdown.addresses[index] === encodeHashToBase64(client.myPubKey)) {
            errorStore.pushError($_("editSupportBreakdown.errorCannotDeleteOwner"));
            return;
        }
        let addresses = [], coefficients = [];
        addresses.push(...supportBreakdown.addresses);
        coefficients.push(...supportBreakdown.coefficients);
        inputs.splice(index, 1);
        let removedAddress = addresses.splice(index, 1)[0];
        let removedCoefficient = coefficients.splice(index, 1)[0];
        supportBreakdown.addresses = addresses;
        supportBreakdown.coefficients = coefficients.map(c => c + removedCoefficient / coefficients.length);
        if (!removedAddresses.includes(removedAddress)) {
            removedAddresses.push(removedAddress);
        }
        onSliderChange(0);
    }

    async function updateSupportBreakdown() {
        try {
            for (let encodedAddress of removedAddresses) {
                if (encodedAddress != "" && !supportBreakdown.addresses.includes(encodedAddress)) {
                    supportBreakdown.addresses.push(encodedAddress);
                    supportBreakdown.coefficients.push(0);
                }
            }

            if (!record) {
                const createRecord: HolochainRecord = await client.callZome({
                    role_name: 'edet',
                    zome_name: 'support',
                    fn_name: 'create_support_breakdown',
                    payload: supportBreakdown
                });
                dispatch('support-breakdown-updated', {actionHash: createRecord.signed_action.hashed.hash});
            } else {
                const updateRecord: HolochainRecord = await client.callZome({
                    role_name: 'edet',
                    zome_name: 'support',
                    fn_name: 'update_support_breakdown',
                    payload: {
                        original_support_breakdown_hash: originalRecordHash,
                        previous_support_breakdown_hash: record.signed_action.hashed.hash,
                        updated_support_breakdown: supportBreakdown
                    }
                });
                dispatch('support-breakdown-updated', {actionHash: updateRecord.signed_action.hashed.hash});
            }
        } catch (e: any) {
            console.error(e);
            let he = e as HolochainError;
            let message = $_("errors." + extractHolochainErrorCode(he.message), { default: he.message });
            const errorMsg = $_("editSupportBreakdown.errorEditSupportBreakdown", {values: {name: he.name, message }});
            errorStore.pushError(errorMsg);
        }
    }

    function onSliderChange(slider: number) {
        let coefficients = [...supportBreakdown.coefficients];
        let count = coefficients.length;

        // Redistribute rounding error on all sliders first
        let total = supportBreakdown.coefficients.reduce((acc, el) => acc + el, 0);
        if (total < BREAKDOWN_SUM_TOLERANCE_LOWER || total > 1) {
            supportBreakdown.coefficients = supportBreakdown.coefficients.map(c => Math.max(0, Math.min(1, c + (1 - total) / count)));
        }
        // Then redistribute reevaluated rounding error on changed slider in order to reach required precision
        do {
            total = supportBreakdown.coefficients.reduce((acc, el) => acc + el, 0);
            supportBreakdown.coefficients[slider] += 1 - total;
        } while (total < BREAKDOWN_SUM_TOLERANCE_LOWER || total > 1)
        
        // Round all coefficients to 4 decimal places to match slider step (0.0001)
        supportBreakdown.coefficients = supportBreakdown.coefficients.map(c => Math.round(c * BREAKDOWN_ROUNDING_PRECISION) / BREAKDOWN_ROUNDING_PRECISION);
        
        // After rounding, adjust the largest coefficient to ensure sum is exactly 1.0
        const roundedSum = supportBreakdown.coefficients.reduce((acc, el) => acc + el, 0);
        if (Math.abs(roundedSum - 1.0) > 1e-12) {
            // Find the index of the largest coefficient
            const maxIndex = supportBreakdown.coefficients.indexOf(Math.max(...supportBreakdown.coefficients));
            // Adjust it to make the sum exactly 1.0
            supportBreakdown.coefficients[maxIndex] = Math.round((supportBreakdown.coefficients[maxIndex] + (1.0 - roundedSum)) * BREAKDOWN_ROUNDING_PRECISION) / BREAKDOWN_ROUNDING_PRECISION;
        }
    }

</script>

<div class="edit-container flex-column">
    <div class="edit-card card">
        <h4 class="flex-row align-vcenter header-row" style="gap: 12px;">
            <span class="owner-title">{ $_("editSupportBreakdown.supportBreakdown")}</span>
        </h4>
        <div class="helper-text">
            { $_("editSupportBreakdown.supportBreakdownHelperText")}
        </div>
        
        <div class="support-list">
            {#each inputs as input, i}
                <div class="support-entry flex-column" style="gap: 8px;">
                    <div class="flex-row align-vcenter" style="gap: 16px; width: 100%;">
                        <div class="address-section flex-2">
                            <div class="flex-row align-vcenter" style="gap: 12px;">
                                {#if isValidAddress(input.address)}
                                    <AgentAvatar agentPubKey={input.address} size={28} />
                                {/if}
                                <Textfield label={$_("editSupportBreakdown.addressToSupport", {values: {index: i } })}
                                        bind:value={input.address}
                                        invalid={input.invalid}
                                        disabled={i === 0}
                                        class="address-field"
                                        style="width: 100%;">
                                    <HelperText class="error" persistent slot="helper">{input.validationError}</HelperText>
                                </Textfield>
                            </div>
                        </div>
                        
                        <div class="coefficient-section flex-1 flex-row align-vcenter" style="gap: 12px;">
                            {#if supportBreakdown.coefficients.length === 1}
                                <Slider class="flex-1" max={1} value={1} disabled></Slider>
                            {:else}
                                <Slider class="flex-1"
                                            step={0.0001}
                                            max={1}
                                            min={0}
                                            bind:value={ supportBreakdown.coefficients[i] }>
                                </Slider>
                            {/if}
                            <span class="coeff-label">{$localizationSettings ? formatPercentage(supportBreakdown.coefficients[i] * 100, 0) : ''}</span>
                        </div>

                        <IconButton class="material-icons delete-btn" 
                                    style="visibility: {i === 0 ? 'hidden' : 'visible'};"
                                    on:click={() => deleteSupport(i)}>
                            delete
                        </IconButton>
                    </div>
                </div>
            {/each}
        </div>

        <div class="footer-actions flex-row justify-between align-vcenter" style="margin-top: 24px;">
            <Button on:click={() => {
                supportBreakdown.addresses = [...supportBreakdown.addresses, ""];
                supportBreakdown.coefficients = [...supportBreakdown.coefficients, 0.0];
                inputs = [...inputs, { address: "", invalid: false, validationError: "" }];
                capacities = [...capacities, -1];
                onSliderChange(supportBreakdown.coefficients.length - 1);
            } } variant="outlined">
                <Icon class="material-icons">add</Icon> {$_("editSupportBreakdown.addSupport")}
            </Button>

            <div class="save-actions flex-row" style="gap: 12px;">
                <Button
                        on:click={() => dispatch('edit-canceled')}
                        variant="outlined"
                        disabled={!record}
                >
                    {$_("editSupportBreakdown.cancel")}
                </Button>
                <Button
                        disabled={!isEditSupportBreakdownValid}
                        on:click={() => updateSupportBreakdown()}
                        variant="raised"
                >
                    {$_("editSupportBreakdown.save")}
                </Button>
            </div>
        </div>
    </div>
</div>

<style>
    .edit-container {
        width: 100%;
        align-items: center;
    }

    .edit-card {
        width: 100%;
        background: var(--mdc-theme-surface, #fff);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 12px;
        padding: 24px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.05);
    }

    :global(.dark-theme) .edit-card {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }

    .header-row {
        margin-bottom: 8px;
        color: var(--mdc-theme-primary);
    }

    .owner-title {
        font-weight: 700;
        font-size: 1.1rem;
    }

    .helper-text {
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        margin-bottom: 24px;
        line-height: 1.4;
    }

    .support-list {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .support-entry {
        padding: 12px;
        background: var(--mdc-theme-background, #fafafa);
        border-radius: 8px;
        border: 1px solid rgba(0,0,0,0.03);
    }

    :global(.dark-theme) .support-entry {
        background: rgba(255, 255, 255, 0.02);
        border-color: rgba(255, 255, 255, 0.05);
    }

    .coeff-label {
        font-family: monospace;
        font-weight: 700;
        min-width: 40px;
        text-align: right;
        color: var(--mdc-theme-primary);
    }

    :global(.address-field .mdc-text-field) {
        background-color: transparent !important;
    }

    :global(.delete-btn) {
        color: var(--mdc-theme-error, #d32f2f) !important;
    }
</style>
