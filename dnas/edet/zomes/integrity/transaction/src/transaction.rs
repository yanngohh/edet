use crate::{
    functions::{previous_activity_matches, tag_to_ranking},
    types::{
        constants::{BASE_CAPACITY, EPOCH_DURATION_SECS, TRIAL_FRACTION, TRIAL_VELOCITY_LIMIT_PER_EPOCH},
        transaction_validation_error, type_resolution_error, wallet_to_transaction_validation_error,
        TransactionStatusTag,
    },
    Wallet,
};

use hdi::prelude::*;

#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum TransactionSide {
    Seller,
    Buyer,
}

#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum TransactionStatus {
    Pending,
    Accepted,
    Rejected,
    Canceled,
    Testing,
    Initial,
}

impl TransactionStatus {
    /// Determine transaction status from risk score and wallet thresholds.
    /// Risk score is in [0, 1] where 0 = no risk, 1 = maximum risk.
    ///
    /// Boundary semantics (inclusive on both ends):
    ///   score <= auto_accept_threshold  →  Accepted  (at or below accept threshold)
    ///   score >= auto_reject_threshold  →  Rejected   (at or above reject threshold)
    ///   otherwise                       →  Pending    (in the manual-review band)
    ///
    /// Using inclusive comparisons (<=, >=) so that a risk score exactly equal to
    /// a threshold deterministically maps to the named status rather than falling
    /// into Pending — matching user expectations that setting auto_accept_threshold=0.4
    /// means "auto-accept anything at or below 40% risk".
    pub fn from_risk_score_for_wallet(risk_score: f64, wallet: Wallet) -> Self {
        if risk_score <= wallet.auto_accept_threshold {
            TransactionStatus::Accepted
        } else if risk_score >= wallet.auto_reject_threshold {
            TransactionStatus::Rejected
        } else {
            TransactionStatus::Pending
        }
    }
}

#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
pub struct Party {
    pub side: TransactionSide,
    pub pubkey: AgentPubKeyB64,
    pub previous_transaction: Option<ActionHash>,
    pub wallet: ActionHash,
}

/// Metadata present on support cascade drain transactions.
///
/// A drain transaction is created by the supporter (buyer) on the beneficiary's cell
/// as a pending request to drain `allocated_amount` of the beneficiary's debt.
/// The beneficiary (seller) approves or rejects exactly like a purchase transaction.
/// On approval, `transfer_debt` runs locally and a sub-cascade fires.
/// No `DebtContract` is created — the beneficiary's existing contracts are reduced.
#[derive(Serialize, Deserialize, SerializedBytes, Debug, Clone, PartialEq)]
pub struct DrainMetadata {
    /// ActionHash of the originating buyer→seller transaction that triggered this cascade.
    pub parent_tx: ActionHash,
    /// Cascade depth: 0 = direct drain from original seller, 1 = second-level, etc.
    pub cascade_depth: u32,
    /// Amount allocated from the cascade waterfilling pass.
    pub allocated_amount: f64,
    /// Visited set carried for cycle detection across the async phase.
    /// Contains all agents already in the cascade chain above this node.
    pub visited: Vec<AgentPubKeyB64>,
}

#[derive(Clone, PartialEq)]
#[hdk_entry_helper]
pub struct Transaction {
    pub id: Option<ExternalHash>,
    pub seller: Party,
    pub buyer: Party,
    pub debt: f64,
    pub description: String,
    pub status: TransactionStatus,
    pub parent: Option<EntryHashB64>,
    pub updated_action: Option<ActionHash>,
    /// True when this is a bootstrap trial transaction (amount < eta * V_base).
    /// Immutably set at creation time. Trial transactions are always created with
    /// status Pending — the seller must manually approve them.
    /// The trial slot is released (allowing a new trial from the same buyer) only
    /// when the resulting DebtContract is Transferred (successful repayment).
    /// Expiry/default does NOT release the slot: the buyer must pay to earn another trial.
    #[serde(default)]
    pub is_trial: bool,
    /// If Some, this is a cascade drain request, not a purchase transaction.
    /// Created by the supporter on the beneficiary's cell to request debt drain.
    /// On acceptance: `transfer_debt` runs (no DebtContract created).
    /// On rejection: nothing changes.
    #[serde(default)]
    pub drain_metadata: Option<DrainMetadata>,
}

impl Transaction {
    /// Returns true if this transaction is a cascade drain request.
    pub fn is_drain(&self) -> bool {
        self.drain_metadata.is_some()
    }
}

