<script lang="ts">
    /**
     * The wallet of a key that exists and has never traded.
     *
     * **This screen is the whole of what "not seated yet" means, and it is a
     * wallet rather than a notice.** The device holds a key; the ledger holds
     * no row for it, because a row is seated by the first bonded trade that
     * names the key (§Medium). What is true meanwhile is that the quantities
     * are zero and the key is ready to trade, so those are what it shows.
     *
     * The first trade can start on either side. Buying: the holder records a
     * purchase under Transactions and shows the seller the code it produces
     * (`lib/offer.ts`), since a key cannot open a pool entry. Selling: a
     * member records a purchase naming this key, and the pool holds that for
     * the key to co-sign — which is the one offer this screen reviews, read
     * off the poll loop by key (`node.ts::pollKeyed`), the same loop that
     * notices the seat land and adopts the identity wherever the app is open.
     *
     * It carries the same `data-tour` anchors as the seated wallet, because
     * the tour runs immediately after key creation and is the reason this
     * screen exists at all: a tour of an empty-state message teaches nothing.
     */
    import { _ } from 'svelte-i18n';
    import { get } from 'svelte/store';
    import Button, { Label } from '@smui/button';
    import CircularProgress from '@smui/circular-progress';

    import QrCodeDisplay from '../../components/QrCodeDisplay.svelte';
    import StatCard from '../../components/StatCard.svelte';
    import Explain from '../../components/Explain.svelte';
    import { networkView, pendingView, refreshSoon } from '../../lib/node';
    import { pendingIdentity } from '../../lib/actors';
    import { declinePendingAsKey, signPendingAsKey } from '../../lib/submit';
    import { buildAdmitQrPayload } from '../../lib/invite';
    import { amountsSealed, copyText, formatAmount } from '../../lib/display';
    import { formatNumber } from '../../common/functions';
    import { bytesToHex, derivePublicKey } from '../../lib/crypto';
    import { errorStore } from '../../common/errorStore';

    $: keyHex = $pendingIdentity ? bytesToHex(Uint8Array.from(derivePublicKey($pendingIdentity))) : '';
    /** A trade a member opened naming this key, waiting for its signature. */
    $: offered = $pendingView?.awaiting_me?.[0] ?? null;
    /**
     * Which side of the offered trade this key is on, and for how much.
     *
     * **"A member recorded a trade with your key" does not say what signing
     * costs**, and the two answers are opposites: as the debtor you end up
     * owing the amount, as the creditor you end up owed it. The raw
     * transaction says so exactly and says it in JSON, which is not a reading
     * anybody should be asked to do before accepting an obligation.
     */
    $: side = offered ? sideOf(offered.tx) : null;
    let signing = false;

    $: net = $networkView;
    // Same fallback the wizard used: a bare key link before the node has
    // named its chain, so the QR is never blank while the view loads.
    $: qrPayload = keyHex
        ? net?.chain_id
            ? buildAdmitQrPayload(net.chain_id, keyHex)
            : `edet://join?k=${keyHex}`
        : '';

    /**
     * `Accept` names debtor and creditor; `Sale` names seller and buyer, and
     * the buyer is the one who takes on the obligation. Anything else that
     * reaches this queue is left undecided rather than guessed at — a wrong
     * direction here is worse than none.
     */
    function sideOf(tx: Record<string, any>): { buying: boolean; amount: number } | null {
        const mine = (p: unknown) =>
            !!p && typeof p === 'object' && 'Key' in (p as object)
                ? bytesToHex(Uint8Array.from((p as { Key: number[] }).Key)) === keyHex
                : false;
        if (tx.Accept) {
            if (mine(tx.Accept.debtor)) return { buying: true, amount: tx.Accept.amount };
            if (mine(tx.Accept.creditor)) return { buying: false, amount: tx.Accept.amount };
        }
        if (tx.Sale) {
            if (mine(tx.Sale.buyer)) return { buying: true, amount: tx.Sale.amount };
            if (mine(tx.Sale.seller)) return { buying: false, amount: tx.Sale.amount };
        }
        return null;
    }

    async function sign() {
        const seed = get(pendingIdentity);
        if (!offered || !seed) return;
        signing = true;
        try {
            if (await signPendingAsKey(offered, seed)) refreshSoon();
        } finally {
            signing = false;
        }
    }

    async function decline() {
        const seed = get(pendingIdentity);
        if (!offered || !seed) return;
        signing = true;
        try {
            if (await declinePendingAsKey(offered, seed)) refreshSoon();
        } finally {
            signing = false;
        }
    }

    async function copyKey() {
        if (await copyText(keyHex)) {
            errorStore.pushError($_('common.copied', { default: 'Copied to clipboard' }), 'warning');
        }
    }
