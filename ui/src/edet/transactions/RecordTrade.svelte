<script lang="ts">
    /**
     * Recording a purchase (Transactions → FAB): the one way to trade.
     *
     * **Buying is taking on a debt to the seller, and that IS the payment.**
     * The seller's signature completes it, and where that signature is
     * collected depends only on who the buyer is. A member's purchase parks in
     * the node's pending pool and the seller's app finds it under Requests. A
     * key with no account yet cannot open a pool entry (keys are free, so the
     * pool's occupancy is paid by a member), so its purchase leaves this device
     * as a CODE shown to the seller (`lib/offer.ts`); their signature creates
     * the account and books the debt at once. Selling is being bought from:
     * the buyer records the purchase, the seller signs.
     *
     * A purchase FROM a key the ledger does not know seats the seller, a
     * purchase BY one seats the buyer, and both are this screen.
     *
     * Under the hood the cascade `Sale` is preferred (a trade with an indebted
     * seller automatically helps clear their debts); the plain `Accept` is the
     * fallback for pairs the cascade cannot serve yet, and the lane arbitration
     * forces — one user intent, protocol picks the lane. Neither is a tier: an
     * obligation is insured where the DEBTOR's capacity carries it and
     * uninsured where it does not, on both lanes alike.
     */
    import { _ } from 'svelte-i18n';
    import { createEventDispatcher } from 'svelte';
    import Button, { Label } from '@smui/button';
    import Checkbox from '@smui/checkbox';
    import FormField from '@smui/form-field';
    import Textfield from '@smui/textfield';

    import ActionPage from '../../components/ActionPage.svelte';
    import Explain from '../../components/Explain.svelte';
    import AddressInput from '../../components/AddressInput.svelte';
    import BondNotice from '../../components/BondNotice.svelte';
    import QrCodeDisplay from '../../components/QrCodeDisplay.svelte';
    import NumericInput from '../../common/NumericInput.svelte';
    import { membersList, myMember, networkView } from '../../lib/node';
    import { currentActorId, pendingIdentity } from '../../lib/actors';
    import { asKey, asMember, tx, type ArbTermsView, type Party } from '../../lib/api';
    import { counterpartyIsUnbacked, insuranceOutlook, tradeParties } from '../../lib/trade';
    import { composeOffer, openInvited, precheck, rejectionMessage, send } from '../../lib/submit';
    import { resolveCounterparty } from '../../lib/resolve';
    import { derivePublicKey } from '../../lib/crypto';
    import { copyText } from '../../lib/display';
    import { epochDate } from '../../lib/epoch';
    import { rememberOffer, type OpenOffer } from '../../lib/offer';
    import type { Invite } from '../../lib/invite';
    import { parseNumber } from '../../common/functions';
    import { errorStore } from '../../common/errorStore';

    const dispatch = createEventDispatcher<{ close: void }>();

    let other: number | null = null;
    /**
     * The counterparty as a PARTY. Usually `{ Member }`; `{ Key }` when they
     * have no account yet, in which case recording this trade is what seats
     * them — or when this device has none and names the seller by key. The
     * transaction always names this, never `other`.
     */
    let otherParty: Party | null = null;
    /** The seller's invitation, when their QR carried one. */
    let invite: Invite | null = null;
    let amountStr = '';
    let maturityStr = '';
    let submitting = false;
    /** The code a keyed buyer shows the seller, once the purchase is signed
     *  and the pool did not take it. */
    let offer: OpenOffer | null = null;
    /** The pool was tried on an invitation and refused; the code is plan B. */
    let fallback = false;

    // Optional arbitration terms (pins the panel at acceptance; forces the
    // plain-contract lane since Sale carries no arbitration).
    let withArb = false;
    let arbAddresses = '';
    let arbQuorumStr = '1';
    let arbWindowStr = '10';
    let arbCapStr = '';

    $: me = $currentActorId;
    $: pendingSeed = $pendingIdentity;
    /** A key with no account: the buyer this trade will seat. */
    $: keyed = me === null && pendingSeed !== null;
    /**
     * The visible line, whichever this device is. A key with no account
     * changes what happens NEXT — the seller's signature is what creates the
     * row — and that is material enough to stay on the line rather than fold
     * behind it with the mechanics.
     */
    $: buySummary = keyed
        ? $_('transactions.keyedSummary', {
              default:
                  'Buying is taking on a debt to the seller — and you have no account yet, so their signature creates one.',
          })
        : $_('transactions.purchaseSummary', {
              default: 'Buying is taking on a debt to the seller — that is the payment.',
          });
    $: meParty = me !== null ? asMember(me) : pendingSeed ? asKey(derivePublicKey(pendingSeed)) : null;
    $: dust = $networkView?.dust ?? 0.01;
    $: otherRow = other === null ? undefined : $membersList.find((m) => m.id === other);
    /**
     * **Insurance is the DEBTOR's capacity against the amount**, and nothing
     * else — the buyer's, which is this device's. A key nobody has backed has
     * a capacity of zero by arithmetic, which is the reading `lib/trade.ts`
     * gives the page rather than "unknown".
     */
    $: debtorCapacity = keyed ? 0 : $myMember?.capacity;
    $: outlook = insuranceOutlook(
        'buy',
        debtorCapacity,
        parseNumber(amountStr),
        Number(maturityStr) || undefined,
        insuredHorizon,
    );
    /**
     * A separate question, and only about arbitration: an agreed panel is the
     * only thing that binds somebody with no standing to an outcome. Not a mark
     * against them — every account starts at zero.
     */
    $: otherUnbacked = counterpartyIsUnbacked(otherRow?.capacity, dust);
    $: minMaturity = $networkView?.min_maturity_epochs ?? 1;
    // The other bound on the term: past it, capacity or not, the claim books
    // uninsured, and the page says so before the member signs.
    $: insuredHorizon = $networkView?.insured_horizon_epochs;
    // The bond notice prices the lane `submit` will actually take, so it names
    // the same parties from the same place.
    $: noticeParties = tradeParties('buy', meParty ?? asMember(0), otherParty ?? asMember(0));
    $: epoch = $networkView?.epoch ?? 0;
    $: if (maturityStr === '') maturityStr = String(minMaturity);

    async function resolveArbTerms(): Promise<ArbTermsView | null> {
        const lines = arbAddresses
            .split('\n')
            .map((l) => l.trim())
            .filter(Boolean);
        const ids: number[] = [];
        for (const line of lines) {
            const r = await resolveCounterparty(line);
            if (!r) {
                errorStore.pushError(
                    $_('transactions.arbUnknown', {
                        values: { address: line },
                        default: `Unknown arbiter address: ${line}`,
                    }),
                );
                return null;
            }
            ids.push(r.member);
        }
        return {
            arbiters: [...new Set(ids)],
            quorum: Math.max(1, Number(arbQuorumStr) || 1),
            window_epochs: Math.max(1, Number(arbWindowStr) || 1),
            award_cap: parseNumber(arbCapStr) || 0,
        };
    }

    async function submit() {
        if (meParty === null || otherParty === null) return;
        const amount = parseNumber(amountStr);
        if (!(amount > 0)) {
            errorStore.pushError($_('transactions.badAmount', { default: 'Enter a positive amount.' }));
            return;
        }
        const maturity = Math.max(minMaturity, Number(maturityStr) || minMaturity);
        submitting = true;
        try {
            // Who owes whom: the buyer is the debtor, and the buyer is this device.
            const { debtor, creditor, seller, buyer } = tradeParties('buy', meParty, otherParty);
            const arb = withArb ? await resolveArbTerms() : null;
            if (withArb && arb === null) return;
            if (keyed && pendingSeed) {
                // A key's half sits in the pool only on the seller's
                // invitation, which their QR carries; without one, or if the
                // pool refuses, the same signed envelope leaves as a code. No
                // dry run either way: the node answers a verdict only to a
                // party it can name, and the seller's device runs it, where
                // it is one.
                const made = await composeOffer(
                    { lane: withArb ? 'accept' : 'sale', seller, amount, maturityEpochs: maturity, arb },
                    pendingSeed,
                );
                if (!made) return;
                const sent = invite ? (await openInvited(made.offer, invite)) === 'sent' : false;
                rememberOffer({ ...made.open, sent });
                if (sent) {
                    errorStore.pushError(
                        $_('transactions.sentToSeller', {
                            default:
                                'Sent to the seller — it shows under their Requests. The code stays under Transactions in case they cannot see it.',
                        }),
                        'warning',
                    );
                    dispatch('close');
                    return;
                }
                fallback = invite !== null;
                offer = made.open;
                return;
            }
            let ok = false;
            if (withArb && arb) {
                ok = await send(tx.accept({ debtor, creditor, amount, maturityEpochs: maturity, arb }));
            } else {
                const sale = tx.sale({ seller, buyer, amount, maturityEpochs: maturity });
                const saleChk = await precheck(sale);
                if (saleChk.ok) {
                    ok = await send(sale);
                } else {
                    const accept = tx.accept({ debtor, creditor, amount, maturityEpochs: maturity });
                    const acceptChk = await precheck(accept);
                    if (acceptChk.ok) {
                        ok = await send(accept);
                    } else {
                        // Surface the cascade lane's reason — it is the primary one.
                        errorStore.pushError(rejectionMessage(saleChk.code ?? 'ET-UNKNOWN'));
                    }
                }
            }
            // Recorded (committed, or parked for the counterparty's signature):
            // back to the history, where it now shows.
            if (ok) dispatch('close');
        } finally {
            submitting = false;
        }
    }

    async function copyOffer() {
        if (offer && (await copyText(offer.payload))) {
            errorStore.pushError($_('common.copied', { default: 'Copied to clipboard' }), 'warning');
        }
    }