pub fn validate_request_create_update_transaction(
    author: AgentPubKey,
    chain_top: ActionHash,
    action_timestamp: Timestamp,
    transaction: Transaction,
) -> Result<ValidateCallbackResult, WasmError> {
    let signed_action = must_get_action(transaction.seller.wallet.clone())?;
    let seller_wallet_entry_hash = signed_action.action().entry_hash().ok_or(wasm_error!(WasmErrorInner::Guest(
        type_resolution_error::WALLET_ENTRY_HASH_RESOLVE_FAILED.to_string()
    )))?;
    let signed_action = must_get_action(transaction.buyer.wallet.clone())?;
    let buyer_wallet_entry_hash = signed_action.action().entry_hash().ok_or(wasm_error!(WasmErrorInner::Guest(
        type_resolution_error::WALLET_ENTRY_HASH_RESOLVE_FAILED.to_string()
    )))?;
    let seller_last_transaction = transaction
        .seller
        .previous_transaction
        .clone()
        .and_then(|action_hash| must_get_action(action_hash).ok());
    let buyer_last_transaction = transaction
        .buyer
        .previous_transaction
        .clone()
        .and_then(|action_hash| must_get_action(action_hash).ok());

    let _seller_wallet: Wallet = seller_wallet_entry_hash.try_into()?;

    // Debt must be a finite positive number.
    // This check runs for ALL transaction types (drain and non-drain) at the integrity
    // layer as a defence-in-depth backstop. The coordinator validates this too
    // (coordinator_transaction_error::DEBT_MUST_BE_POSITIVE), but a node that writes
    // entries directly — bypassing the coordinator — must be caught here.
    if !transaction.debt.is_finite() || transaction.debt <= 0.0 {
        return Ok(ValidateCallbackResult::Invalid(transaction_validation_error::DEBT_NOT_POSITIVE.to_string()));
    }

    // Drain transactions are created by the SUPPORTER (buyer) on the beneficiary's cell,
    // not by the seller. Skip the buyer-must-create check for drains.
    if transaction.is_drain() {
        // For drain transactions: the author must be the beneficiary (seller field)
        // or the supporter (buyer field). Accept both for the creation path since
        // the supporter fires the initial create_drain_request on the beneficiary's cell.
        // Full role validation is handled by the coordinator since integrity cannot
        // do cross-cell lookups.
        //
        // Wallet reference validation is also skipped for the buyer side: the supporter
        // (buyer) is a remote agent whose wallet freshness cannot be verified without a
        // cross-cell DHT lookup (unavailable in integrity validation). The seller-wallet
        // reference (line above) has already been resolved.
        return Ok(ValidateCallbackResult::Valid);
    }

    if transaction.updated_action.is_none() && transaction.buyer.pubkey != author.to_owned().into() {
        return Ok(ValidateCallbackResult::Invalid(
            transaction_validation_error::BUYER_MUST_CREATE_TRANSACTION.to_string(),
        ));
    }

    // Enforce monotonous timestamps within the source chain: new transactions must have a
    // timestamp strictly greater than any previous Transaction entry on this chain.
    if previous_activity_matches(
        author.clone(),
        chain_top.clone(),
        Transaction::entry_type().ok(),
        |(_, activity): (usize, &RegisterAgentActivity)| Some(activity.action.action().timestamp() > action_timestamp),
    )? {
        return Ok(ValidateCallbackResult::Invalid(
            transaction_validation_error::TRANSACTION_TIMESTAMP_NON_MONOTONOUS.to_string(),
        ));
    }

    // Verify the referenced wallet is the LATEST one on the author's chain.
    // If the author (buyer or seller) has updated their wallet (thresholds, etc.) since
    // referencing it in this transaction, the transaction is rejected as obsolete.
    let is_seller = author == Into::<AgentPubKey>::into(transaction.seller.pubkey.clone());
    let is_buyer = author == Into::<AgentPubKey>::into(transaction.buyer.pubkey.clone());

    if is_seller
        && previous_activity_matches(
            author.clone(),
            chain_top.clone(),
            Wallet::entry_type().ok(),
            |(_, activity): (usize, &RegisterAgentActivity)| {
                Some(
                    activity
                        .action
                        .action()
                        .entry_hash()
                        .is_none_or(|action_entry_hash| *action_entry_hash != *seller_wallet_entry_hash),
                )
            },
        )?
    {
        return Ok(ValidateCallbackResult::Invalid(transaction_validation_error::SELLER_WALLET_OBSOLETE.to_string()));
    }

    if is_buyer
        && previous_activity_matches(
            author.clone(),
            chain_top.clone(),
            Wallet::entry_type().ok(),
            |(_, activity): (usize, &RegisterAgentActivity)| {
                Some(
                    activity
                        .action
                        .action()
                        .entry_hash()
                        .is_none_or(|action_entry_hash| *action_entry_hash != *buyer_wallet_entry_hash),
                )
            },
        )?
    {
        return Ok(ValidateCallbackResult::Invalid(transaction_validation_error::BUYER_WALLET_OBSOLETE.to_string()));
    }

    if let Some(msg) = {
        #[allow(unused_variables)]
        let (party_last_transaction, party_name) =
            if is_seller { (&seller_last_transaction, "Seller") } else { (&buyer_last_transaction, "Buyer") };

        previous_activity_matches::<_, Option<String>>(
            author.clone(),
            chain_top.clone(),
            Transaction::entry_type().ok(),
            |(_, activity): (usize, &RegisterAgentActivity)| -> Option<Option<String>> {
                let activity_action_hash = activity.action.action_address();
                let activity_action = activity.action.action();

                // 1. Check if this activity is a "skippable" transaction (Canceled or Rejected).
                // If it is, we skip it regardless of whether our pointer is None or an older sequence.
                let mut skip = false;
                if let Some(entry_hash) = activity_action.entry_hash().cloned() {
                    if let Ok(entry_hashed) = must_get_entry(entry_hash) {
                        if let Ok(found_transaction) = Transaction::try_from(entry_hashed.content) {
                            if matches!(
                                found_transaction.status,
                                TransactionStatus::Canceled | TransactionStatus::Rejected
                            ) {
                                skip = true;
                            }
                        }
                    }
                }

                if skip {
                    return None; // Skip this activity and keep searching back
                }

                // 2. We found a non-skippable transaction. Validate against our pointer.
                let last_transaction_record = match party_last_transaction.as_ref() {
                    Some(r) => r,
                    None => {
                        return Some(Some(if is_seller {
                            transaction_validation_error::SELLER_LAST_TRANSACTION_OBSOLETE.to_string()
                        } else {
                            transaction_validation_error::BUYER_LAST_TRANSACTION_OBSOLETE.to_string()
                        }));
                    }
                };

                let last_action_hash = last_transaction_record.action_address();
                let last_action = last_transaction_record.action();

                // If this activity is older than or the same sequence as our pointer, it's not "newer" so it's not obsolete.
                // UNLESS it's at the same sequence but a different hash (branching).
                if activity_action.action_seq() < last_action.action_seq() {
                    return None; // Keep searching back
                }

                if activity_action.action_seq() == last_action.action_seq() {
                    // Same sequence: hashes must match OR it must be the original of an update we are pointing to.
                    let matches = if last_action_hash == activity_action_hash {
                        true
                    } else if let Action::Update(ref update) = last_action {
                        update.original_action_address == *activity_action_hash
                    } else {
                        false
                    };

                    if matches {
                        return Some(None); // Validated! Stop searching.
                    }
                    // Different hash at same sequence: branch detected (obsolete).
                }

                // Found a non-skippable newer/branching transaction: obsolete!
                error!(
                    "{} last transaction obsolete: Expected action {}, found {} at seq {}",
                    party_name,
                    last_action_hash,
                    activity_action_hash,
                    activity_action.action_seq()
                );
                Some(Some(if is_seller {
                    transaction_validation_error::SELLER_LAST_TRANSACTION_OBSOLETE.to_string()
                } else {
                    transaction_validation_error::BUYER_LAST_TRANSACTION_OBSOLETE.to_string()
                }))
            },
        )?
    } {
        return Ok(ValidateCallbackResult::Invalid(msg));
    }

    // Risk assessment (including EigenTrust reputation) is handled by the coordinator.
    // The integrity zome ensures structural correctness: correct author, ordering,
    // source chain linearity, and wallet references.
    Ok(ValidateCallbackResult::Valid)
}

