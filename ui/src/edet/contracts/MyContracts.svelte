<script lang="ts">
    import { _ } from 'svelte-i18n';
    import Button, { Label } from '@smui/button';

    import ContractCard from './ContractCard.svelte';
    import NumericInput from '../../common/NumericInput.svelte';
    import MemberChip from '../../components/MemberChip.svelte';
    import AddressInput from '../../components/AddressInput.svelte';
    import Explain from '../../components/Explain.svelte';
    import { activeBase, contractsList, networkView, pendingView } from '../../lib/node';
    import { currentActorId } from '../../lib/actors';
    import { tx, type ContractView } from '../../lib/api';
    import { copyText } from '../../lib/display';
    import { epochDate } from '../../lib/epoch';
    import { extensionFloor, extensionTerms, pendingExtensionOf } from '../../lib/extension';
    import { errorStore } from '../../common/errorStore';
    import { send } from '../../lib/submit';

    let showClosed = false;
    // The only manual action left: renegotiating a maturity. Paying back
    // happens by selling — mutual debts net inside the sale itself.
    let extending: number | null = null;
    let maturityStr = '';
    let busy = false;

    $: me = $currentActorId;
    $: epoch = $networkView?.epoch ?? 0;
    // The other bound on an extension: past `accepted_epoch + horizon` an
    // insured debt keeps its amount and drops its insurance, on the same two
    // signatures, so the page says so before the debtor asks.
    $: insuredHorizon = $networkView?.insured_horizon_epochs;
    $: mine = me === null ? [] : $contractsList.filter((c) => c.debtor === me).sort((a, b) => b.id - a.id);
    // The maturity the open form holds, read as a number once so the card's
    // label, the floor and the hint below the field all follow one value.
    $: candidate = Number.parseInt(maturityStr, 10);
    // An extension already parked for this contract: the card says so, and
    // the button waits, since a second request would only sit beside it.
    $: parkedOf = (c: ContractView) => pendingExtensionOf($pendingView, c.id);
    $: open = mine.filter((c) => c.status === 'active' || c.status === 'expired');
    $: closed = mine.filter((c) => c.status !== 'active' && c.status !== 'expired');

    // Handing a debt on. The old debtor discharges and the successor is booked
    // against the new debtor's own standing — which is also how a member of two
    // communities passes a debt to somebody the other one backs, needing no
    // mechanism beyond this one.
    let handing: number | null = null;
    let newDebtor: number | null = null;

    function beginHand(c: ContractView) {
        handing = c.id;
        newDebtor = null;
    }

    async function runHand(c: ContractView) {
        if (newDebtor === null) return;
        busy = true;
        try {
            if (await send(tx.transfer(c, newDebtor))) handing = null;
        } finally {
            busy = false;
        }
    }

    function beginExtend(c: ContractView) {
        extending = c.id;
        maturityStr = String(
            Math.max(extensionFloor(c.maturity_epoch, epoch), epoch + ($networkView?.min_maturity_epochs ?? 1)),
        );
    }

    async function runExtend(c: ContractView) {
        busy = true;
        try {
            if (await send(tx.extend(c, Number(maturityStr) || 0))) extending = null;
        } finally {
            busy = false;
        }
    }
</script>

