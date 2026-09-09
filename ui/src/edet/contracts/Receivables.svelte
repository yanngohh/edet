<script lang="ts">
    import { _ } from 'svelte-i18n';
    import Button, { Label } from '@smui/button';

    import ContractCard from './ContractCard.svelte';
    import Explain from '../../components/Explain.svelte';
    import NumericInput from '../../common/NumericInput.svelte';
    import { contractsList, networkView, pendingView } from '../../lib/node';
    import { currentActorId } from '../../lib/actors';
    import { tx, type ContractView } from '../../lib/api';
    import { pendingExtensionOf } from '../../lib/extension';
    import { send } from '../../lib/submit';
    import { formatNumber, parseNumber } from '../../common/functions';

    let showClosed = false;
    let attestFor: number | null = null;
    let attestAmountStr = '';
    let busy = false;

    $: me = $currentActorId;
    $: epoch = $networkView?.epoch ?? 0;
    $: owedToMe = me === null ? [] : $contractsList.filter((c) => c.creditor === me).sort((a, b) => b.id - a.id);
    $: open = owedToMe.filter((c) => c.status === 'active' || c.status === 'expired');
    $: closed = owedToMe.filter((c) => c.status !== 'active' && c.status !== 'expired');
    // Panel duties: any contract whose consented panel names me and whose
    // debtor has defaulted (attestations mint the award once quorum concurs).
    $: panelDuties =
        me === null
            ? []
            : $contractsList.filter(
                  (c) => c.arb && c.arb.arbiters.includes(me) && c.status === 'expired' && !c.arb_awarded,
              );

    function isOverdue(c: ContractView): boolean {
        return c.status === 'active' && epoch > c.maturity_epoch;
    }

    async function markExpired(c: ContractView) {
        busy = true;
        try {
            await send(tx.markExpired(c));
        } finally {
            busy = false;
        }
    }

    async function attest(c: ContractView) {
        if (me === null) return;
        busy = true;
        try {
            if (await send(tx.arbAttest(c, me, parseNumber(attestAmountStr)))) {
                attestFor = null;
            }
        } finally {
            busy = false;
        }
    }
</script>

<div class="main-container flex-column">
    <Explain summary={$_('receivables.summary', { default: 'Debts owed to you. Past its maturity, you may mark one as defaulted.' })}>
        {$_('receivables.intro', {
            default:
                'Marking a default protects the community by making the risk visible, and it unlocks the cure path and arbitration.',
        })}
    </Explain>

    {#if open.length === 0}
        <p class="empty">{$_('receivables.empty', { default: 'Nobody owes you right now — sell something and have the buyer record the purchase.' })}</p>
    {/if}

    <div class="list">
        {#each open as c (c.id)}
            <ContractCard contract={c} pendingExtension={pendingExtensionOf($pendingView, c.id)}>
                <div class="actions">
                    {#if isOverdue(c)}
                        <Button variant="raised" disabled={busy} on:click={() => markExpired(c)}>
                            <Label>{$_('receivables.markExpired', { default: 'Mark as defaulted' })}</Label>
                        </Button>
                    {/if}
                </div>
                {#if c.status === 'expired'}
                    <Explain
                        tone="notice"
                        icon="info"
                        summary={$_('receivables.expiredSummary', { default: 'Defaulted — buying from the debtor cures it.' })}
                    >
                        {$_('receivables.expiredNote', {
                            default: 'Your purchase discharges the debt and repairs both sides; try that before anything else.',
                        })}
                    </Explain>
                {/if}
            </ContractCard>
        {/each}
    </div>

    {#if panelDuties.length > 0}
        <h3 class="section-title">{$_('receivables.panelTitle', { default: 'Panel duties — you are an arbiter' })}</h3>
        <Explain summary={$_('receivables.panelSummary', { default: 'These defaulted contracts name you on their arbitration panel.' })}>
            {$_('receivables.panelIntro', {
                default:
                    'Both parties consented to the panel. Attest the amount you find justified; when the quorum concurs, the median (capped) mints for the creditor.',
            })}
        </Explain>
        <div class="list">
            {#each panelDuties as c (c.id)}
                <ContractCard contract={c}>
                    <div class="actions">
                        {#if me !== null && (c.arb_attested ?? []).includes(me)}
                            <span class="attested">{$_('receivables.attested', { default: 'You have attested.' })}</span>
                        {:else if attestFor === c.id}
                            <div class="attest-form">
                                <NumericInput bind:value={attestAmountStr} label={$_('receivables.attestAmount', { default: 'Justified amount' })} />
                                <Button variant="raised" disabled={busy} on:click={() => attest(c)}>
                                    <Label>{$_('receivables.attest', { default: 'Attest' })}</Label>
                                </Button>
                                <Button variant="outlined" on:click={() => (attestFor = null)}>
                                    <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
                                </Button>
                            </div>
                        {:else}
                            <Button
                                variant="outlined"
                                on:click={() => {
                                    attestFor = c.id;
                                    attestAmountStr = formatNumber(c.outstanding);
                                }}
                            >
                                <Label>{$_('receivables.attestOpen', { default: 'Review & attest' })}</Label>
                            </Button>
                        {/if}
                    </div>
                </ContractCard>
            {/each}
        </div>
    {/if}

    {#if closed.length > 0}
        <button class="toggle-closed" type="button" on:click={() => (showClosed = !showClosed)}>
            {showClosed
                ? $_('myContracts.hideClosed', { default: 'Hide closed contracts' })
                : $_('myContracts.showClosed', { values: { count: closed.length }, default: `Show ${closed.length} closed contracts` })}
        </button>
        {#if showClosed}
            <div class="list">
                {#each closed as c (c.id)}
                    <ContractCard contract={c} />
                {/each}
            </div>
        {/if}
    {/if}
</div>

<style>
    .main-container {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
        gap: 14px;
        max-width: var(--edet-column);
    }
    .list {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .actions {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
        align-items: center;
    }
    .attest-form {
        display: flex;
        gap: 8px;
        align-items: flex-end;
        flex-wrap: wrap;
    }
    .attested {
        font-size: 0.85rem;
        color: #2e7d32;
    }
    .empty {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin: 8px 0 0;
        font-weight: 500;
    }
    .toggle-closed {
        background: none;
        border: none;
        color: var(--mdc-theme-primary);
        cursor: pointer;
        font: inherit;
        text-align: left;
        padding: 4px 0;
    }
</style>