/// Count the number of trial transaction approvals authored by `seller` in
/// the given `current_epoch`, by walking their source chain back from
/// `chain_top`.
///
/// A "trial approval" is an Update action on a Transaction entry that:
///   (a) was authored by `seller` (always true within the chain walk), AND
///   (b) transitions the transaction status to `Accepted`, AND
///   (c) references an `is_trial=true` original transaction, AND
///   (d) was created in the same epoch as the current action.
///
/// This implements the per-seller per-epoch trial velocity limit
/// (L_trial = 5, Whitepaper §2.3) must be enforced at the integrity layer
/// because a modified conductor could otherwise mass-approve trials by
/// writing arbitrary `wallet.trial_tx_count` values.
///
/// Returns the count (clamped at u32::MAX, which is far above any realistic
/// trial volume).
fn count_trial_approvals_in_epoch(
    seller: &AgentPubKey,
    chain_top: &ActionHash,
    current_epoch: u64,
) -> ExternResult<u32> {
    let activity = must_get_agent_activity(seller.clone(), ChainFilter::new(chain_top.clone()))?;
    let mut count: u32 = 0;
    for item in &activity {
        let action = item.action.action();
        // Only Update actions can transition Pending -> Accepted.
        let update = match action {
            Action::Update(u) => u,
            _ => continue,
        };
        // Stop early if this action is in an earlier epoch than the current
        // one. Activity is returned in chain order; once we cross the epoch
        // boundary we know there can be no more matches.
        let action_epoch = action.timestamp().as_seconds_and_nanos().0 as u64 / EPOCH_DURATION_SECS;
        if action_epoch != current_epoch {
            // Activity ordering is not strictly chronological in all cases
            // (re-published actions, etc.), so we don't break — we just skip.
            continue;
        }
        // Resolve the entry to check the new status.
        let entry_hash = update.entry_hash.clone();
        let entry = match must_get_entry(entry_hash) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let txn = match Transaction::try_from(entry.content) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // The new status must be Accepted.
        if txn.status != TransactionStatus::Accepted {
            continue;
        }
        // The original transaction must have been a trial. We can detect this
        // from the new entry too because is_trial is immutable across updates
        // (validated separately).
        if !txn.is_trial {
            continue;
        }
        count = count.saturating_add(1);
    }
    Ok(count)
}