<div class="main-container flex-column">
    <Explain
        summary={$_('myContracts.summary', { default: 'Debts you owe. You pay them back by selling.' })}
    >
        {$_('myContracts.intro', {
            default:
                'When your creditor buys from you, the mutual debt settles by itself — and any other sale clears your debts oldest-first. Nothing to file; if you need more time, extend the maturity together with your creditor.',
        })}
    </Explain>

    {#if open.length === 0}
        <p class="empty">{$_('myContracts.empty', { default: 'You owe nothing right now.' })}</p>
    {/if}

    <div class="list">
        {#each open as c (c.id)}
            <ContractCard
                contract={c}
                maturityPreview={extending === c.id && Number.isInteger(candidate) ? candidate : null}
                pendingExtension={parkedOf(c)}
            >
                <p class="payback-hint" class:cure={c.status === 'expired'}>
                    {#if c.status === 'expired'}
                        {$_('myContracts.cureHint', {
                            default: 'To cure this default, sell to your creditor — their purchase discharges it and restores your capacity.',
                        })}
                    {:else}
                        {$_('myContracts.paybackHint', {
                            default: 'Sell to your creditor and this settles by itself; any other sale clears it oldest-first.',
                        })}
                    {/if}
                    {#if c.creditor !== undefined}
                        <MemberChip memberId={c.creditor} size={18} />
                    {/if}
                </p>
                <div class="actions">
                    {#if c.status === 'active'}
                        <Button variant="outlined" disabled={parkedOf(c) !== null} on:click={() => beginExtend(c)}>
                            <Label>{$_('myContracts.extend', { default: 'Extend' })}</Label>
                        </Button>
                        <Button variant="outlined" on:click={() => beginHand(c)}>
                            <Label>{$_('myContracts.handOver', { default: 'Pass it on' })}</Label>
                        </Button>
                    {/if}
                </div>

                {#if c.status === 'active'}
                    <p class="hint deadline">
                        {$_('myContracts.handOverDeadline', {
                            values: { date: $epochDate(c.maturity_epoch) },
                            default: `Handing it on is open until ${$epochDate(c.maturity_epoch)}, and closes when it matures — after that only a cure your creditor also signs will clear it.`,
                        })}
                    </p>
                {/if}

                {#if handing === c.id}
                    <div class="action-form">
                        <Explain
                            summary={$_('myContracts.handOverSummary', {
                                default: 'Somebody else takes this debt over.',
                            })}
                        >
                            {$_('myContracts.handOverHint', {
                                default:
                                    'They must agree, and so must your creditor unless the community can back them for it — the claim moves, so it is theirs to consent to.',
                            })}
                        </Explain>
                        <p class="hint">
                            {$_('myContracts.handOverCarries', {
                                default:
                                    'What they take on is the whole of it: if it goes unpaid the default is recorded against them, not you, and the capacity it holds is theirs to lose.',
                            })}
                        </p>
                        <AddressInput
                            bind:value={newDebtor}
                            label={$_('myContracts.newDebtor', { default: 'Who takes it over' })}
                            exclude={[...(me === null ? [] : [me]), ...(c.creditor === undefined ? [] : [c.creditor])]}
                        />
                        <div class="action-buttons">
                            <Button variant="raised" disabled={busy || newDebtor === null} on:click={() => runHand(c)}>
                                <Label>{$_('common.confirm', { default: 'Confirm' })}</Label>
                            </Button>
                            <Button variant="outlined" on:click={() => (handing = null)}>
                                <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
                            </Button>
                        </div>
                    </div>
                {/if}


                {#if extending === c.id}
                    <div class="action-form">
                        <NumericInput
                            integer
                            min={extensionFloor(c.maturity_epoch, epoch)}
                            bind:value={maturityStr}
                            label={$_('myContracts.newMaturity', { default: 'New maturity epoch' })}
                        />
                        {#if extensionTerms(candidate, c.maturity_epoch, epoch).valid}
                            <p class="hint">
                                {$_('myContracts.extendPreview', {
                                    values: { left: candidate - epoch, added: candidate - c.maturity_epoch },
                                    default: `Ends ${candidate - epoch} epochs from now, ${candidate - c.maturity_epoch} later than it does today. Your creditor must sign this too; the due date above changes once they have.`,
                                })}
                            </p>
                        {/if}
                        {#if c.insured && insuredHorizon !== undefined && (Number(maturityStr) || 0) > c.accepted_epoch + insuredHorizon}
                            <Explain
                                tone="notice"
                                icon="info"
                                summary={$_('myContracts.extendPastHorizonSummary', {
                                    values: { date: $epochDate(c.accepted_epoch + insuredHorizon) },
                                    default: `After ${$epochDate(c.accepted_epoch + insuredHorizon)} the community stops standing behind this debt.`,
                                })}
                            >
                                {$_('myContracts.extendPastHorizon', {
                                    default:
                                        'Extending past it keeps the debt and drops the insurance: nothing stays reserved, and your creditor bears it alone from then on. Your creditor signs this too.',
                                })}
                            </Explain>
                        {/if}
                        <div class="action-buttons">
                            <Button variant="raised" disabled={busy} on:click={() => runExtend(c)}>
                                <Label>{$_('common.confirm', { default: 'Confirm' })}</Label>
                            </Button>
                            <Button variant="outlined" on:click={() => (extending = null)}>
                                <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
                            </Button>
                        </div>
                    </div>
                {/if}
            </ContractCard>
        {/each}
    </div>

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
    }
    .hint { font-size: 0.85rem; opacity: 0.8; margin: 0 0 0.5rem;
    }
    .deadline {
        margin: 0;
        font-size: 0.78rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        opacity: 1;
    }
    .action-form {
        border-top: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
        padding-top: 12px;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .payback-hint {
        margin: 0;
        font-size: 0.82rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        line-height: 1.45;
        display: flex;
        align-items: center;
        gap: 6px;
        flex-wrap: wrap;
    }
    .payback-hint.cure {
        color: #d32f2f;
    }
    :global(.dark-theme) .payback-hint.cure {
        color: #e57373;
    }
    .action-buttons {
        display: flex;
        gap: 8px;
    }
    .empty {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
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