</script>

<div class="flex-column main-container">
    <div class="wallet-header" data-tour="wallet-header">
        <div class="key-glyph" aria-hidden="true">
            <i class="material-icons">vpn_key</i>
        </div>
        <div class="wallet-id flex-column">
            <span class="wallet-name">{$_('pendingWallet.title', { default: 'Your key' })}</span>
            <span class="wallet-address">
                <code>{keyHex.slice(0, 16)}…{keyHex.slice(-8)}</code>
                <button
                    type="button"
                    class="copy-btn"
                    aria-label={$_('common.copy', { default: 'Copy' })}
                    title={$_('common.copy', { default: 'Copy' })}
                    on:click={copyKey}
                >
                    <i class="material-icons" aria-hidden="true">content_copy</i>
                </button>
            </span>
            <span class="badges">
                <span class="pill pending">{$_('pendingWallet.pill', { default: 'no account yet' })}</span>
            </span>
        </div>
        <div class="wallet-qr" data-tour="wallet-qr">
            <QrCodeDisplay value={qrPayload} size={132} />
            <span class="qr-hint">{$_('pendingWallet.qrHint', { default: 'Selling? Have the buyer scan this.' })}</span>
        </div>
    </div>

    <Explain
        summary={$_('pendingWallet.summary', {
            default: 'Nobody admits you here — your first purchase is what creates your account.',
        })}
    >
        {$_('pendingWallet.body', {
            default:
                "Record it under Transactions: scanning the seller's QR sends it to their app, and a code you can show them is kept in case. Their signature seats you and books the debt at once. Selling first works too: the buyer scans this key and records the purchase, and it comes back here for your signature. An account that holds nothing for a year — no trade in it, nothing owed, nothing backing it — is retired, and your next trade seats a new one at the same address.",
        })}
    </Explain>

    {#if offered}
        <div class="offered">
            {#if side}
                <p class="explain direction">
                    {#if side.buying}
                        {$_('pendingWallet.buying', {
                            values: { amount: formatAmount(formatNumber, side.amount, $amountsSealed) },
                            default: `You are BUYING. Sign and you owe ${formatAmount(formatNumber, side.amount, $amountsSealed)}.`,
                        })}
                    {:else}
                        {$_('pendingWallet.selling', {
                            values: { amount: formatAmount(formatNumber, side.amount, $amountsSealed) },
                            default: `You are SELLING. Sign and they owe you ${formatAmount(formatNumber, side.amount, $amountsSealed)}.`,
                        })}
                    {/if}
                </p>
            {/if}
            <Explain
                tone="notice"
                icon="info"
                summary={$_('pendingWallet.offeredSummary', { default: 'A member recorded a trade with your key.' })}
            >
                {$_('pendingWallet.offeredBody', {
                    default:
                        'Signing it is what creates your account — read it first: it is an obligation, not a formality.',
                })}
            </Explain>
            <details>
                <summary>{$_('pendingWallet.raw', { default: 'The exact transaction' })}</summary>
                <pre class="offered-tx">{JSON.stringify(offered.tx, null, 2)}</pre>
            </details>
            <div class="actions-row">
                <Button variant="outlined" disabled={signing} on:click={decline}>
                    <Label>{$_('pendingWallet.decline', { default: 'No, refuse it' })}</Label>
                </Button>
                <Button variant="raised" disabled={signing} on:click={sign}>
                    <Label>{$_('pendingWallet.sign', { default: 'Sign it' })}</Label>
                </Button>
            </div>
        </div>
    {:else}
        <div class="awaiting">
            <CircularProgress class="circular-progress" indeterminate />
            <span>{$_('pendingWallet.awaiting', { default: 'No trade yet. Record a purchase under Transactions, or have a buyer scan your key.' })}</span>
        </div>
    {/if}

    <!--
        Zeroes, and they are the true figures rather than placeholders: a key
        with no incident stakes has a capacity of zero by definition, which is
        the same number the seated wallet would show on its first day.
    -->
    <div class="metrics-grid" data-tour="wallet-metrics" data-loaded="true">
        <StatCard
            label={$_('myWallet.capacity', { default: 'Capacity' })}
            value={formatAmount(formatNumber, 0, $amountsSealed)}
            icon="speed"
            help={$_('pendingWallet.capacityHelp', {
                default:
                    'What the community will let you owe. Zero until somebody backs you, and settling what you owe is what earns it.',
            })}
        />
        <StatCard
            label={$_('myWallet.debt', { default: 'You owe' })}
            value={formatAmount(formatNumber, 0, $amountsSealed)}
            icon="trending_down"
            help={$_('pendingWallet.debtHelp', { default: 'Nothing yet — this key has never traded.' })}
        />
    </div>
</div>

<style>
    .main-container {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
        gap: 16px;
        max-width: var(--edet-column);
    }
    .wallet-header {
        display: flex;
        align-items: flex-start;
        gap: 16px;
        flex-wrap: wrap;
    }
    .key-glyph {
        width: 64px;
        height: 64px;
        border-radius: 50%;
        display: flex;
        align-items: center;
        justify-content: center;
        background: var(--mdc-theme-surface, #eee);
        border: 1px dashed var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.3));
    }
    .wallet-id {
        gap: 4px;
        min-width: 0;
        flex: 1;
    }
    .wallet-name {
        font-size: 1.2rem;
        font-weight: 600;
    }
    .wallet-address {
        display: flex;
        align-items: center;
        gap: 6px;
        font-size: 0.85rem;
        opacity: 0.8;
        word-break: break-all;
    }
    .copy-btn {
        border: none;
        background: transparent;
        cursor: pointer;
        color: inherit;
        padding: 0;
        display: inline-flex;
    }
    .copy-btn .material-icons {
        font-size: 16px;
    }
    .badges {
        display: flex;
        gap: 6px;
    }
    .pill {
        font-size: 0.72rem;
        padding: 2px 8px;
        border-radius: 10px;
        background: rgba(0, 0, 0, 0.08);
    }
    .wallet-qr {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 6px;
    }
    .qr-hint {
        font-size: 0.75rem;
        opacity: 0.75;
        max-width: 160px;
        text-align: center;
    }
    .explain {
        margin: 0;
        font-size: 0.9rem;
        line-height: 1.5;
    }
    .awaiting {
        display: flex;
        align-items: center;
        gap: 10px;
        font-size: 0.9rem;
        opacity: 0.85;
    }
    .offered {
        display: flex;
        flex-direction: column;
        gap: 10px;
        padding: 12px;
        border: 1px solid var(--mdc-theme-primary, #6200ee);
        border-radius: 8px;
    }
    .direction {
        font-weight: 600;
    }
    details summary {
        cursor: pointer;
        font-size: 0.8rem;
        opacity: 0.8;
    }
    .offered-tx {
        margin: 0;
        padding: 8px;
        overflow-x: auto;
        font-size: 0.75rem;
        background: rgba(0, 0, 0, 0.05);
        border-radius: 4px;
    }
    .actions-row {
        display: flex;
        gap: 8px;
        justify-content: flex-end;
    }
    .metrics-grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
        gap: 12px;
    }
</style>
