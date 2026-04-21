<script lang="ts">
    import {_} from 'svelte-i18n';
    import {createEventDispatcher, getContext, onMount, onDestroy} from 'svelte';
    import type {Action, AgentPubKey, AppClient, HolochainError, NewEntryAction, Record, SignedActionHashed} from '@holochain/client';
    import {decodeHashFromBase64, encodeHashToBase64, fakeAgentPubKey} from "@holochain/client";
    import {clientContext} from '../../contexts';
    import type {Party, Transaction, Wallet, TransactionStatus} from './types';
    import Button from '@smui/button';
    import Textfield from '@smui/textfield';
    import CharacterCounter from '@smui/textfield/character-counter';
    import { Icon } from '@smui/common';
    import Dialog, { Actions, Content as DialogContent, Title as DialogTitle } from '@smui/dialog';
    import WalletDetail from './WalletDetail.svelte';

    import {isValidAddress, formatNumber, parseNumber, isValidNumberFormat, cleanNumberInput, extractHolochainErrorCode} from "../../common/functions";
    import {TRIAL_THRESHOLD} from "../../common/constants";
    import NumericInput from "../../common/NumericInput.svelte";
    import {localizationSettings} from "../../common/localizationSettings";
    import {decode} from "@msgpack/msgpack";
    import { copyToClipboard } from '../../common/clipboard';
    import { errorStore } from '../../common/errorStore';
    import IconButton from '@smui/icon-button';
    import HelperText from '@smui/textfield/helper-text';
    import AgentAvatar from './AgentAvatar.svelte';
    import QrScanner from './QrScanner.svelte';

    let client: AppClient = (getContext(clientContext) as any).getClient();

    const dispatch = createEventDispatcher();

    let afterTransactionStatus: TransactionStatus | undefined;
    let isTrial: boolean = false;

    // ── Peer wallet lookup ────────────────────────────────────────────────────
    // Allows the buyer to inspect the seller's wallet metrics before transacting.
    // The lookup is read-only — the same WalletDetail component used for the buyer's
    // own wallet is reused here with the seller's walletRecord as prop.
    let peerLookupOpen = false;
    let peerWalletRecord: Record | null = null;
    let peerLookupLoading = false;

    async function openPeerLookup() {
        if (!isAddressFormatValid || inputs.sellerAddress.invalid) return;
        peerLookupLoading = true;
        peerLookupOpen = true;
        try {
            const [, record]: [unknown, Record | null] = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_wallet_for_agent',
                payload: decodeHashFromBase64(inputs.sellerAddress.value),
            });
            peerWalletRecord = record;
        } catch (e) {
            console.error('Peer wallet lookup failed:', e);
            peerWalletRecord = null;
        }
        peerLookupLoading = false;
    }

    let buyer: AgentPubKey = client.myPubKey;

    let buyerWallet: Record;
    let buyerLastTransaction: Record;
    let sellerWallet: Record;
    let sellerLastTransaction: Record;

    let scannerOpen = false;


    // Whether the buyer has previous transactions with the entered seller.
    // null = not yet fetched, true/false = fetched result.
    let hasBilateralHistory: boolean | null = null;
    let bilateralHistoryDebounce: ReturnType<typeof setTimeout> | null = null;

    // Debounce handle for the transaction simulation check.
    // Without debouncing, every keystroke in the amount field fires a separate async call.
    // Rapid successive calls can resolve out of order, leaving afterTransactionStatus stale.
    let simulationDebounce: ReturnType<typeof setTimeout> | null = null;

    $: isTrialAmount = numericDebt > 0 && numericDebt < TRIAL_THRESHOLD;


    let isTransactionCreateValid = false;

    interface Form {
        sellerAddress: { 
            value: string,
            invalid: boolean,
            validationError: string,
        },
        debt: {
            value: string,
            invalid: boolean,
            validationError: string
        },
        description: {
            value: string
        },
        autoRejectThreshold: {
            invalid: boolean
        },
        autoAcceptThreshold: {
            invalid: boolean,
            warning: boolean
        }
    }

    let inputs: Form = {
        sellerAddress: { 
            value: "",
            invalid: false,
            validationError: "",
        },
        debt: {
            value: "",
            invalid: false,
            validationError: ""
        },
        description: {
            value: ""
        },
        autoRejectThreshold: {
            invalid: false
        },
        autoAcceptThreshold: {
            invalid: false,
            warning: false
        } 
    }

    let numericDebt = 0;
    let isAddressFormatValid = false;

    // Decouple debt validation from address validation to prevent flickering of bilateral history checks
    $: {
        const rawDebt = inputs.debt.value;
        const formatValid = isValidNumberFormat(rawDebt);
        numericDebt = parseNumber(rawDebt);

        if (!formatValid || isNaN(numericDebt) || numericDebt <= 0) {
            inputs.debt.invalid = (rawDebt !== "");
            inputs.debt.validationError = $_("createTransaction.validationErrorDebtFormat");
        } else {
            inputs.debt.invalid = false;
            inputs.debt.validationError = "";
        }
    }

    $: {
        const address = inputs.sellerAddress.value;
        isAddressFormatValid = isValidAddress(address);

        const isAddressFilled = address && address.length > 0;
        if (!isAddressFormatValid) {
            inputs.autoRejectThreshold.invalid = false;
            inputs.autoAcceptThreshold.warning = false;
            inputs.sellerAddress.invalid = !!isAddressFilled;
            inputs.sellerAddress.validationError = $_("createTransaction.validationErrorAddressFormat");
        } else if (address === encodeHashToBase64(client.myPubKey)) {
            inputs.autoRejectThreshold.invalid = false;
            inputs.autoAcceptThreshold.warning = false;
            inputs.sellerAddress.invalid = true;
            inputs.sellerAddress.validationError = $_("createTransaction.validationErrorAddressOwn");
        } else {
            inputs.sellerAddress.invalid = false;
            inputs.sellerAddress.validationError = "";
        }
    }

    // Secondary reactive block for async simulation check.
    // Uses a 300ms debounce to prevent concurrent out-of-order resolutions when the
    // user types quickly. A request identifier ensures that only the most recent
    // pending call updates the UI (stale responses from superseded calls are discarded).
    let simulationRequestId = 0;
    $: if (isAddressFormatValid && !inputs.sellerAddress.invalid && numericDebt > 0) {
        if (simulationDebounce !== null) clearTimeout(simulationDebounce);
        const capturedId = ++simulationRequestId;
        simulationDebounce = setTimeout(async () => {
            try {
                const result = await getTransactionStatusFromSimulation();
                // Discard if a newer request has been issued since this one started.
                if (capturedId !== simulationRequestId) return;

                if (result) {
                    afterTransactionStatus = result.status;
                    isTrial = result.is_trial;
                    inputs.autoRejectThreshold.invalid = result.status.type == "Rejected";
                    inputs.autoAcceptThreshold.warning = result.status.type == "Pending";
                } else {
                    // Seller has no wallet yet — clear any stale indicators.
                    afterTransactionStatus = undefined;
                    isTrial = false;
                    inputs.autoRejectThreshold.invalid = false;
                    inputs.autoAcceptThreshold.warning = false;
                }
            } catch (e) {
                if (capturedId !== simulationRequestId) return;
                console.error("Simulation check failed:", e);
                afterTransactionStatus = undefined;
                inputs.autoRejectThreshold.invalid = false;
                inputs.autoAcceptThreshold.warning = false;
            }
        }, 300);
    } else {
        if (simulationDebounce !== null) clearTimeout(simulationDebounce);
        afterTransactionStatus = undefined;
    }

    $: {
        const isUIValid = !inputs.sellerAddress.invalid && !inputs.debt.invalid;
        // Ensure both wallets are loaded before allowing the transaction to be created.
        // Wallet hashes are required for the integrity check.
        const isWalletLoaded = !!sellerWallet && !!buyerWallet;
        isTransactionCreateValid = isUIValid && isAddressFormatValid && numericDebt > 0 && isWalletLoaded;
    }

    // Reactive: check bilateral history whenever the seller address changes.
    // Uses a local variable to prevent re-fetching when other dependencies (like numericDebt)
    // trigger a reactive update of address validity.
    let lastFetchedBilateralAddress: string | null = null;
    $: if (isAddressFormatValid && !inputs.sellerAddress.invalid) {
        const sellerAddr = inputs.sellerAddress.value;
        if (sellerAddr !== lastFetchedBilateralAddress) {
            if (bilateralHistoryDebounce !== null) clearTimeout(bilateralHistoryDebounce);
            hasBilateralHistory = null;
            lastFetchedBilateralAddress = sellerAddr;
            bilateralHistoryDebounce = setTimeout(async () => {
                try {
                    hasBilateralHistory = await client.callZome({
                        role_name: 'edet',
                        zome_name: 'transaction',
                        fn_name: 'check_bilateral_history',
                        payload: decodeHashFromBase64(sellerAddr),
                    });
                } catch (_) {
                    hasBilateralHistory = null;
                }
            }, 400);
        }
    } else {
        hasBilateralHistory = null;
        lastFetchedBilateralAddress = null;
    }

    onMount(async () => {
        let _entryHash;
        [_entryHash, buyerWallet] = await client.callZome({
            role_name: 'edet',
            zome_name: 'transaction',
            fn_name: 'get_wallet_for_agent',
            payload: buyer,
        });
        buyerLastTransaction = await client.callZome({
            role_name: 'edet',
            zome_name: 'transaction',
            fn_name: 'get_agent_last_transaction',
            payload: buyer,
        });
    });

    async function getTransactionStatusFromSimulation(): Promise<{status: TransactionStatus, is_trial: boolean} | undefined> {
        let _entryHash;
        [_entryHash, sellerWallet] = await client.callZome({
            role_name: 'edet',
            zome_name: 'transaction',
            fn_name: 'get_wallet_for_agent',
            payload: inputs.sellerAddress.value,
        });
        if (!sellerWallet) {
            // Seller has no wallet yet — simulation cannot run, return undefined silently.
            return undefined;
        }
        sellerLastTransaction = await client.callZome({
            role_name: 'edet',
            zome_name: 'transaction',
            fn_name: 'get_agent_last_transaction',
            payload: inputs.sellerAddress.value,
        });
        let transaction: Transaction = {
            seller: {
                pubkey: inputs.sellerAddress.value,
                side: { type: "Seller" },
                previous_transaction: sellerLastTransaction?.signed_action?.hashed?.hash ?? null,
                wallet: sellerWallet.signed_action.hashed.hash
            },
            buyer: {
                pubkey: encodeHashToBase64(buyer),
                side: { type: "Buyer" },
                previous_transaction: buyerLastTransaction?.signed_action?.hashed?.hash ?? null,
                wallet: buyerWallet.signed_action.hashed.hash
            },
            debt: numericDebt,
            description: "",
            is_trial: false,
            status: { type: 'Testing' }
        };
        let result = await client.callZome({
            role_name: 'edet',
            zome_name: 'transaction',
            fn_name: 'get_transaction_status_from_simulation',
            payload: transaction,
        });
        return result
    }

    async function createTransaction() {
        if (!sellerWallet || !buyerWallet) {
            errorStore.pushError($_("createTransaction.errorWalletsNotLoaded", {default: "Wallets not fully loaded. Please wait a moment."}));
            return;
        }

        const transactionEntry: Transaction = {
            seller: {
                pubkey: inputs.sellerAddress.value,
                side: { type: "Seller" },
                previous_transaction: sellerLastTransaction?.signed_action?.hashed?.hash ?? null,
                wallet: sellerWallet.signed_action.hashed.hash
            },
            buyer: {
                pubkey: encodeHashToBase64(buyer),
                side: { type: "Buyer" },
                previous_transaction: buyerLastTransaction?.signed_action?.hashed?.hash ?? null,
                wallet: buyerWallet.signed_action.hashed.hash
            },
            debt: numericDebt,
            description: inputs.description.value,
            // is_trial is set authoritatively by the backend; pass false as a placeholder
            // (the backend will override this based on the debt vs TRIAL_FRACTION * BASE_CAPACITY)
            is_trial: false,
            status: afterTransactionStatus ?? { type: 'Pending' },
        };
        try {
            const transactionRecord: Record = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'create_transaction',
                payload: transactionEntry,
            });
            buyerLastTransaction = transactionRecord;
            isTrial = false;
            dispatch('transaction-created', {transactionHash: transactionRecord.signed_action.hashed.hash});
        } catch (e: any) {
            console.error(e);
            let he = e as HolochainError;
            let message = $_("errors." + extractHolochainErrorCode(he.message), { default: he.message });
            const errorMsg = $_("createTransaction.errorCreateTransaction", {values: {name: he.name, message}});
            errorStore.pushError(errorMsg);
        }
    }