pub fn validate_create_transaction(
    action: EntryCreationAction,
    transaction: Transaction,
) -> ExternResult<ValidateCallbackResult> {
    let status_initial = TransactionStatus::Initial;
    if transaction.status == status_initial {
        Ok(ValidateCallbackResult::Valid)
    } else {
        match action {
            EntryCreationAction::Create(_) => {
                // Drain transactions can be created with status Pending or Accepted (auto-accept).
                // The coordinator determines this via risk assessment.
                if transaction.is_drain()
                    && !matches!(transaction.status, TransactionStatus::Pending | TransactionStatus::Accepted)
                {
                    error!(
                        "Validation failed: Drain transaction must be Pending or Accepted at creation, found {:?}",
                        transaction.status
                    );
                    return Ok(ValidateCallbackResult::Invalid(
                        transaction_validation_error::DRAIN_INVALID_CREATION_STATUS.to_string(),
                    ));
                }

                // Trial transactions must be created with status Pending.
                // The coordinator enforces this; the integrity layer double-checks so that
                // no node can bypass the coordinator and commit an auto-accepted trial entry.
                if transaction.is_trial && transaction.status != TransactionStatus::Pending {
                    error!(
                        "Validation failed: Trial transaction must be Pending at creation, found {:?}",
                        transaction.status
                    );
                    return Ok(ValidateCallbackResult::Invalid(
                        transaction_validation_error::TRIAL_INVALID_CREATION_STATUS.to_string(),
                    ));
                }

                // Trial amount must be strictly below η · V_base (Whitepaper §2.3,
                // Theorem `thm:bootstrap`). The coordinator enforces this in normal flow,
                // but the integrity layer must also enforce it so that a modified conductor
                // cannot publish an `is_trial=true` transaction with arbitrary amount and
                // bypass the trial-slot extraction bound. With TRIAL_FRACTION = 0.05 and
                // BASE_CAPACITY = 1000, the cap is 50 units.
                let trial_amount_cap = TRIAL_FRACTION * BASE_CAPACITY;
                if transaction.is_trial && transaction.debt >= trial_amount_cap {
                    error!(
                        "Validation failed: Trial transaction amount {} exceeds cap {} (η · V_base)",
                        transaction.debt, trial_amount_cap
                    );
                    return Ok(ValidateCallbackResult::Invalid(
                        transaction_validation_error::TRIAL_AMOUNT_EXCEEDS_CAP.to_string(),
                    ));
                }

                validate_request_create_update_transaction(
                    action.author().to_owned(),
                    action.prev_action().to_owned(),
                    action.timestamp().to_owned(),
                    transaction,
                )
            }
            EntryCreationAction::Update(ref update) => validate_request_create_update_transaction(
                action.author().to_owned(),
                update.prev_action.clone(),
                action.timestamp().to_owned(),
                transaction,
            ),
        }
    }
}