</script>

<ActionPage
    icon={offer ? 'qr_code_2' : 'shopping_cart'}
    title={offer
        ? $_('transactions.offerTitle', { default: 'Show this to the seller' })
        : $_('transactions.purchase', { default: 'Record a purchase' })}
    on:close={() => dispatch('close')}
>
    {#if offer}
        <!-- The purchase is signed; what is left is the seller's half, and it
             cannot wait on the node for a key that has no account. -->
        <div class="offer-panel">
            <QrCodeDisplay value={offer.payload} size={260} level="L" />
            <Explain
                summary={fallback
                    ? $_('transactions.offerFallback', {
                          default: "The seller's app could not take it through the node, so show them this code instead.",
                      })
                    : $_('transactions.offerSummary', {
                          default: 'Show this to the seller — their signature completes the purchase.',
                      })}
            >
                {$_('transactions.offerBody', {
                    default:
                        'Your purchase is signed on this device. They scan the code, or you send them the text; they check it and sign, and their signature creates your account and books the debt at once. The code stays under Transactions until then.',
                })}
            </Explain>
            <div class="page-actions">
                <Button variant="outlined" on:click={copyOffer}>
                    <Label>{$_('offers.copy', { default: 'Copy as text' })}</Label>
                </Button>
                <Button variant="raised" on:click={() => dispatch('close')}>
                    <Label>{$_('transactions.offerDone', { default: 'Done' })}</Label>
                </Button>
            </div>
        </div>
    {:else}
        <Explain summary={buySummary}>
            {$_('transactions.purchaseExplainer', {
                default:
                    "Enter the seller's wallet address or scan their QR. The purchase binds once they sign it from their app, and if the seller has open debts your purchase automatically helps clear them.",
            })}
            {#if keyed}
                {$_('transactions.keyedExplainer', {
                    default:
                        'This purchase reaches the seller on their invitation, which their QR carries; without one it leaves this device as a code you show them. Either way their signature creates your account and books the debt at once.',
                })}
            {/if}
        </Explain>

        <!-- One row when there is room for it: who, how much, by when. The
             address needs the most space, so it is the field that stretches;
             when the column narrows, whole fields wrap rather than shrinking
             into unreadable slivers. -->
        <div class="form-grid">
            <div class="cell cell-address">
                <AddressInput
                    bind:value={other}
                    bind:party={otherParty}
                    bind:invite
                    newcomers={!keyed}
                    byKey={keyed}
                    label={$_('transactions.sellerAddress', { default: "Seller's address" })}
                    exclude={me === null ? [] : [me]}
                />
            </div>
            <!-- Only the outlooks a PURCHASE can produce. `insuranceOutlook`
                 is asked with 'buy', so the member is the debtor and the
                 seller is the creditor: the `-mine` pair, where the member
                 is the one who bears an uninsured loss, is reachable from
                 'sell' alone and no screen asks that — the seller reviews a
                 purchase by its risk score, and the "I sold" form this copy
                 was written for is gone. Rendering an unreachable branch is
                 six locales of prose nobody can ever read, and its text said
                 "you would bear it alone" to the party who would not. -->
            {#if outlook === 'uninsured-theirs'}
                <div class="cell cell-address">
                    <Explain
                        tone="notice"
                        icon="info"
                        summary={$_('transactions.uninsuredTheirsSummary', {
                            default: 'Not insured: this is more than your capacity carries.',
                        })}
                    >
                        {$_('transactions.uninsuredTheirs', {
                            default:
                                'Nothing is reserved, and if you do not pay, the seller bears it alone. They may still agree — that is their decision to make — but it is worth knowing you are asking for it.',
                        })}
                    </Explain>
                </div>
            {:else if outlook === 'beyond-horizon-theirs'}
                <div class="cell cell-address">
                    <Explain
                        tone="notice"
                        icon="info"
                        summary={$_('transactions.beyondHorizonTheirsSummary', {
                            values: { horizon: insuredHorizon },
                            default: `Not insured: this maturity is past the ${insuredHorizon} epochs the community stands behind a claim for.`,
                        })}
                    >
                        {$_('transactions.beyondHorizonTheirs', {
                            default:
                                'Nothing is reserved, and if you do not pay, the seller bears it alone. A shorter term is insured; a longer one can be re-accepted when it falls due.',
                        })}
                    </Explain>
                </div>
            {:else if otherUnbacked}
                <div class="cell cell-address">
                    <Explain
                        tone="notice"
                        icon="info"
                        summary={$_('transactions.unbackedSummary', {
                            default: 'Nobody has backed them yet, so nothing binds them to deliver.',
                        })}
                    >
                        {$_('transactions.unbackedCounterparty', {
                            default:
                                'That is not a mark against them — every account starts there. If the amount matters to you, name an arbiter below: it is the only thing that binds somebody with no standing to an outcome.',
                        })}
                    </Explain>
                </div>
            {/if}
            <div class="cell cell-amount">
                <NumericInput
                    style="width: 100%;"
                    bind:value={amountStr}
                    label={$_('transactions.amount', { default: 'Amount' })}
                />
            </div>
            <div class="cell cell-maturity">
                <NumericInput
                    integer
                    style="width: 100%;"
                    min={minMaturity}
                    bind:value={maturityStr}
                    label={$_('transactions.maturity', { values: { min: minMaturity }, default: `Maturity (epochs, min ${minMaturity})` })}
                />
            </div>
        </div>

        <FormField>
            <Checkbox bind:checked={withArb} />
            <span slot="label">
                {$_('transactions.arbToggle', { default: 'Agree on an arbitration panel (optional, immutable after)' })}
            </span>
        </FormField>
        {#if withArb}
            <div class="arb-box">
                <Textfield
                    textarea
                    label={$_('transactions.arbiters', { default: 'Arbiter addresses (one per line)' })}
                    bind:value={arbAddresses}
                    style="width: 100%;"
                />
                <div class="form-row">
                    <NumericInput integer min={1} bind:value={arbQuorumStr} label={$_('transactions.arbQuorum', { default: 'Quorum' })} />
                    <NumericInput integer min={1} bind:value={arbWindowStr} label={$_('transactions.arbWindow', { default: 'Window (epochs after maturity)' })} />
                    <NumericInput bind:value={arbCapStr} label={$_('transactions.arbCap', { default: 'Award ceiling' })} />
                </div>
            </div>
        {/if}

        <Explain
            summary={$_('transactions.maturitySummary', {
                values: { epoch, date: $epochDate(epoch) },
                default: `Unpaid at its maturity, this can be marked a default by anyone. Today is ${$epochDate(epoch)}, epoch ${epoch}.`,
            })}
        >
            {$_('transactions.maturityDisclosure', {
                default:
                    "The debtor's standing stays consumed, and where the community was standing behind the debt, the underwriters whose backing carried it become the creditor's debtors instead.",
            })}
        </Explain>

        <!-- Follows the lane `submit` will actually take: arbitration forces the
             plain-contract lane, since Sale carries no arbitration terms. Both
             classes happen to be priced the same, but tying the notice to the
             lane keeps it honest if the schedule ever separates them. -->
        <BondNotice
            plan={withArb
                ? tx.accept({ ...noticeParties, amount: 0, maturityEpochs: minMaturity })
                : tx.sale({ ...noticeParties, amount: 0, maturityEpochs: minMaturity })}
        />

        <div class="page-actions">
            <Button variant="raised" disabled={submitting || otherParty === null} on:click={submit}>
                <Label>{$_('transactions.record', { default: 'Record purchase' })}</Label>
            </Button>
            <Button variant="outlined" disabled={submitting} on:click={() => dispatch('close')}>
                <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
            </Button>
        </div>
    {/if}
</ActionPage>

<style>
    .offer-panel {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 14px;
        text-align: center;
    }
    /* One line when the column is wide enough, wrapping by whole fields when
       it is not. Flex bases rather than media queries: the form sits in a
       content column whose width has little to do with the viewport's (the
       drawer takes a bite out of it). */
    .form-grid {
        display: flex;
        flex-wrap: wrap;
        gap: 12px 18px;
        align-items: flex-start;
        width: 100%;
    }
    .cell {
        min-width: 0;
    }
    .cell-address {
        flex: 3 1 300px;
    }
    .cell-amount {
        flex: 1 1 140px;
        max-width: 220px;
    }
    .cell-maturity {
        flex: 1 1 200px;
        max-width: 280px;
    }
    /* The arbitration panel's three numbers: same idea, all equal. */
    .form-row {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
        gap: 12px 18px;
        align-items: start;
    }
    .arb-box {
        border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
        border-radius: 8px;
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .page-actions {
        display: flex;
        gap: 10px;
        flex-wrap: wrap;
    }
</style>
