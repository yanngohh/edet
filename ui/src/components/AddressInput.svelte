<script lang="ts">
    /**
     * Counterparty picker, production-shaped: type/paste a wallet address
     * (or raw public key) or scan its QR — never a member dropdown. The
     * input resolves against the node (`/whois`) and exposes the member id
     * through `value`; a chip confirms who it resolved to.
     *
     * With `newcomers`, a PUBLIC KEY the ledger does not know is not an error
     * but a party in its own right (`{ Key }` on `party`): an
     * account is seated by the first bonded trade that names its key, so a key
     * `/whois` answers `null` for is somebody who is about to exist — and the
     * only screens that should offer it are the ones whose transaction can
     * actually seat them. An ADDRESS is never treated this way: it is a hash
     * of a key, so it cannot be signed with, and a stray one would silently
     * become an unsignable transaction.
     */
    import { _ } from 'svelte-i18n';
    import { createEventDispatcher } from 'svelte';
    import Textfield from '@smui/textfield';
    import IconButton from '@smui/icon-button';

    import QrScanner from './QrScanner.svelte';
    import MemberChip from './MemberChip.svelte';
    import { classifyAddressInput, resolveCounterparty } from '../lib/resolve';
    import { parsePayInput, type Invite } from '../lib/invite';
    import { hexToBytes } from '../lib/crypto';
    import * as api from '../lib/api';

    /** Resolved member id (null until a valid, known address is entered). */
    export let value: number | null = null;
    /**
     * The same answer as a PARTY: `{ Member }` for a member, `{ Key }` for a
     * newcomer when `newcomers` is on. Callers that only ever name members can
     * keep reading `value` and ignore this.
     */
    export let party: api.Party | null = null;
    export let label: string = '';
    /** Member ids this input refuses (e.g. yourself). */
    export let exclude: number[] = [];
    /** Accept an unknown public key as a party the transaction will seat. */
    export let newcomers = false;
    /**
     * Accept a raw public key as the party WITHOUT looking it up. A device
     * with no seated identity cannot ask the node whose key it is, and the
     * transaction names the holder by key exactly as well: nothing is claimed
     * about the key here, and the holder's own signature is what completes
     * the trade.
     */
    export let byKey = false;
    /**
     * The seller's invitation, when the input was their "pay me" QR rather
     * than a bare address (`lib/invite.ts`). Only a buyer with no account
     * uses it; everyone else reads `value` and ignores this.
     */
    export let invite: Invite | null = null;

    /**
     * Emitted once, when a SCANNED code resolves to a member — never when one
     * is typed or pasted. A scan is a completed choice: the member pointed a
     * camera at one person and it decoded. Typing is not, since every
     * intermediate string resolves too.
     */
    const dispatch = createEventDispatcher<{ scanned: number }>();

    let text = '';
    let scanning = false;
    /**
     * The exact string the scanner handed over, held until its own resolve
     * lands. Comparing the raw input against it is what separates "this
     * resolve came from the camera" from "the member typed the last character
     * of the same address" — `resolve` rewrites its argument on the way
     * through, so the comparison has to be against the untouched one.
     */
    let scannedText: string | null = null;
    let state: 'empty' | 'resolving' | 'resolved' | 'newcomer' | 'byKey' | 'unknown' | 'invalid' | 'excluded' = 'empty';
    let resolveSeq = 0;

    async function resolve(input: string) {
        const seq = ++resolveSeq;
        const raw = input;
        // A "pay me" QR names an address and may carry an invitation and the
        // seller's key; the address is what resolves, the rest rides along.
        const pay = parsePayInput(input);
        invite = pay?.invite ?? null;
        const payKeyHex = pay?.invite?.keyHex ?? null;
        if (pay) input = pay.address;
        const kind = classifyAddressInput(input).kind;
        if (!input.trim()) {
            state = 'empty';
            value = null;
            party = null;
            return;
        }
        if (kind === 'invalid') {
            state = 'invalid';
            value = null;
            party = null;
            return;
        }
        if (byKey && kind === 'pubkey') {
            state = 'byKey';
            value = null;
            party = api.asKey(Array.from(hexToBytes(classifyAddressInput(input).clean)));
            return;
        }
        state = 'resolving';
        const r = await resolveCounterparty(input);
        if (seq !== resolveSeq) return; // superseded by newer input
        if (!r && byKey && payKeyHex) {
            // The seller's QR carried their key, and this device cannot look
            // the address up: the trade names them by key, and the node
            // resolves it to their row.
            state = 'byKey';
            value = null;
            party = api.asKey(Array.from(hexToBytes(payKeyHex)));
        } else if (!r) {
            // A key nobody holds: a party this trade would seat, where the
            // screen can do that, and an unusable address everywhere else.
            const key = newcomers && kind === 'pubkey' ? classifyAddressInput(input).clean : null;
            state = key ? 'newcomer' : 'unknown';
            value = null;
            party = key ? api.asKey(Array.from(hexToBytes(key))) : null;
        } else if (exclude.includes(r.member)) {
            state = 'excluded';
            value = null;
            party = null;
        } else {
            state = 'resolved';
            value = r.member;
            party = api.asMember(r.member);
            if (scannedText !== null && raw.trim() === scannedText) {
                scannedText = null;
                dispatch('scanned', r.member);
            }
        }
    }

    $: void resolve(text);

    function onScanned(e: CustomEvent<string>) {
        scanning = false;
        scannedText = e.detail.trim();
        text = scannedText;
    }

    /**
     * Empty the field, for a caller that has CONSUMED what it resolved to —
     * a beneficiary added to a list, say. Clearing the text re-runs `resolve`,
     * which puts `value`, `party` and `invite` back to their empty state
     * together; nulling them from outside would leave the box still showing
     * an address that has already gone somewhere else.
     */
    export function clear(): void {
        text = '';
    }
