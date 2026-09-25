<script lang="ts">
    /**
     * Where a seller takes in a buyer's code: the camera, or the code's text
     * pasted in. Each code is parsed and checked here (`lib/offer.ts`) before
     * it is shown for review, so what the card renders is what the buyer
     * signed; a code that fails says why and is dropped.
     */
    import { _ } from 'svelte-i18n';
    import Explain from './Explain.svelte';
    import Button, { Label } from '@smui/button';
    import Textfield from '@smui/textfield';

    import QrScanner from './QrScanner.svelte';
    import ScannedOfferCard from './ScannedOfferCard.svelte';
    import { addScannedOffer, parseOffer, scannedOffers, verifyOffer, type OfferProblem } from '../lib/offer';
    import { networkView } from '../lib/node';
    import { currentActorId, heldSeed } from '../lib/actors';
    import { chainIdOrReport } from '../lib/submit';
    import { bytesToHex, derivePublicKey } from '../lib/crypto';
    import { errorStore } from '../common/errorStore';

    /** Buttons only, for a corner of another screen; the intro stays on Requests. */
    export let compact = false;

    let scanning = false;
    let pasting = false;
    let text = '';

    $: me = $currentActorId;

    function problemText(p: OfferProblem): string {
        switch (p) {
            case 'chain':
                return $_('offers.chain', { default: 'This code was signed for a different network.' });
            case 'signature':
                return $_('offers.signature', {
                    default: 'The signature on this code does not match what it says. Nothing was signed.',
                });
            case 'not-for-me':
                return $_('offers.notForMe', { default: 'This code names a different seller.' });
            case 'expired':
                return $_('offers.expired', { default: 'This code has expired. Ask the buyer for a new one.' });
        }
    }

    function take(raw: string) {
        const offer = parseOffer(raw);
        if (!offer) {
            errorStore.pushError($_('offers.badCode', { default: 'That is not a buyer’s code.' }));
            return;
        }
        const chainId = chainIdOrReport();
        if (chainId === null) return;
        const seed = me === null ? null : heldSeed(me);
        const keyHex = seed ? bytesToHex(Uint8Array.from(derivePublicKey(seed))) : '';
        const r = verifyOffer(offer, { chainId, epoch: $networkView?.epoch ?? 0, me: { member: me, keyHex } });
        if (!r.ok) {
            errorStore.pushError(problemText(r.problem));
            return;
        }
        addScannedOffer(r.verified);
        text = '';
        pasting = false;
    }
</script>

<div class="intake" class:compact>
    {#if !compact}
        <Explain summary={$_('offers.introSummary', { default: 'Somebody with no account yet is buying from you.' })}>
            {$_('offers.intro', {
                default:
                    'Their purchase arrives here when they scanned your QR, or as a code they show you. It is already signed; your signature creates their account and books the debt at once.',
            })}
        </Explain>
    {/if}
    <div class="buttons">
        <Button variant={compact ? 'outlined' : 'raised'} disabled={me === null} on:click={() => (scanning = true)}>
            <Label>{$_('offers.scan', { default: 'Scan a buyer’s code' })}</Label>
        </Button>
        <Button variant="outlined" disabled={me === null} on:click={() => (pasting = !pasting)}>
            <Label>{$_('offers.pasteToggle', { default: 'Paste a code instead' })}</Label>
        </Button>
    </div>
    {#if pasting}
        <div class="paste">
            <Textfield textarea label={$_('offers.pasteLabel', { default: 'Buyer’s code' })} bind:value={text} style="width: 100%;" />
            <Button variant="raised" disabled={!text.trim()} on:click={() => take(text)}>
                <Label>{$_('offers.add', { default: 'Add' })}</Label>
            </Button>
        </div>
    {/if}
    {#each $scannedOffers as v (v.digestHex)}
        <ScannedOfferCard verified={v} />
    {/each}
</div>

{#if scanning}
    <QrScanner
        title={$_('offers.scan', { default: 'Scan a buyer’s code' })}
        on:scanned={(e) => {
            scanning = false;
            take(e.detail);
        }}
        on:close={() => (scanning = false)}
    />
{/if}

<style>
    .intake {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .buttons {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }
    .paste {
        display: flex;
        flex-direction: column;
        gap: 8px;
        align-items: flex-start;
    }
</style>
