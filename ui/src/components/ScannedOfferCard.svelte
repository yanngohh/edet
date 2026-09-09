<script lang="ts">
    /**
     * A buyer's code under review: the purchase it carries, who it comes
     * from, what signing it does, and whether the ledger would take it now.
     * The buyer's signature was checked before this card existed; the
     * seller's is what the one button adds.
     */
    import { _ } from 'svelte-i18n';
    import Explain from './Explain.svelte';
    import { onMount } from 'svelte';
    import Button, { Label } from '@smui/button';

    import MemberChip from './MemberChip.svelte';
    import { membersList, myMember } from '../lib/node';
    import { memberName } from '../lib/display';
    import { epochDate } from '../lib/epoch';
    import { txSummary } from '../lib/txSummary';
    import { formatNumber } from '../common/functions';
    import { acceptOffer, checkOffer, rejectionMessage } from '../lib/submit';
    import { dropScannedOffer, type VerifiedOffer } from '../lib/offer';

    export let verified: VerifiedOffer;

    let busy = false;
    let check: { ok: boolean; code?: string } | null = null;

    $: summary = txSummary(verified.tx as Record<string, any>, {
        t: $_,
        nameOf: $memberName,
        fmt: (n, d) => formatNumber(n, d ?? 2),
        dateOf: $epochDate,
    });
    $: buyerHex = verified.offer.buyerKeyHex;
    $: short = `${buyerHex.slice(0, 8)}…`;
    // A code composed before the buyer's account existed may arrive after it
    // does — their first trade landed from somebody else meanwhile. Then this
    // is an ordinary purchase, and the card says so instead of promising a
    // seat.
    $: buyerId = $membersList.find((m) => m.keys?.includes(buyerHex))?.id ?? null;
    $: bondPos = $myMember?.operation_bond;
    $: seatsLeft = bondPos && bondPos.unit > 0 ? Math.floor(bondPos.seat_reach / bondPos.unit) : null;

    onMount(() => {
        void checkOffer(verified).then((r) => (check = r));
    });

    async function sign() {
        busy = true;
        try {
            await acceptOffer(verified);
        } finally {
            busy = false;
        }
    }
</script>

<div class="offer-card">
    <div class="head">
        <span class="summary">{summary}</span>
    </div>
    <div class="meta">
        <span class="from">
            {$_('requests.from', { default: 'from' })}
            {#if buyerId !== null}
                <MemberChip memberId={buyerId} size={20} />
            {:else}
                {$_('requests.newcomer', { values: { key: short }, default: `a new account (${short})` })}
            {/if}
        </span>
        <span class="digest mono">{verified.digestHex.slice(0, 12)}…</span>
    </div>
    <!-- Two instances rather than one with a conditional body: a member's
         case has nothing to fold, and an expander over an empty detail is a
         control that does nothing. -->
    {#if buyerId !== null}
        <Explain
            tone="notice"
            icon="info"
            summary={$_('offers.member', { default: 'They are a member already, so this books the debt like any other purchase.' })}
        />
    {:else}
        <Explain
            tone="notice"
            icon="info"
            summary={$_('offers.newAccountSummary', {
                default: 'This trade creates their account, and if they do not pay the debt is yours alone.',
            })}
        >
            {$_('offers.newAccount', {
                default:
                    'It holds one of the newcomers you can bring in for as long as that account exists. Nobody has backed them yet, so nothing binds them but the arbitration they named, if any.',
            })}
            {#if seatsLeft !== null}
                {$_('offers.seatsLeft', {
                    values: { count: seatsLeft },
                    default: `Newcomers you can bring in: ${seatsLeft}.`,
                })}
            {/if}
        </Explain>
    {/if}
    {#if check && !check.ok}
        <p class="invalid" role="alert">
            {$_('requests.invalidNow', {
                values: { reason: rejectionMessage(check.code ?? 'ET-UNKNOWN') },
                default: `As things stand this would be rejected: ${rejectionMessage(check.code ?? 'ET-UNKNOWN')}`,
            })}
        </p>
    {/if}
    <div class="actions">
        <Button variant="raised" disabled={busy} on:click={sign}>
            <Label>{$_('offers.sign', { default: 'Sign and book it' })}</Label>
        </Button>
        <Button variant="outlined" disabled={busy} on:click={() => dropScannedOffer(verified.digestHex)}>
            <Label>{$_('offers.discard', { default: 'Discard' })}</Label>
        </Button>
    </div>
</div>

<style>
    .offer-card {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-left: 4px solid #1565c0;
        border-radius: 8px;
        padding: 12px 16px;
        background: var(--mdc-theme-surface, #fff);
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    :global(.dark-theme) .offer-card {
        background: #1e1e1e;
        border-top-color: rgba(255, 255, 255, 0.1);
        border-right-color: rgba(255, 255, 255, 0.1);
        border-bottom-color: rgba(255, 255, 255, 0.1);
        border-left-color: #64b5f6;
    }
    .summary {
        font-weight: 600;
    }
    .meta {
        display: flex;
        align-items: center;
        gap: 12px;
        flex-wrap: wrap;
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .from {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .mono {
        font-family: monospace;
        margin-left: auto;
    }
    .invalid {
        margin: 0;
        font-size: 0.85rem;
        color: var(--mdc-theme-error, #d32f2f);
    }
    .actions {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }
</style>
