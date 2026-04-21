<script lang="ts">
    import {_} from 'svelte-i18n';
    import {getContext, onMount} from 'svelte';
    import '@smui/circular-progress';
    import type {AppClient, HolochainError, Record} from '@holochain/client';
    import {encodeHashToBase64} from '@holochain/client';
    import {clientContext} from '../../contexts';
    import '@smui/fab';
    import '@smui/icon-button';
    import SupporterDetail from "./SupporterDetail.svelte";
    import CircularProgress from '@smui/circular-progress';
    import { decode } from '@msgpack/msgpack';
    import type { SupportBreakdown } from './types';
    import {extractHolochainErrorCode} from '../../common/functions';
    import { errorStore } from '../../common/errorStore';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    let loading = true;

    let supportBreakdowns: Record[] | undefined;

    let externalSupporters: Record[] = [];

    $: if (supportBreakdowns) {
        externalSupporters = supportBreakdowns.filter(record => {
            const supportBreakdown = decode((record.entry as any).Present.entry) as SupportBreakdown;
            return supportBreakdown.owner !== encodeHashToBase64(client.myPubKey);
        });
    }

    onMount(async () => {
        await fetchOwnBeneficiaries();
    });

    async function fetchOwnBeneficiaries() {
        loading = true;
        supportBreakdowns = undefined;

        try {
            supportBreakdowns = await client.callZome({
                role_name: 'edet',
                zome_name: 'support',
                fn_name: 'get_support_breakdown_for_address',
                payload: client.myPubKey,
            });
        } catch (e: any) {
            console.error(e);
            let he = e as HolochainError;
            let message = $_("errors." + extractHolochainErrorCode(he.message), { default: he.message });
            const errorMsg = $_("mySupporters.errorFetchSupportBreakdown", {values: {name: he.name, message}});
            errorStore.pushError(errorMsg);
        }

        loading = false;
    }

</script>


{#if loading}
    <div class="center-container">    
        <CircularProgress class="circular-progress" indeterminate/>
    </div>
{:else}
    <div class="flex-column main-container">
        
        <h4 class="flex-row section-title">
            { $_("mySupporters.supportersList")}
        </h4>
        <div class="helper-text">
            {$_('mySupporters.helperText')}
        </div>
        <div class="supporter-list flex-column">
            {#if externalSupporters.length === 0}
                <div class="flex-row">
                    <div class="paragraph empty-msg">{$_("mySupporters.none")}</div>
                </div>
            {/if}

            {#each externalSupporters as supportBreakdown}
                <SupporterDetail record={supportBreakdown} ownAddress={encodeHashToBase64(client.myPubKey)}></SupporterDetail>
            {/each}
        </div>
    </div>
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

    .supporter-list {
        gap: 16px;
    }

    .empty-msg {
        padding: 24px;
        text-align: center;
        color: var(--mdc-theme-text-secondary-on-background, #888);
        background: var(--mdc-theme-background, #f5f5f5);
        border-radius: 8px;
        font-style: italic;
        width: 100%;
    }

    :global(.dark-theme) .empty-msg {
        background: rgba(255, 255, 255, 0.05);
        color: #aaa;
    }
</style>