pub fn validate_update_transaction(
    action: Update,
    transaction: Transaction,
    _original_action: EntryCreationAction,
    original_transaction: Transaction,
) -> ExternResult<ValidateCallbackResult> {
    // is_trial is immutable: cannot change between create and any update.
    if transaction.is_trial != original_transaction.is_trial {
        return Ok(ValidateCallbackResult::Invalid(
            transaction_validation_error::TRANSACTION_STATUS_INCOHERENT.to_string(), // Or a more specific code if we had one
        ));
    }

    // Enforce role-based status transition rules.
    // Only the seller can approve or reject; only the buyer can cancel.
    // These transitions are only valid from Pending status.
    match (&original_transaction.status, &transaction.status) {
        // Pending -> Accepted: seller-only (seller moderates purchases; seller=beneficiary moderates drains)
        (TransactionStatus::Pending, TransactionStatus::Accepted) => {
            let is_moderator = action.author == original_transaction.seller.pubkey.clone().into();
            if !is_moderator {
                error!(
                    "Validation failed: Only the moderator can approve transaction. Author={:?}, Seller={:?}, Buyer={:?}, is_drain={}",
                    action.author, original_transaction.seller.pubkey, original_transaction.buyer.pubkey, original_transaction.is_drain()
                );
                return Ok(ValidateCallbackResult::Invalid(
                    transaction_validation_error::ONLY_SELLER_CAN_APPROVE.to_string(),
                ));
            }

            // Per-seller per-epoch trial velocity limit (Whitepaper §2.3).
            // The coordinator enforces L_trial = 5 trials/epoch via
            // `wallet.trial_tx_count`, but a modified conductor can bypass this
            // by writing arbitrary `trial_tx_count` values. The integrity layer
            // must independently scan the seller's chain and count actual trial
            // approvals in the current epoch.
            //
            // We count Update actions on Transaction entries that:
            //   (a) authored by the seller (current action author), AND
            //   (b) transition status to `Accepted`, AND
            //   (c) reference an `is_trial=true` original transaction, AND
            //   (d) occurred in the same epoch as the current action.
            if original_transaction.is_trial {
                let current_epoch = action.timestamp.as_seconds_and_nanos().0 as u64 / EPOCH_DURATION_SECS;
                let trial_approvals_this_epoch =
                    count_trial_approvals_in_epoch(&action.author, &action.prev_action, current_epoch)?;
                if trial_approvals_this_epoch >= TRIAL_VELOCITY_LIMIT_PER_EPOCH {
                    error!(
                        "Validation failed: Trial velocity exceeded — seller already approved {} trials in epoch {} (limit: {})",
                        trial_approvals_this_epoch, current_epoch, TRIAL_VELOCITY_LIMIT_PER_EPOCH
                    );
                    return Ok(ValidateCallbackResult::Invalid(
                        transaction_validation_error::TRIAL_VELOCITY_EXCEEDED.to_string(),
                    ));
                }
            }
        }
        // Pending -> Rejected: seller-only (seller moderates purchases; seller=beneficiary moderates drains)
        (TransactionStatus::Pending, TransactionStatus::Rejected) => {
            let is_moderator = action.author == original_transaction.seller.pubkey.clone().into();
            if !is_moderator {
                error!(
                    "Validation failed: Only the moderator can reject transaction. Author={:?}, Seller={:?}, Buyer={:?}, is_drain={}",
                    action.author, original_transaction.seller.pubkey, original_transaction.buyer.pubkey, original_transaction.is_drain()
                );
                return Ok(ValidateCallbackResult::Invalid(
                    transaction_validation_error::ONLY_SELLER_CAN_REJECT.to_string(),
                ));
            }
        }
        // Pending -> Canceled: buyer-only (buyer withdraws their own request)
        // For purchases: buyer is the requester who can cancel.
        // For drains: buyer is the supporter (requester) who can cancel.
        // In both cases, the buyer field holds the requester.
        (TransactionStatus::Pending, TransactionStatus::Canceled) => {
            let is_requester = action.author == original_transaction.buyer.pubkey.clone().into();
            if !is_requester {
                error!(
                    "Validation failed: Only the requester can cancel transaction. Author={:?}, Seller={:?}, Buyer={:?}, is_drain={}",
                    action.author, original_transaction.seller.pubkey, original_transaction.buyer.pubkey, original_transaction.is_drain()
                );
                return Ok(ValidateCallbackResult::Invalid(
                    transaction_validation_error::ONLY_BUYER_CAN_CANCEL.to_string(),
                ));
            }
        }
        // Non-Pending -> Accepted/Rejected/Canceled: invalid
        // These terminal statuses can only be reached from Pending.
        (_, TransactionStatus::Accepted) | (_, TransactionStatus::Rejected) | (_, TransactionStatus::Canceled) => {
            return Ok(ValidateCallbackResult::Invalid(
                transaction_validation_error::INVALID_STATUS_TRANSITION_SOURCE.to_string(),
            ));
        }
        // All other transitions (e.g., same-status updates) are allowed
        _ => {}
    }

    validate_request_create_update_transaction(
        action.author.to_owned(),
        action.prev_action,
        action.timestamp.to_owned(),
        transaction,
    )
}