</script>
<h4 class="flex-row">
    { $_("createTransaction.createTransaction")}
</h4>
<div class="flex-column form-container">
    {#if (inputs.autoRejectThreshold.invalid)}
        <!-- role="alert" causes screen readers to announce this immediately when it appears -->
        <div class="error-msg paragraph" role="alert" aria-live="assertive" aria-atomic="true">
            <div class="flex-row error-title">
                <Icon class="material-icons" aria-hidden="true">error</Icon> {$_("createTransaction.errorDisadvantageTitle")}</div>
            <div class="flex-row error-info">  {$_("createTransaction.errorDisadvantageInfo")}</div>
        </div>
    {:else if isTrial}
        <div class="info-msg paragraph" role="status" aria-live="polite" aria-atomic="true">
            <div class="flex-row info-title">
                <Icon class="material-icons" aria-hidden="true">flash_on</Icon> {$_("createTransaction.trialTransactionTitle")}</div>
            <div class="flex-row info-info">  {$_("createTransaction.trialTransactionInfo")}</div>
        </div>
    {:else if (inputs.autoAcceptThreshold.warning)}
        <!-- role="status" is polite — announced at the next opportunity, not interrupting -->
        <div class="warning-msg paragraph" role="status" aria-live="polite" aria-atomic="true">
            <div class="flex-row warning-title">
                <Icon class="material-icons" aria-hidden="true">warning</Icon> {$_("createTransaction.warningModerationTitle")}</div>
            <div class="flex-row warning-info">  {$_("createTransaction.warningModerationInfo")}</div>
        </div>
    {/if}
    <div class="flex-row paragraph" style="gap: 12px; align-items: flex-start;">
        <div style="width: 32px; height: 56px; display: flex; align-items: center; justify-content: center;">
            {#if isValidAddress(inputs.sellerAddress.value)}
                <AgentAvatar agentPubKey={inputs.sellerAddress.value} size={32} />
            {/if}
        </div>
        <div class="flex-hgrow flex-column">
            <div class="flex-row align-vcenter" style="gap: 8px;">
                <Textfield class="tf-address address-text"
                            label={$_("createTransaction.sellerAddress")}
                            style="flex: 1;"
                            invalid={inputs.sellerAddress.invalid}
                            bind:value={inputs.sellerAddress.value}>
                </Textfield>
                <IconButton class="qr-scan-btn" on:click={() => scannerOpen = true} title={$_("createTransaction.scanQr")}>
                    <Icon class="material-icons">border_all</Icon>
                </IconButton>
                {#if isAddressFormatValid && !inputs.sellerAddress.invalid}
                    <IconButton
                        title={$_("createTransaction.viewSellerMetrics", {default: "View seller metrics"})}
                        on:click={openPeerLookup}>
                        <Icon class="material-icons">account_circle</Icon>
                    </IconButton>
                {/if}
            </div>
            <QrScanner bind:open={scannerOpen} on:scan-success={(e) => {
                inputs.sellerAddress.value = e.detail.text;
                scannerOpen = false;
            }} />
            {#if inputs.sellerAddress.invalid}
                <HelperText class="error" persistent>{inputs.sellerAddress.validationError}</HelperText>
            {:else if hasBilateralHistory === true}
                <HelperText persistent style="color: var(--mdc-theme-primary);">
                    <Icon class="material-icons" style="font-size:14px;vertical-align:middle;">history</Icon>
                    {$_("createTransaction.bilateralHistoryExists", {default: "Returning seller — bilateral history found"})}
                </HelperText>
            {:else if hasBilateralHistory === false}
                <HelperText persistent style="color: var(--mdc-theme-secondary, #888);">
                    <Icon class="material-icons" style="font-size:14px;vertical-align:middle;">person_add</Icon>
                    {$_("createTransaction.bilateralHistoryNone", {default: "First-time seller — no previous history"})}
                </HelperText>
            {/if}
        </div>
    </div>
    <div class="flex-row paragraph">
        <div class="flex-hgrow flex-column">
            <NumericInput
                label={$_("createTransaction.debt")}
                style="width: 100%; flex: 1;"
                invalid={inputs.debt.invalid}
                bind:value={inputs.debt.value}
                on:input={() => {
                    // Trigger reactivity if needed, or rely on binding
                }}
            />
            {#if inputs.debt.invalid}
                <HelperText class="error" persistent>{inputs.debt.validationError}</HelperText>
            {/if}
        </div>
    </div>
    <div class="flex-row paragraph">
        <Textfield textarea class="flex-hgrow tf-description"
                    input$maxlength={200}
                      label={$_("createTransaction.description")}
                      bind:value={inputs.description.value}>
            <CharacterCounter slot="internalCounter">{inputs.description.value.length} / 200</CharacterCounter>
        </Textfield>
    </div>
    <div class="flex-row helper-text">
        { $_("createTransaction.createTransactionHelperText")}
    </div>
    <div class="flex-row flex-hcenter" style="gap: 12px;">
        <Button
                class="flex-1"
                on:click={() => dispatch('create-canceled')}
                variant="outlined"
        >
            {$_("createTransaction.cancel")}
        </Button>
        <Button
                class="flex-1"
                disabled={!isTransactionCreateValid}
                on:click={() => createTransaction()}
                variant="raised"
        >
        {$_("createTransaction.buy")}
        </Button>
    </div>
</div>

<!-- ── Peer Wallet Lookup Dialog ─────────────────────────────────────────── -->
<!-- Read-only view of the seller's wallet metrics before creating a transaction -->
<Dialog bind:open={peerLookupOpen} aria-labelledby="peer-lookup-title" class="metrics-dialog">
    <DialogTitle id="peer-lookup-title">
        <Icon class="material-icons" style="vertical-align:middle;margin-right:6px;">account_circle</Icon>
        {$_("createTransaction.sellerMetricsTitle", {default: "Seller Metrics"})}
        <span style="font-size:0.75rem;font-weight:400;margin-left:8px;opacity:0.6;">{inputs.sellerAddress.value.slice(0, 12)}…</span>
    </DialogTitle>
    <DialogContent>
        {#if peerLookupLoading}
            <div style="display:flex;justify-content:center;padding:32px;">
                <span class="material-icons" style="animation:spin 1s linear infinite;">hourglass_empty</span>
            </div>
        {:else if peerWalletRecord}
            <WalletDetail walletRecord={peerWalletRecord} showActions={false} />
        {:else}
            <p style="color:var(--mdc-theme-text-secondary-on-surface);text-align:center;padding:24px;">
                <Icon class="material-icons" style="display:block;font-size:40px;margin-bottom:8px;">person_off</Icon>
                {$_("createTransaction.sellerNoWallet", {default: "This seller has not yet published a wallet."})}
            </p>
        {/if}
    </DialogContent>
    <Actions>
        <Button on:click={() => peerLookupOpen = false}>
            {$_("createTransaction.close", {default: "Close"})}
        </Button>
    </Actions>
</Dialog>

<style>
    .form-container {
        display: flex;
        flex-direction: column;
        width: 100%;
        max-width: 800px;
        margin: 0 auto;
        padding: 0 16px;
        box-sizing: border-box;
        gap: 16px;
    }

    :global(.qr-scan-btn) {
        margin-top: 4px !important;
        color: var(--mdc-theme-primary) !important;
    }

    .info-msg {
        background: var(--mdc-theme-background, #e3f2fd);
        color: var(--mdc-theme-primary, #1565c0);
        padding: 12px;
        border-radius: 8px;
        border: 1px solid rgba(21, 101, 192, 0.2);
    }
    :global(.dark-theme) .info-msg {
        background: rgba(25, 118, 210, 0.1);
        border-color: rgba(25, 118, 210, 0.3);
        color: #90caf9;
    }
    .info-title {
        font-weight: 700;
        gap: 8px;
        align-items: center;
        margin-bottom: 4px;
    }
    .info-info {
        font-size: 0.9rem;
        opacity: 0.9;
    }

    :global(.qr-scan-btn.mdc-icon-button) {
        width: 44px !important;
        height: 44px !important;
        padding: 0 !important;
        margin: 0 !important;
        display: inline-flex !important;
        align-items: center !important;
        justify-content: center !important;
    }

    :global(.qr-scan-btn.mdc-icon-button .material-icons) {
        position: absolute !important;
        top: 0 !important;
        left: 0 !important;
        width: 44px !important;
        height: 44px !important;
        font-size: 24px !important;
        display: flex !important;
        align-items: center !important;
        justify-content: center !important;
        margin: 0 !important;
        padding: 0 !important;
        pointer-events: none !important;
        overflow: hidden !important;
        white-space: nowrap !important;
        letter-spacing: normal !important;
    }

    :global(.stepper-btn) {
        color: var(--mdc-theme-primary) !important;
        background: rgba(0,0,0,0.05);
        border-radius: 50%;
    }
    :global(.dark-theme) .stepper-btn {
        background: rgba(255,255,255,0.05);
    }

    :global(.metrics-dialog .mdc-dialog__surface) {
        max-width: 900px !important;
        width: 95vw !important;
    }
</style>