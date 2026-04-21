<script lang="ts">
    import {_} from 'svelte-i18n';
    import {getContext, onMount} from 'svelte';
    import type {AppClient, HolochainError} from '@holochain/client';
    import {encodeHashToBase64} from '@holochain/client';
    import {clientContext} from '../../contexts';
    import CircularProgress from '@smui/circular-progress';
    import AgentAvatar from '../transaction/AgentAvatar.svelte';
    import {formatNumber, extractHolochainErrorCode} from '../../common/functions';
    import {errorStore} from '../../common/errorStore';
    import {localizationSettings} from '../../common/localizationSettings';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    interface Vouch {
        sponsor: Uint8Array;
        entrant: Uint8Array;
        amount: number;
        status: { type: 'Active' | 'Slashed' | 'Released' };
        slashed_amount: number;
        is_genesis: boolean;
        expired_contract_hash: Uint8Array | null;
    }

    let loading = true;
    let vouches: Vouch[] = [];


    $: totalEffective = vouches
        .filter(v => v.status.type === 'Active' || v.status.type === 'Slashed')
        .reduce((sum, v) => sum + Math.max(0, v.amount - v.slashed_amount), 0);

    onMount(async () => {
        await fetchVouches();
    });

    async function fetchVouches() {
        loading = true;
        try {
            vouches = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_vouches_for_entrant',
                payload: client.myPubKey,
            });
        } catch (e: any) {
            console.error(e);
            const he = e as HolochainError;
            const msg = $_('errors.' + extractHolochainErrorCode(he.message), {default: he.message});
            const errorMsg = $_('myVouchesReceived.errorFetch', {values: {name: he.name, message: msg}});
            errorStore.pushError(errorMsg);
        }
        loading = false;
    }

    function statusLabel(v: Vouch): string {
        switch (v.status.type) {
            case 'Active':   return $_('myVouchesReceived.statusActive');
            case 'Slashed':  return $_('myVouchesReceived.statusSlashed');
            case 'Released': return $_('myVouchesReceived.statusReleased');
        }
    }

    function effectiveAmount(v: Vouch): number {
        return Math.max(0, v.amount - v.slashed_amount);
    }
</script>



{#if loading}
    <div class="center-container">
        <CircularProgress class="circular-progress" indeterminate />
    </div>
{:else}
    <div class="flex-column main-container">
        <h4 class="flex-row section-title">
            {$_('myVouchesReceived.title')}
        </h4>

        <div class="helper-text">
            {$_('myVouchesReceived.helperText')}
        </div>

        {#if vouches.length === 0}
            <div class="empty-msg">{$_('myVouchesReceived.none')}</div>
        {:else}
            <div class="summary-chip flex-row align-vcenter">
                <span class="material-icons summary-icon">info</span>
                <span>
                    {$_('myVouchesReceived.totalEffective', {
                        values: {
                            amount: $localizationSettings
                                ? formatNumber(totalEffective, 2)
                                : totalEffective.toFixed(2)
                        }
                    })}
                </span>
            </div>

            <div class="vouch-list flex-column">
                {#each vouches as v}
                    {@const sponsorB64 = encodeHashToBase64(v.sponsor)}
                    <div class="vouch-item card flex-column"
                        class:is-active={v.status.type === 'Active'}
                        class:is-slashed={v.status.type === 'Slashed'}
                        class:is-released={v.status.type === 'Released'}>
                        <div class="flex-row align-vcenter" style="gap: 16px;">
                            <AgentAvatar agentPubKey={sponsorB64} size={48} />
                            <div class="flex-column flex-1" style="overflow: hidden;">
                                <div class="flex-row align-vcenter justify-between">
                                    <span class="role-label">{$_('myVouchesReceived.sponsorLabel')}</span>
                                    <span class="status-badge status-{v.status.type.toLowerCase()}">{statusLabel(v)}</span>
                                </div>
                                <span class="address-text">{sponsorB64}</span>
                                <div class="flex-row align-vcenter amounts-row" style="gap: 24px; margin-top: 8px;">
                                    <div class="flex-column">
                                        <span class="amount-label">{$_('myVouchesReceived.vouchedAmount')}</span>
                                        <span class="amount-value">{$localizationSettings ? formatNumber(v.amount, 2) : v.amount.toFixed(2)}</span>
                                    </div>
                                    {#if v.slashed_amount > 0}
                                        <div class="flex-column">
                                            <span class="amount-label slashed">{$_('myVouchesReceived.slashedAmount')}</span>
                                            <span class="amount-value slashed">{$localizationSettings ? formatNumber(v.slashed_amount, 2) : v.slashed_amount.toFixed(2)}</span>
                                        </div>
                                    {/if}
                                    <div class="flex-column">
                                        <span class="amount-label">{$_('myVouchesReceived.effectiveAmount')}</span>
                                        <span class="amount-value effective">{$localizationSettings ? formatNumber(effectiveAmount(v), 2) : effectiveAmount(v).toFixed(2)}</span>
                                    </div>
                                </div>
                                {#if v.is_genesis}
                                    <span class="genesis-badge">{$_('myVouchesReceived.genesisBadge')}</span>
                                {/if}
                            </div>
                        </div>
                    </div>
                {/each}
            </div>
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
        margin-bottom: 16px;
        line-height: 1.4;
    }

    .summary-chip {
        gap: 8px;
        background: rgba(var(--mdc-theme-primary-rgb, 98, 0, 238), 0.08);
        border: 1px solid var(--mdc-theme-primary);
        border-radius: 20px;
        padding: 8px 16px;
        margin-bottom: 20px;
        font-size: 0.9rem;
        font-weight: 600;
        color: var(--mdc-theme-primary);
        width: fit-content;
    }

    :global(.dark-theme) .summary-chip {
        background: rgba(187, 134, 252, 0.12);
    }

    .summary-icon {
        font-size: 18px;
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
</style>