pub fn validate_delete_transaction(
    _action: Delete,
    _original_action: EntryCreationAction,
    _original_transaction: Transaction,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(transaction_validation_error::TRANSACTION_NOT_DELETABLE.to_string()))
}

pub fn validate_create_link_wallet_to_transactions(
    action: CreateLink,
    _base_address: AnyLinkableHash,
    target_address: AnyLinkableHash,
    tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    match tag_to_ranking(tag) {
        Ok((_, Some(tag), _)) => {
            let target_entry_hash = target_address
                .into_entry_hash()
                .ok_or(wasm_error!(WasmErrorInner::Guest("Could not resolve target entry from hash".into())))?;
            let target_entry = must_get_entry(target_entry_hash)?;
            match TransactionStatusTag::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))? {
                TransactionStatusTag::Pending => {
                    match target_entry
                        .as_app_entry()
                        .and_then(|bytes| Transaction::try_from(bytes.clone().into_sb()).ok())
                    {
                        Some(transaction) => {
                            if transaction.is_drain() {
                                // Drain transactions are created on the beneficiary's cell.
                                // After role realignment the beneficiary is the seller, so
                                // the author (beneficiary=seller) won't match buyer.pubkey.
                                // Allow either party to create the Pending ranking link.
                                let is_party = action.author == transaction.buyer.pubkey.clone().into()
                                    || action.author == transaction.seller.pubkey.clone().into();
                                if !is_party {
                                    Ok(ValidateCallbackResult::Invalid(
                                        wallet_to_transaction_validation_error::BUYER_MUST_PERFORM_ASSOCIATION
                                            .to_string(),
                                    ))
                                } else {
                                    Ok(ValidateCallbackResult::Valid)
                                }
                            } else if action.author != transaction.buyer.pubkey.into() {
                                Ok(ValidateCallbackResult::Invalid(
                                    wallet_to_transaction_validation_error::BUYER_MUST_PERFORM_ASSOCIATION.to_string(),
                                ))
                            } else {
                                Ok(ValidateCallbackResult::Valid)
                            }
                        }
                        None => Ok(ValidateCallbackResult::Invalid(
                            wallet_to_transaction_validation_error::INVALID_TRANSACTION_DATA.to_string(),
                        )),
                    }
                }
                TransactionStatusTag::Finalized => match target_entry.as_content() {
                    Entry::App(app_entry_bytes) => {
                        match Transaction::try_from(app_entry_bytes.clone().into_sb()).ok() {
                            Some(transaction) => {
                                let status_initial = TransactionStatus::Initial;
                                Ok(if transaction.status == status_initial {
                                    ValidateCallbackResult::Valid
                                } else if action.author != transaction.buyer.pubkey.clone().into()
                                    && [TransactionStatus::Canceled].contains(&transaction.status)
                                {
                                    ValidateCallbackResult::Invalid(
                                        wallet_to_transaction_validation_error::BUYER_MUST_PERFORM_ASSOCIATION
                                            .to_string(),
                                    )
                                } else if action.author != transaction.seller.pubkey.clone().into()
                                    && action.author != transaction.buyer.pubkey.clone().into()
                                    && [TransactionStatus::Accepted, TransactionStatus::Rejected]
                                        .contains(&transaction.status)
                                {
                                    ValidateCallbackResult::Invalid(
                                        wallet_to_transaction_validation_error::SELLER_MUST_PERFORM_ASSOCIATION
                                            .to_string(),
                                    )
                                } else {
                                    ValidateCallbackResult::Valid
                                })
                            }
                            None => Ok(ValidateCallbackResult::Invalid(
                                wallet_to_transaction_validation_error::INVALID_TRANSACTION_DATA.to_string(),
                            )),
                        }
                    }
                    _ => Ok(ValidateCallbackResult::Invalid(
                        wallet_to_transaction_validation_error::INVALID_TRANSACTION_DATA.to_string(),
                    )),
                },
            }
        }
        _ => Ok(ValidateCallbackResult::Valid),
    }
}

