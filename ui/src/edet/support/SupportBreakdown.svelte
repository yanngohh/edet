<script lang="ts">
    import {_} from 'svelte-i18n';
    import {getContext, onMount} from 'svelte';
    import '@smui/circular-progress';
    import type {AppClient, ActionHash, HolochainError, Record} from '@holochain/client';
    import {clientContext} from '../../contexts';
    import {extractHolochainErrorCode} from '../../common/functions';
    import { errorStore } from '../../common/errorStore';
    import Fab, { Icon } from '@smui/fab';
    import EditSupportBreakdown from './EditSupportBreakdown.svelte';
    import SupportBreakdownDetail from "./SupportBreakdownDetail.svelte";
    import CircularProgress from '@smui/circular-progress';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    let loading = true;

    let supportBreakdown: Record | undefined;
    let originalSupportBreakdownHash: ActionHash | undefined;

    let editing = false;
    let isSupportBreakdownCreateValid = false;


    $: editing, loading, isSupportBreakdownCreateValid, supportBreakdown;

    onMount(async () => {
        await fetchOwnSupportBreakdown();
    });

    async function fetchOwnSupportBreakdown() {
        loading = true;
        supportBreakdown = undefined;

        try {
            [originalSupportBreakdownHash, supportBreakdown] = await client.callZome({
                role_name: 'edet',
                zome_name: 'support',
                fn_name: 'get_support_breakdown_for_owner',
                payload: client.myPubKey,
            });
            isSupportBreakdownCreateValid = supportBreakdown === null;
        } catch (e: any) {
            console.error(e);
            let he = e as HolochainError;
            let message = $_("errors." + extractHolochainErrorCode(he.message), { default: he.message });
            const errorMsg = $_("supportBreakdown.errorFetchSupportBreakdown", {values: {name: he.name, message}});
            errorStore.pushError(errorMsg);
        }

        loading = false;
    }

</script>




{#if loading}
    <div class="center-container">    
        <CircularProgress class="circular-progress" indeterminate/>
    </div>
{:else if editing}
    <div class="main-container">
        <EditSupportBreakdown
                record={supportBreakdown}
                originalRecordHash={originalSupportBreakdownHash}
                on:support-breakdown-updated={async () => {
                    editing = false;
                    await fetchOwnSupportBreakdown()
                } }
                on:edit-canceled={() => { editing = false; } }
        ></EditSupportBreakdown>
    </div>
{:else}
    <div class="main-container flex-column">
        {#if isSupportBreakdownCreateValid}
            <EditSupportBreakdown
                    record={supportBreakdown}
                    originalRecordHash={originalSupportBreakdownHash}
                    on:support-breakdown-updated={async () => await fetchOwnSupportBreakdown()}
            ></EditSupportBreakdown>
        {:else if supportBreakdown}
            <SupportBreakdownDetail record={supportBreakdown}></SupportBreakdownDetail>
            <Fab class="fab-edit" on:click={() => { editing = true } }>
                <Icon class="material-icons">edit</Icon>
            </Fab>
        {/if}
    </div>
{/if}

<style>
    .main-container {
        width: 100%;
        margin: 0;
        padding: 16px;
        box-sizing: border-box;
    }

    :global(.fab-edit) {
        position: fixed !important;
        bottom: 24px;
        right: 24px;
    }
</style>

