<script lang="ts">
    import {_} from 'svelte-i18n';
    import {createEventDispatcher, getContext, onMount} from 'svelte';
    import {encodeHashToBase64, type ActionHash, type AppClient, type HolochainError, type NewEntryAction, type Record, type SignedActionHashed} from '@holochain/client';
    import {decode} from '@msgpack/msgpack';
    import {clientContext} from '../../contexts';
    import type {Transaction, TransactionStatus, Wallet} from './types';
    import {extractHolochainErrorCode} from '../../common/functions';
    import { errorStore } from '../../common/errorStore';
    import '@smui/button';

    import '@smui/slider';
    import TransactionDetail from "./TransactionDetail.svelte";


    let client: AppClient = (getContext(clientContext) as any).getClient();

    const dispatch = createEventDispatcher();

    export let record!: Record;
    let transaction: Transaction = decode((record.entry as any).Present.entry) as Transaction;
    
    let sellerDebt: number = 0;
    let sellerCapacity: number = 0;

    $: vitalityImpact = sellerCapacity > 0 ? (transaction.debt / sellerCapacity) * 100 : 0;

    let error: string = "";
    // Prevent double-clicks and re-moderation after a successful action
    let actionInProgress = false;

   

    onMount(async () => {
        sellerDebt = await client.callZome({
            role_name: 'edet',
            zome_name: 'transaction',
            fn_name: 'get_total_debt',
            payload: transaction.seller.pubkey,
        });
        sellerCapacity = await client.callZome({
            role_name: 'edet',
            zome_name: 'transaction',
            fn_name: 'get_credit_capacity',
            payload: transaction.seller.pubkey,
        });
    });

    async function updateTransaction(status: TransactionStatus) {
        // Guard: prevent duplicate submissions (double-click, re-click on stale card)
        if (actionInProgress) return;
        actionInProgress = true;

        try {
            let resultRecord: Record;

            if (status.type === 'Accepted') {
                // Use approve_pending_transaction which additionally calls call_remote to
                // trigger create_buyer_debt_contract on the buyer's chain.
                resultRecord = await client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'approve_pending_transaction',
                    payload: {
                        original_transaction_hash: record.signed_action.hashed.hash,
                        previous_transaction_hash: record.signed_action.hashed.hash,
                        transaction,
                    }
                });
            } else if (status.type === 'Rejected') {
                // Use reject_pending_transaction for proper seller-only validation.
                resultRecord = await client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'reject_pending_transaction',
                    payload: {
                        original_transaction_hash: record.signed_action.hashed.hash,
                        previous_transaction_hash: record.signed_action.hashed.hash,
                        transaction,
                    }
                });
            } else {
                // Canceled: use cancel_pending_transaction for proper pointer refresh.
                resultRecord = await client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'cancel_pending_transaction',
                    payload: {
                        original_transaction_hash: record.signed_action.hashed.hash,
                        previous_transaction_hash: record.signed_action.hashed.hash,
                        transaction,
                    }
                });
            }

            // Dispatch immediately (no setTimeout) with original hash for optimistic list removal.
            // actionInProgress stays true — card is gone from the list after this, no further clicks possible.
            dispatch('transaction-updated', { originalHash: record.signed_action.hashed.hash, resultRecord });
        } catch (e: any) {
            // Re-enable buttons only on failure so the user can retry
            actionInProgress = false;
            console.error(e);
            let he = e as HolochainError;
            let message = $_("errors." + extractHolochainErrorCode(he.message), { default: he.message });
            const errorMsg = $_("editTransaction.errorUpdateTransaction", {values: {name: he.name, message}});
            errorStore.pushError(errorMsg);
        }
    }

</script>

<TransactionDetail record={record} vitalityImpact={vitalityImpact} disabled={actionInProgress}
    on:transaction-accepted={async () => await updateTransaction({ type: 'Accepted'})}
    on:transaction-rejected={async () => await updateTransaction({ type: 'Rejected'})}
    on:transaction-canceled={async () => await updateTransaction({ type: 'Canceled'})}></TransactionDetail>