pub fn validate_delete_link_wallet_to_transactions(
    action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    target: AnyLinkableHash,
    tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    match tag_to_ranking(tag) {
        Ok((_, Some(tag), _)) => {
            let target_entry_hash = target
                .into_entry_hash()
                .ok_or(wasm_error!(WasmErrorInner::Guest("Could not resolve target entry from hash".into())))?;
            let target_entry = must_get_entry(target_entry_hash)?;
            match TransactionStatusTag::try_from(tag).map_err(|e| wasm_error!(WasmErrorInner::Guest(e.into())))? {
                TransactionStatusTag::Finalized => Ok(ValidateCallbackResult::Invalid(
                    wallet_to_transaction_validation_error::FINALIZED_TRANSACTION_ASSOCIATION_NOT_DELETABLE.to_string(),
                )),
                TransactionStatusTag::Pending => {
                    match target_entry
                        .as_app_entry()
                        .and_then(|bytes| Transaction::try_from(bytes.clone().into_sb()).ok())
                    {
                        Some(transaction) => {
                            if action.author != transaction.buyer.pubkey.clone().into()
                                && [TransactionStatus::Canceled].contains(&transaction.status)
                            {
                                Ok(ValidateCallbackResult::Invalid(
                                    wallet_to_transaction_validation_error::BUYER_MUST_PERFORM_ASSOCIATION.to_string(),
                                ))
                            } else if action.author != transaction.seller.pubkey.clone().into()
                                && [TransactionStatus::Accepted, TransactionStatus::Rejected]
                                    .contains(&transaction.status)
                            {
                                Ok(ValidateCallbackResult::Invalid(
                                    wallet_to_transaction_validation_error::SELLER_MUST_PERFORM_ASSOCIATION.to_string(),
                                ))
                            } else {
                                Ok(ValidateCallbackResult::Valid)
                            }
                        }
                        None => Ok(ValidateCallbackResult::Invalid(
                            wallet_to_transaction_validation_error::INVALID_TRANSACTION_DATA.to_string(),
                        )),
                    }
                }
            }
        }
        _ => Ok(ValidateCallbackResult::Valid),
    }
}

pub fn validate_create_link_transaction_to_parent(
    _action: CreateLink,
    base_address: AnyLinkableHash,
    target_address: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    let entry_hash = base_address
        .into_entry_hash()
        .ok_or(wasm_error!(WasmErrorInner::Guest(String::from("No entry hash associated with link"))))?;
    let entry = must_get_entry(entry_hash)?.content;
    let _transaction = crate::Transaction::try_from(entry)?;
    let action_hash = target_address
        .into_action_hash()
        .ok_or(wasm_error!(WasmErrorInner::Guest(String::from("No action hash associated with link"))))?;
    let record = must_get_valid_record(action_hash)?;
    let _parent_transaction: crate::Transaction = record
        .entry()
        .to_app_option()
        .map_err(|e| wasm_error!(e))?
        .ok_or(wasm_error!(WasmErrorInner::Guest(String::from("Linked action must reference an entry"))))?;
    Ok(ValidateCallbackResult::Valid)
}

pub fn validate_delete_link_transaction_to_parent(
    _action: DeleteLink,
    _original_action: CreateLink,
    _base: AnyLinkableHash,
    _target: AnyLinkableHash,
    _tag: LinkTag,
) -> ExternResult<ValidateCallbackResult> {
    Ok(ValidateCallbackResult::Invalid(String::from("TransactionToParent links cannot be deleted")))
}