</script>

<div class="address-input">
    <div class="address-row">
        <Textfield
            {label}
            bind:value={text}
            style="flex: 1; min-width: 260px;"
            input$spellcheck="false"
            input$autocapitalize="off"
        />
        <IconButton
            class="material-icons"
            aria-label={$_('addressInput.scan', { default: 'Scan QR code' })}
            title={$_('addressInput.scan', { default: 'Scan QR code' })}
            on:click={() => (scanning = true)}
        >
            qr_code_scanner
        </IconButton>
    </div>
    {#if state === 'resolved' && value !== null}
        <span class="resolved">
            <i class="material-icons ok" aria-hidden="true">check_circle</i>
            <MemberChip memberId={value} size={20} />
        </span>
    {:else if state === 'newcomer'}
        <span class="hint new">
            {$_('addressInput.newcomer', {
                default:
                    'Nobody on this ledger holds that key yet — recording this trade is what creates their account. They sign it from their own app.',
            })}
        </span>
    {:else if state === 'byKey'}
        <span class="hint new">
            {$_('addressInput.byKey', {
                default: "Named by this key. It must be a member's, and they sign from their own app.",
            })}
        </span>
    {:else if state === 'resolving'}
        <span class="hint">{$_('addressInput.resolving', { default: 'Looking up…' })}</span>
    {:else if state === 'unknown'}
        <span class="hint bad">{$_('addressInput.unknown', { default: 'No member with this address on your ledger.' })}</span>
    {:else if state === 'invalid'}
        <span class="hint bad">{$_('addressInput.invalid', { default: 'Not an address (0x + 40 hex) or public key (64 hex).' })}</span>
    {:else if state === 'excluded'}
        <span class="hint bad">{$_('addressInput.excluded', { default: 'That address cannot be the counterparty here.' })}</span>
    {/if}
</div>

{#if scanning}
    <QrScanner on:scanned={onScanned} on:close={() => (scanning = false)} />
{/if}

<style>
    .address-input {
        display: flex;
        flex-direction: column;
        gap: 4px;
        min-width: 0;
    }
    .address-row {
        display: flex;
        align-items: flex-end;
        gap: 2px;
    }
    .resolved {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-size: 0.9rem;
    }
    .resolved .ok {
        color: #2e7d32;
        font-size: 18px;
    }
    .hint {
        font-size: 0.8rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }
    .hint.bad {
        color: var(--mdc-theme-error, #d32f2f);
    }
    .hint.new {
        color: var(--mdc-theme-primary, #1565c0);
    }
</style>
