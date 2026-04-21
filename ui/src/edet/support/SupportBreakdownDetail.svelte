<script lang="ts">
    import {_} from 'svelte-i18n';
    import type {SupportBreakdown} from './types';
    import type {Record} from "@holochain/client";
    import {decode} from "@msgpack/msgpack";
    import CircularProgress from '@smui/circular-progress';
    import { onMount } from 'svelte';
    import {formatPercentage} from "../../common/functions.js";
    import {localizationSettings} from "../../common/localizationSettings";
    import AgentAvatar from '../transaction/AgentAvatar.svelte';

    export let record: Record;

    let supportBreakdown: SupportBreakdown;

    $: supportBreakdown;

    onMount(async () => {
        if (!record) {
            throw new Error(`The record input is required for the SupportBreakdownDetail element`);
        }
        supportBreakdown = decode((record.entry as any).Present.entry) as SupportBreakdown;
    });

    $: externalIndices = supportBreakdown ? supportBreakdown.addresses.map((addr, i) => addr !== supportBreakdown.owner ? i : -1).filter(i => i !== -1) : [];
    $: displayAddresses = (supportBreakdown && externalIndices.length > 0) 
        ? externalIndices.map(i => supportBreakdown.addresses[i]) 
        : (supportBreakdown ? supportBreakdown.addresses : []);
    $: displayCoefficients = (supportBreakdown && externalIndices.length > 0) 
        ? externalIndices.map(i => supportBreakdown.coefficients[i]) 
        : (supportBreakdown ? supportBreakdown.coefficients : []);
    
    $: selfPercent = (supportBreakdown)
        ? (supportBreakdown.coefficients[supportBreakdown.addresses.indexOf(supportBreakdown.owner)] || 0) * 100
        : 0;
</script>

{#if supportBreakdown}
    <div class="support-container flex-column">
        {#each displayAddresses as address, i}
            <div class="support-item card flex-row align-vcenter">
                <AgentAvatar agentPubKey={address} size={36} />
                <div class="flex-column flex-1" style="margin-left: 12px; overflow: hidden;">
                    <span class="address-text">{address}</span>
                    <div class="progress-row flex-row align-vcenter">
                        <div class="progress-bar-container flex-1">
                            <div class="progress-bar" style="width: {displayCoefficients[i] * 100}%"></div>
                        </div>
                        <span class="percent-label">{$localizationSettings ? formatPercentage(displayCoefficients[i] * 100, 1) : ''}</span>
                    </div>
                </div>
            </div>
        {/each}
        {#if externalIndices.length > 0 && supportBreakdown.addresses.length > externalIndices.length}
            <div class="self-support-hint flex-row justify-center">
                <span>{$_("supportBreakdown.selfSupportHint", { values: { percent: $localizationSettings ? formatPercentage(selfPercent, 0).replace('%', '') : '' } })}</span>
            </div>
        {/if}
    </div>
{/if}

<style>
    .support-container {
        gap: 16px;
        width: 100%;
    }

    :global(.support-item.card) {
        background: var(--mdc-theme-surface, #fff) !important;
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12)) !important;
        border-radius: 12px !important;
        padding: 20px 24px !important;
        box-shadow: 0 4px 12px rgba(0,0,0,0.05) !important;
    }

    :global(.dark-theme) :global(.support-item.card) {
        background: #1e1e1e !important;
        border-color: rgba(255, 255, 255, 0.1) !important;
        box-shadow: 0 8px 24px rgba(0,0,0,0.4) !important;
    }

    .address-text {
        font-family: monospace;
        font-size: 1rem;
        color: var(--mdc-theme-on-surface);
        text-overflow: ellipsis;
        overflow: hidden;
        white-space: nowrap;
        opacity: 0.9;
    }

    .progress-row {
        gap: 12px;
        margin-top: 8px;
    }

    .progress-bar-container {
        height: 8px;
        background: rgba(0,0,0,0.05);
        border-radius: 4px;
        overflow: hidden;
    }

    :global(.dark-theme) .progress-bar-container {
        background: rgba(255, 255, 255, 0.1);
    }

    .progress-bar {
        height: 100%;
        background: var(--mdc-theme-primary, #7b1fa2);
        border-radius: 4px;
        transition: width 0.5s ease-out;
    }

    .percent-label {
        font-size: 0.9rem;
        font-weight: 700;
        color: var(--mdc-theme-primary);
        min-width: 50px;
        text-align: right;
    }

    .self-support-hint {
        margin-top: 24px;
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        font-style: italic;
        opacity: 0.8;
    }
</style>
