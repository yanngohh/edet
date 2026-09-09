<script lang="ts">
    /**
     * The codes this device has signed and may still have to show: a
     * purchase the seller has not scanned yet. Kept until its window closes
     * or the holder forgets it, since the node never sees it and cannot say
     * whether it was used.
     */
    import { _ } from 'svelte-i18n';
    import Explain from './Explain.svelte';
    import Button, { Label } from '@smui/button';

    import QrCodeDisplay from './QrCodeDisplay.svelte';
    import { forgetOffer, myOffers, pruneOffers, type OpenOffer } from '../lib/offer';
    import { networkView } from '../lib/node';
    import { amountsSealed, copyText, formatAmount, memberName } from '../lib/display';
    import { epochDate } from '../lib/epoch';
    import { partyKeyHex, partyMember } from '../lib/api';
    import { formatNumber } from '../common/functions';
    import { errorStore } from '../common/errorStore';

    /** Whether to explain the list; off where the screen around it already does. */
    export let intro = true;

    let shown: string | null = null;

    $: epoch = $networkView?.epoch ?? 0;
    $: if (epoch > 0) pruneOffers(epoch);
    $: sellerOf = (o: OpenOffer): string => {
        const id = partyMember(o.seller);
        return id !== null ? $memberName(id) : `${(partyKeyHex(o.seller) ?? '').slice(0, 8)}…`;
    };

    async function copy(o: OpenOffer) {
        if (await copyText(o.payload)) {
            errorStore.pushError($_('common.copied', { default: 'Copied to clipboard' }), 'warning');
        }
    }
</script>

{#if $myOffers.length > 0}
    <div class="open-offers">
        <h3 class="section-title">{$_('offers.openTitle', { default: 'Codes you have shown' })}</h3>
        {#if intro}
            <Explain summary={$_('offers.openSummary', { default: 'Purchases you signed that wait for the seller.' })}>
                {$_('offers.openIntro', {
                    default:
                        'One sent to their app needs nothing more; show the code only if they cannot see it. Once they sign, the trade appears in your history.',
                })}
            </Explain>
        {/if}
        <div class="list">
            {#each $myOffers as o (o.payload)}
                <div class="offer">
                    <span class="line">
                        {#if o.sent}
                            {$_('offers.openLineSent', {
                                values: {
                                    amount: formatAmount(formatNumber, o.amount, $amountsSealed),
                                    seller: sellerOf(o),
                                    date: $epochDate(o.notAfterEpoch),
                                },
                                default: `Buying ${formatAmount(formatNumber, o.amount, $amountsSealed)} from ${sellerOf(o)} — sent to their app, valid until ${$epochDate(o.notAfterEpoch)}`,
                            })}
                        {:else}
                            {$_('offers.openLine', {
                                values: {
                                    amount: formatAmount(formatNumber, o.amount, $amountsSealed),
                                    seller: sellerOf(o),
                                    date: $epochDate(o.notAfterEpoch),
                                },
                                default: `Buying ${formatAmount(formatNumber, o.amount, $amountsSealed)} from ${sellerOf(o)} — valid until ${$epochDate(o.notAfterEpoch)}`,
                            })}
                        {/if}
                    </span>
                    <div class="actions">
                        <Button variant="raised" on:click={() => (shown = shown === o.payload ? null : o.payload)}>
                            <Label>
                                {shown === o.payload
                                    ? $_('offers.hideCode', { default: 'Hide code' })
                                    : $_('offers.showCode', { default: 'Show code' })}
                            </Label>
                        </Button>
                        <Button variant="outlined" on:click={() => copy(o)}>
                            <Label>{$_('offers.copy', { default: 'Copy as text' })}</Label>
                        </Button>
                        <Button variant="outlined" on:click={() => forgetOffer(o.payload)}>
                            <Label>{$_('offers.forget', { default: 'Forget' })}</Label>
                        </Button>
                    </div>
                    {#if shown === o.payload}
                        <div class="code">
                            <QrCodeDisplay value={o.payload} size={240} level="L" />
                        </div>
                    {/if}
                    <Explain
                        tone="notice"
                        summary={$_('offers.forgetSummary', { default: 'Forgetting removes it from this device only.' })}
                    >
                        {$_('offers.forgetNote', {
                            default: 'A seller who already has the code can complete it until it expires.',
                        })}
                    </Explain>
                </div>
            {/each}
        </div>
    </div>
{/if}

<style>
    .open-offers {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin: 8px 0 0;
        font-weight: 500;
    }
    .list {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .offer {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-left: 4px solid var(--mdc-theme-primary);
        border-radius: 8px;
        padding: 12px 16px;
        background: var(--mdc-theme-surface, #fff);
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    :global(.dark-theme) .offer {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }
    .line {
        font-weight: 600;
    }
    .actions {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }
    .code {
        display: flex;
        justify-content: center;
        padding: 8px 0;
    }
</style>
