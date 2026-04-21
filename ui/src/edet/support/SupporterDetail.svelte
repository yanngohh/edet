<script lang="ts">
    import {_} from 'svelte-i18n';
    import {onMount} from 'svelte';
    import '@smui/circular-progress';
    import type {SupportBreakdown} from './types';
    import type {AgentPubKeyB64, Record} from "@holochain/client";
    import {decode} from "@msgpack/msgpack";
    import CircularProgress from '@smui/circular-progress';
    import {formatPercentage} from "../../common/functions.js";
    import {localizationSettings} from "../../common/localizationSettings";
    import AgentAvatar from '../transaction/AgentAvatar.svelte';

    export let record: Record;
    export let ownAddress: AgentPubKeyB64;

    let supportBreakdown: SupportBreakdown;

    $: supportBreakdown;

    onMount(async () => {
        if (record === undefined) {
            throw new Error(`The record input is required for the SupportBreakdownDetail element`);
        }
        supportBreakdown = decode((record.entry as any).Present.entry) as SupportBreakdown;
    });
</script>

{#if supportBreakdown && supportBreakdown.owner !== ownAddress}
    <div class="supporter-container flex-column">
        {#each supportBreakdown.addresses as address, i}
            {#if address == ownAddress}
                <div class="supporter-item card flex-row align-vcenter">
                    <AgentAvatar agentPubKey={supportBreakdown.owner} size={48} />
                    <div class="flex-column flex-1" style="margin-left: 16px; overflow: hidden;">
                        <span class="owner-label">{$_("mySupportersDetail.supporter")}</span>
                        <span class="address-text">{supportBreakdown.owner}</span>
                        <div class="progress-row flex-row align-vcenter">
                            <div class="progress-bar-container flex-1">
                                <div class="progress-bar" style="width: {supportBreakdown.coefficients[i] * 100}%"></div>
                            </div>
                            <span class="percent-label">{$localizationSettings ? formatPercentage(supportBreakdown.coefficients[i] * 100, 1) : ''}</span>
                        </div>
                    </div>
                </div>
            {/if}
        {/each}
    </div>
{/if}

<style>
    .supporter-container {
        gap: 16px;
        width: 100%;
    }

    .supporter-item {
        background: var(--mdc-theme-surface, #fff);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 12px;
        padding: 20px 24px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.05);
        transition: transform 0.2s ease, box-shadow 0.2s ease;
    }

    :global(.dark-theme) .supporter-item {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
        box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    }

    .owner-label {
        font-size: 0.75rem;
        text-transform: uppercase;
        font-weight: 700;
        letter-spacing: 0.5px;
        color: var(--mdc-theme-primary);
        margin-bottom: 0;
        padding: 2px 0;
        line-height: 1.2;
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
        background: var(--mdc-theme-secondary, #388e3c);
        border-radius: 4px;
        transition: width 0.5s ease-out;
    }

    .percent-label {
        font-size: 0.9rem;
        font-weight: 700;
        color: var(--mdc-theme-secondary);
        min-width: 50px;
        text-align: right;
    }
</style>
