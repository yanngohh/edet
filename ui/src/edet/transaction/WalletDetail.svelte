<script lang="ts">
    import { _ } from 'svelte-i18n'
    import {getContext, onMount} from 'svelte';
    import type { AppClient, Record} from '@holochain/client';
    import {formatNumber, formatPercentage} from "../../common/functions.js";
    import {localizationSettings} from "../../common/localizationSettings";
    import {TRIAL_THRESHOLD, TRIAL_VELOCITY_LIMIT_PER_EPOCH, FAILURE_TOLERANCE, EIGENTRUST_ALPHA} from "../../common/constants";
    import type {DebtContract, ReputationClaim, CommunityStanding} from './types';
    import AgentAvatar from './AgentAvatar.svelte';
    import '@smui/circular-progress';
    import type {ReputationResult, Wallet, NextDeadline} from './types';
    import type {ActionHash} from '@holochain/client';
    import {decode} from "@msgpack/msgpack";
    import {clientContext} from '../../contexts';
    import { copyToClipboard } from '../../common/clipboard';
    import { errorStore } from '../../common/errorStore';
    import { extractHolochainErrorCode } from '../../common/functions';
    import type { HolochainError } from '@holochain/client';
    import IconButton from '@smui/icon-button';
    import { Icon } from '@smui/common';
    import Snackbar, { Label } from '@smui/snackbar';
    import QrCodeDisplay from './QrCodeDisplay.svelte';

    export let walletRecord: Record;
    export let showActions: boolean = true;

    let client: AppClient = (getContext(clientContext) as any).getClient();

    let wallet: Wallet;
    let debt: number = 0;
    let trust: number = 0;
    let acquaintanceCount: number = 0;
    let capacity: number = 0;
    let openTrialContracts: number = 0;
    let vouchedCapacity: number = 0;
    let claim: ReputationClaim | null = null;
    let lockedToOthers: number = 0;
    let witnessCount: number = 0;
    let aggregateWitnessRate: number = 0;
    let riskScore: number = 0;
    let qrOpen = false;
    let nextExpiration: number | null = null;
    let nextDueAmount: number = 0;
    let expirationCountdown: string = "";
    let expirationUrgency: 'none' | 'info' | 'warning' | 'error' = 'none';
    let copySnackbar: Snackbar;

    // Operative capacity: mirrors the 3-level cascade enforced by the Rust coordinator
    // at transaction-creation time (transaction/mod.rs:221-235):
    //   1. If a fresh ReputationClaim exists, use claim.capacity_lower_bound (conservative,
    //      rounded-down bound certified by DHT validators at claim-publish time).
    //   2. Else if the agent has vouched capacity > 0, use that.
    //   3. Else use the full EigenTrust ceiling (get_credit_capacity).
    //
    // A claim is "fresh" if its timestamp is within MAX_CLAIM_STALENESS_SECONDS (900 s)
    // of the current wall-clock time.  Same check as Rust reputation.rs::is_claim_fresh.
    const MAX_CLAIM_STALENESS_SECONDS = 900;
    $: claimIsFresh = claim != null
        && (Math.floor(Date.now() / 1000) - claim.timestamp) <= MAX_CLAIM_STALENESS_SECONDS;
    $: totalCapacity = claimIsFresh && claim
        ? claim.capacity_lower_bound
        : vouchedCapacity > 0
            ? vouchedCapacity
            : capacity;

    // Headline: how much headroom the agent actually has across both sources
    $: available = Math.max(0, totalCapacity - debt);

    // Three-segment bar (full width = totalCapacity):
    // Segment 1 (red):    Debt portion of the total
    // Segment 2 (blue):   Vouched-in portion
    // Segment 3 (flex:1): Reputation-based headroom
    $: usedSeg       = totalCapacity > 0 ? (debt / totalCapacity) * 100 : 0;
    $: vouchedSeg    = totalCapacity > 0 ? (vouchedCapacity / totalCapacity) * 100 : 0;
    $: reputationSeg = totalCapacity > 0 ? ((totalCapacity - vouchedCapacity) / totalCapacity) * 100 : 0;

    $: wallet, debt, trust, capacity, openTrialContracts, vouchedCapacity, claim, riskScore, nextExpiration;

    // Relative reputation: trust / t_baseline = trust / (α / |A|) = trust × |A| / α.
    // Values above 1 mean the agent is above the noise floor; the progress bar
    // saturates at rel_rep = 5 (5× baseline) for a useful 0-100% scale.
    // Without this scaling, raw `trust` values (~1e-3) would always render near 0%.
    $: relRep = acquaintanceCount > 0 && EIGENTRUST_ALPHA > 0
        ? trust / (EIGENTRUST_ALPHA / acquaintanceCount)
        : 0;
    $: relRepBarPct = Math.min(100, (relRep / 5) * 100);

    $: if (nextExpiration) {
        updateCountdown();
    }

    function updateCountdown() {
        if (!nextExpiration) return;
        const now = Date.now();
        const diff = nextExpiration - now;
        
        if (diff <= 0) {
            expirationCountdown = $_("walletDetail.expired", {default: "Expired"});
            expirationUrgency = 'error';
            return;
        }

        const hours = Math.floor(diff / (1000 * 60 * 60));
        const minutes = Math.floor((diff % (1000 * 60 * 60)) / (1000 * 60));
        
        if (hours > 48) {
            expirationCountdown = $_("walletDetail.days_remaining", {values: {count: Math.floor(hours / 24)}, default: `${Math.floor(hours / 24)}d remaining`});
            expirationUrgency = 'info';
        } else if (hours >= 1) {
            expirationCountdown = `${hours}h ${minutes}m`;
            expirationUrgency = hours < 6 ? 'error' : 'warning';
        } else {
            expirationCountdown = `${minutes}m`;
            expirationUrgency = 'error';
        }
    }

    let countdownInterval: any;
    onMount(() => {
        countdownInterval = setInterval(updateCountdown, 60000);
        return () => clearInterval(countdownInterval);
    });

    $: if (walletRecord) {
        wallet = decode((walletRecord.entry as any).Present.entry) as Wallet;
        fetchMetrics();
    }

    async function fetchMetrics() {
        if (!wallet) return;

        // Track which metrics failed so we can show a single aggregated warning.
        const failedMetrics: string[] = [];

        // Fetch each metric independently so one failure doesn't block others.
        try {
            const repResult: ReputationResult = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_subjective_reputation',
                payload: wallet.owner,
            });
            trust = repResult.trust;
            acquaintanceCount = repResult.acquaintance_count ?? 0;
        } catch (e: any) {
            console.error('Failed to fetch reputation:', e);
            failedMetrics.push('trust');
        }

        try {
            debt = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_total_debt',
                payload: wallet.owner,
            });
        } catch (e: any) {
            console.error('Failed to fetch debt:', e);
            failedMetrics.push('debt');
        }

        try {
            capacity = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_credit_capacity',
                payload: wallet.owner,
            });
        } catch (e: any) {
            console.error('Failed to fetch capacity:', e);
            failedMetrics.push('capacity');
        }

        try {
            vouchedCapacity = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_vouched_capacity',
                payload: wallet.owner,
            });
        } catch (e: any) {
            console.error('Failed to fetch vouched capacity:', e);
            failedMetrics.push('vouchedCapacity');
        }

        try {
            const claimResult: [ActionHash, ReputationClaim] | null = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_reputation_claim',
                payload: wallet.owner,
            });
            claim = claimResult ? claimResult[1] : null;
        } catch (e: any) {
            console.error('Failed to fetch reputation claim:', e);
            failedMetrics.push('claim');
        }

        try {
            lockedToOthers = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_total_locked_capacity',
                payload: wallet.owner,
            });
        } catch (e: any) {
            console.error('Failed to fetch locked capacity:', e);
            failedMetrics.push('lockedCapacity');
        }

        try {
            // Count active trial contracts where this agent is the debtor (buyer) OR creditor (seller).
            const [debtorContracts, creditorContracts]: [Record[], Record[]] = await Promise.all([
                client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_active_contracts_for_debtor',
                    payload: wallet.owner,
                }),
                client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_active_contracts_for_creditor',
                    payload: wallet.owner,
                }),
            ]);
            const isTrialContract = (r: Record) => {
                const c = decode((r.entry as any).Present.entry) as DebtContract;
                return c.is_trial;
            };
            openTrialContracts = [...debtorContracts, ...creditorContracts].filter(isTrialContract).length;
        } catch (e: any) {
            console.error('Failed to fetch trial contracts:', e);
            failedMetrics.push('trialContracts');
        }

        try {
            // Community standing: failure witness count and aggregate witness rate
            const [witnesses, aggRate]: [string[], number] = await Promise.all([
                client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_failure_witnesses',
                    payload: wallet.owner,
                }),
                client.callZome({
                    role_name: 'edet',
                    zome_name: 'transaction',
                    fn_name: 'get_aggregate_witness_rate',
                    payload: wallet.owner,
                }),
            ]);
            witnessCount = witnesses.length;
            aggregateWitnessRate = aggRate;
        } catch (e: any) {
            console.error('Failed to fetch community standing:', e);
            failedMetrics.push('communityStanding');
        }

        try {
            riskScore = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_risk_score',
                payload: wallet.owner,
            });
        } catch (e: any) {
            console.error('Failed to fetch risk score:', e);
            failedMetrics.push('riskScore');
        }

        try {
            const nextExpResult: NextDeadline | null = await client.callZome({
                role_name: 'edet',
                zome_name: 'transaction',
                fn_name: 'get_next_debt_expiration',
                payload: wallet.owner,
            });
            // Result is microseconds from Timestamp, convert to ms
            nextExpiration = nextExpResult ? Math.floor(nextExpResult.timestamp / 1000) : null;
            nextDueAmount = nextExpResult ? nextExpResult.amount : 0;
        } catch (e: any) {
            console.error('Failed to fetch next expiration:', e);
            failedMetrics.push('nextExpiration');
        }

        // Surface a single aggregated warning if any metric failed, so the user
        // knows the dashboard may be showing stale or incomplete data rather than
        // silently showing zeros.
        if (failedMetrics.length > 0) {
            errorStore.pushError(
                $_('walletDetail.errorFetchMetrics', { default: 'Some wallet metrics could not be loaded' }),
                'warning'
            );
        }
    }
</script>

{#if wallet !== undefined}
    <div class="wallet-card card">
        <div class="card-header flex-row align-vcenter">
            <AgentAvatar agentPubKey={wallet.owner} size={48} />
            <div class="flex-column flex-1" style="margin-left: 12px; overflow: hidden;">
                <span class="owner-label">{$_("walletDetail.owner")}</span>
                <span class="address-text">{wallet.owner}</span>
            </div>
            {#if showActions}
                <div class="header-actions flex-row">
                    <IconButton on:click={() => qrOpen = true} title={$_("walletDetail.showQr")}>
                        <Icon class="material-icons">border_all</Icon>
                    </IconButton>
                    <IconButton on:click={() => { copyToClipboard(wallet.owner); copySnackbar.open(); }} title={$_("walletDetail.copyAddress")}>
                        <Icon class="material-icons">content_copy</Icon>
                    </IconButton>
                </div>
            {/if}
        </div>

        <Snackbar bind:this={copySnackbar} leading>
            <Label>{$_("walletDetail.addressCopied")}</Label>
        </Snackbar>

        <QrCodeDisplay bind:open={qrOpen} text={wallet.owner} />

        <div class="metrics-grid">
            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon debt" aria-hidden="true">account_balance</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.debt_label", {default: "Current Debt"})}</span>
                        <span class="metric-value">{formatNumber(debt, 2)}</span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.debt_tooltip")}
                    data-tooltip={$_("walletDetail.debt_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-container">
                    <div class="progress-bar debt" style="width: {capacity > 0 ? (Math.min(100, (debt / capacity) * 100)) : 0}%"></div>
                </div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon trust" aria-hidden="true">verified_user</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.reputation_label", {default: "Subjective Trust"})}</span>
                        <span class="metric-value">{formatNumber(trust, 4)}</span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.reputation_tooltip")}
                    data-tooltip={$_("walletDetail.reputation_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-container">
                    <!-- Bar shows relative reputation (trust / t_baseline), saturating at 5×.
                         Raw trust is shown as the numeric label above. -->
                    <div class="progress-bar trust" style="width: {relRepBarPct.toFixed(2)}%"></div>
                </div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon vitality" aria-hidden="true">favorite</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.vitality_label", {default: "System Vitality"})}</span>
                        <span class="metric-value">{capacity > 0 ? formatPercentage(Math.max(0, 1 - debt / capacity) * 100, 1) : (debt === 0 ? '100%' : '0%')}</span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.vitality_tooltip")}
                    data-tooltip={$_("walletDetail.vitality_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-container">
                    <div class="progress-bar vitality" style="width: {capacity > 0 ? (Math.max(0, (1 - debt / capacity) * 100)) : (debt === 0 ? 100 : 0)}%"></div>
                </div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon trial" aria-hidden="true">swap_horiz</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.trial_activity_label")}</span>
                        <span class="metric-value trial-value">
                            <span class="trial-part">{openTrialContracts} <span class="trial-sub">{$_("walletDetail.trial_activity_open")}</span></span>
                            <span class="trial-sep">·</span>
                            <span class="trial-part">{wallet.trial_tx_count || 0}/{TRIAL_VELOCITY_LIMIT_PER_EPOCH} <span class="trial-sub">{$_("walletDetail.trial_activity_seller_approved")}</span></span>
                        </span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.trial_activity_tooltip")}
                    data-tooltip={$_("walletDetail.trial_activity_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-spacer"></div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon capacity" aria-hidden="true">battery_charging_full</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.capacity_label")}</span>
                        <span class="metric-value">{formatNumber(available, 2)}</span>
                        <span class="metric-value trial-value">
                            <span class="trial-part">{formatNumber(debt, 2)} <span class="trial-sub">{$_("walletDetail.capacity_used")}</span></span>
                            <span class="trial-sep">·</span>
                            <span class="trial-part">{formatNumber(vouchedCapacity, 2)} <span class="trial-sub">{$_("walletDetail.capacity_vouched")}</span></span>
                            <span class="trial-sep">·</span>
                            <span class="trial-part">{formatNumber(totalCapacity - vouchedCapacity, 2)} <span class="trial-sub">{$_("walletDetail.capacity_reputation")}</span></span>
                        </span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.capacity_tooltip")}
                    data-tooltip={$_("walletDetail.capacity_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-container segmented">
                    <div class="progress-bar seg-used"    style="width: {usedSeg}%"></div>
                    <div class="progress-bar seg-vouched" style="width: {vouchedSeg}%"></div>
                    <div class="progress-bar seg-free"    style="flex: 1"></div>
                </div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon slashed" aria-hidden="true">gavel</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.total_slashed_as_sponsor_label", {default: "Slashed Capacity"})}</span>
                        <span class="metric-value">{formatNumber(wallet.total_slashed_as_sponsor || 0, 2)}</span>
                        <span class="metric-value metric-value--small">{formatNumber(lockedToOthers, 2)} <span class="metric-sub">{$_("walletDetail.capacity_given")}</span></span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.slashed_tooltip")}
                    data-tooltip={$_("walletDetail.slashed_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-spacer"></div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon community" aria-hidden="true">groups</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.community_standing_label", {default: "Community Standing"})}</span>
                        <span class="metric-value trial-value">
                            <span class="trial-part">{witnessCount} <span class="trial-sub">{$_("walletDetail.failure_witnesses", {default: "witnesses"})}</span></span>
                            <span class="trial-sep">·</span>
                            <span class="trial-part">{formatPercentage(aggregateWitnessRate * 100, 1)} <span class="trial-sub">{$_("walletDetail.community_failure_rate", {default: "community rate"})}</span></span>
                        </span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.community_standing_tooltip")}
                    data-tooltip={$_("walletDetail.community_standing_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-container">
                    <div class="progress-bar community" style="width: {Math.min(100, (aggregateWitnessRate / FAILURE_TOLERANCE) * 100)}%"></div>
                </div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon risk" aria-hidden="true">shield</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.risk_score_label", {default: "Risk Score"})}</span>
                        <span class="metric-value">{formatPercentage(riskScore * 100, 1)}</span>
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.risk_score_tooltip")}
                    data-tooltip={$_("walletDetail.risk_score_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-container">
                    <div class="progress-bar risk" style="width: {(riskScore * 100).toFixed(2)}%"></div>
                </div>
            </div>

            <div class="metric-item card elevation-z1">
                <div class="metric-header">
                    <i class="material-icons metric-icon evidence" aria-hidden="true">history_edu</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.trust_evidence_label", {default: "Trust Evidence"})}</span>
                        <span class="metric-value trial-value">
                            <span class="trial-part">{claim ? formatNumber(claim.cumulative_stats.total_amount_transferred, 2) : '0.00'} <span class="trial-sub">{$_("walletDetail.evidence_transferred")}</span></span>
                            <span class="trial-sep">·</span>
                            <span class="trial-part">{claim ? claim.distinct_counterparties : 0} <span class="trial-sub">{$_("walletDetail.evidence_peers")}</span></span>
                        </span>
                        {#if claim && claim.cumulative_stats.total_amount_expired > 0}
                            <span class="metric-sub error-text">
                                {formatNumber(claim.cumulative_stats.total_amount_expired, 2)} {$_("walletDetail.evidence_expired")}
                            </span>
                        {/if}
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.trust_evidence_tooltip")}
                    data-tooltip={$_("walletDetail.trust_evidence_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-spacer"></div>
            </div>

            <div class="metric-item card elevation-z1 expiration-card {expirationUrgency}">
                <div class="metric-header">
                    <i class="material-icons metric-icon expiration" aria-hidden="true">alarm</i>
                    <div class="metric-body">
                        <span class="metric-label">{$_("walletDetail.next_expiration_label", {default: "Next Deadline"})}</span>
                        {#if nextExpiration}
                            <span class="metric-value">{formatNumber(nextDueAmount, 2)} <span class="metric-sub">{$_("walletDetail.due_in", {default: "due in"})}</span> {expirationCountdown}</span>
                            <span class="metric-sub">{new Date(nextExpiration).toLocaleDateString(undefined, {month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit'})}</span>
                        {:else}
                            <span class="metric-value none">{$_("walletDetail.no_active_debt", {default: "None"})}</span>
                        {/if}
                    </div>
                </div>
                <div class="metric-help" tabindex="0" role="img"
                    aria-label={$_("walletDetail.next_expiration_tooltip")}
                    data-tooltip={$_("walletDetail.next_expiration_tooltip")}>
                    <i class="material-icons" aria-hidden="true">help_outline</i>
                </div>
                <div class="progress-spacer"></div>
            </div>
        </div>
    </div>
{/if}

<style>
    .wallet-card {
        background: var(--mdc-theme-surface, #fff);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 12px;
        padding: 24px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.05);
        width: 100%;
        box-sizing: border-box;
    }

    :global(.dark-theme) .wallet-card {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
        box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    }

    .card-header {
        margin-bottom: 24px;
        padding-bottom: 16px;
        border-bottom: 1px solid var(--mdc-theme-text-hint-on-background, #f0f0f0);
    }

    .header-actions {
        display: flex;
        align-items: center;
        gap: 8px;
        color: var(--mdc-theme-text-secondary-on-surface);
    }

    :global(.header-actions .mdc-icon-button) {
        width: 48px !important;
        height: 48px !important;
        padding: 0 !important;
        margin: 0 !important;
        display: inline-flex !important;
        align-items: center !important;
        justify-content: center !important;
        color: var(--mdc-theme-text-secondary-on-surface) !important;
    }

    :global(.header-actions .mdc-icon-button .material-icons) {
        font-size: 24px !important;
        width: 24px !important;
        height: 24px !important;
        display: block !important;
        line-height: 24px !important;
        text-align: center !important;
        margin: 0 !important;
        padding: 0 !important;
    }

    :global(.dark-theme) .card-header {
        border-bottom-color: rgba(255, 255, 255, 0.05);
    }

    .owner-label {
        font-size: 0.75rem;
        text-transform: uppercase;
        font-weight: 700;
        letter-spacing: 0.5px;
        color: var(--mdc-theme-primary);
    }

    .address-text {
        font-family: monospace;
        font-size: 1rem;
        color: var(--mdc-theme-on-surface);
        text-overflow: ellipsis;
        overflow: hidden;
        white-space: nowrap;
    }

    .metrics-grid {
        display: grid;
        grid-template-columns: repeat(2, 1fr);
        gap: 16px;
    }

    @media (min-width: 700px) {
        .metrics-grid {
            grid-template-columns: repeat(3, 1fr);
        }
    }

    .metric-item {
        background: var(--mdc-theme-background, #fcfcfc);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0,0,0,0.05));
        border-radius: 8px;
        padding: 16px;
        min-height: 90px;
        box-sizing: border-box;
        position: relative;
    }

    .metric-help {
        position: absolute;
        top: 8px;
        right: 8px;
        /* Use a color that meets WCAG AA against the card background */
        color: var(--mdc-theme-text-secondary-on-surface, #767676);
        cursor: help;
        transition: color 0.2s ease;
        /* Make the div itself focusable with a visible outline for keyboard navigation */
        border-radius: 50%;
        outline-offset: 2px;
    }

    .metric-help:hover,
    .metric-help:focus-within {
        color: var(--mdc-theme-primary);
    }

    /* Keyboard-visible focus ring on the help icon */
    .metric-help:focus-within {
        outline: 2px solid var(--mdc-theme-primary, #6200ee);
    }

    .metric-help i {
        font-size: 16px !important;
    }

    /* Tooltip styles */
    .metric-help::before {
        content: attr(data-tooltip);
        position: absolute;
        bottom: 125%;
        right: 0;
        width: 200px;
        padding: 8px 12px;
        background: #333;
        color: #fff;
        font-size: 0.75rem;
        font-weight: 500;
        line-height: 1.4;
        border-radius: 6px;
        box-shadow: 0 4px 12px rgba(0,0,0,0.25);
        opacity: 0;
        visibility: hidden;
        transition: opacity 0.2s ease, transform 0.2s ease;
        transform: translateY(10px);
        z-index: 100;
        pointer-events: none;
    }

    :global(.dark-theme) .metric-help::before {
        background: #444;
        border: 1px solid rgba(255,255,255,0.1);
    }

    /* Tooltip visible on hover OR keyboard focus */
    .metric-help:hover::before,
    .metric-help:focus-within::before {
        opacity: 1;
        visibility: visible;
        transform: translateY(0);
    }

    /* Tooltip arrow */
    .metric-help::after {
        content: '';
        position: absolute;
        bottom: 105%;
        right: 10px;
        border: 6px solid transparent;
        border-top-color: #333;
        opacity: 0;
        visibility: hidden;
        transition: opacity 0.2s ease, transform 0.2s ease;
        transform: translateY(10px);
        z-index: 100;
        pointer-events: none;
    }

    :global(.dark-theme) .metric-help::after {
        border-top-color: #444;
    }

    /* Tooltip arrow visible on hover OR keyboard focus */
    .metric-help:hover::after,
    .metric-help:focus-within::after {
        opacity: 1;
        visibility: visible;
        transform: translateY(0);
    }

    .metric-header {
        display: flex;
        flex-direction: row;
        align-items: center;
    }

    .metric-body {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-width: 0;
    }

    :global(.dark-theme) .metric-item {
        background: rgba(255, 255, 255, 0.03);
        border-color: rgba(255, 255, 255, 0.05);
    }

    :global(.metric-icon) {
        display: inline-block !important;
        margin-right: 8px !important;
        margin-left: 0 !important;
        padding: 0 !important;
        font-size: 20px !important;
        width: 20px !important;
        height: 20px !important;
        line-height: 20px !important;
        flex-shrink: 0;
    }

    :global(.metric-icon.debt) { color: var(--mdc-theme-error, #d32f2f) !important; }
    :global(.metric-icon.trust) { color: var(--mdc-theme-primary, #7b1fa2) !important; }
    :global(.metric-icon.vitality) { color: var(--mdc-theme-secondary, #388e3c) !important; }
    :global(.metric-icon.capacity) { color: #1976d2 !important; }
    :global(.metric-icon.trial) { color: #f57c00 !important; }
    :global(.metric-icon.slashed) { color: var(--mdc-theme-error, #f44336) !important; }
    :global(.metric-icon.community) { color: #00796b !important; }
    :global(.metric-icon.risk) { color: #e65100 !important; }
    :global(.metric-icon.evidence) { color: #795548 !important; }
    :global(.metric-icon.expiration) { color: var(--mdc-theme-text-secondary-on-surface) !important; }

    .error-text {
        color: var(--mdc-theme-error, #d32f2f);
        font-weight: 600;
    }

    .expiration-card.info :global(.metric-icon.expiration) { color: var(--mdc-theme-primary) !important; }
    .expiration-card.warning :global(.metric-icon.expiration) { color: #fbc02d !important; }
    .expiration-card.error :global(.metric-icon.expiration) { color: var(--mdc-theme-error) !important; }

    .metric-value.none {
        /* #767676 on white = 4.54:1 contrast ratio — meets WCAG AA for normal text.
           The previous #ccc was ~1.6:1 which fails WCAG AA entirely. */
        color: var(--mdc-theme-text-secondary-on-surface, #767676);
        font-weight: 500;
        font-size: 1.1rem;
    }

    .expiration-card.warning {
        border-color: rgba(251, 192, 45, 0.4);
        background: rgba(251, 192, 45, 0.05);
    }
    .expiration-card.error {
        border-color: rgba(211, 47, 47, 0.4);
        background: rgba(211, 47, 47, 0.05);
        animation: pulse-border 2s infinite;
    }

    @keyframes pulse-border {
        0% { border-color: rgba(211, 47, 47, 0.4); }
        50% { border-color: rgba(211, 47, 47, 0.8); }
        100% { border-color: rgba(211, 47, 47, 0.4); }
    }

    .metric-label {
        font-size: 0.7rem;
        text-transform: uppercase;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        font-weight: 600;
    }

    .metric-value {
        display: block;
        font-size: 1.25rem;
        font-weight: 700;
        color: var(--mdc-theme-on-surface);
        margin-top: 2px;
    }

    .metric-value--small {
        font-size: 0.95rem;
        margin-top: 1px;
    }

    .trial-value {
        display: flex;
        flex-direction: row;
        align-items: baseline;
        gap: 6px;
        flex-wrap: wrap;
        font-size: 1rem;
    }

    .trial-part {
        display: inline-flex;
        align-items: baseline;
        gap: 3px;
    }

    .trial-sub {
        font-size: 0.65rem;
        font-weight: 600;
        text-transform: uppercase;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
        letter-spacing: 0.3px;
    }

    .trial-sep {
        color: var(--mdc-theme-text-hint-on-background, #ccc);
        font-weight: 400;
    }

    .progress-container {
        height: 6px;
        background: rgba(0, 0, 0, 0.05);
        border-radius: 3px;
        margin-top: 12px;
        overflow: hidden;
    }

    :global(.dark-theme) .progress-container {
        background: rgba(255, 255, 255, 0.1);
    }

    .progress-bar {
        height: 100%;
        border-radius: 3px;
        transition: width 0.5s ease-out;
    }

    .progress-bar.debt { background: var(--mdc-theme-error, #f44336); }
    .progress-bar.trust { background: var(--mdc-theme-primary, #9c27b0); }
    .progress-bar.vitality { background: var(--mdc-theme-secondary, #4caf50); }
    .progress-bar.community { background: #00796b; }
    .progress-bar.risk { background: #e65100; }

    /* Segmented capacity bar */
    .progress-container.segmented {
        display: flex;
        flex-direction: row;
        gap: 1px;
    }
    .progress-bar.seg-used    { background: var(--mdc-theme-error, #f44336); }
    .progress-bar.seg-vouched { background: #1976d2; }
    .progress-bar.seg-free    { background: rgba(0, 0, 0, 0.08); min-width: 0; }

    /* Invisible spacer keeps card height equal to cards that have a progress bar */
    .progress-spacer {
        height: 6px;
        margin-top: 12px;
    }

    .metric-secondary {
        display: block;
        font-size: 0.85rem;
        font-weight: 600;
        color: var(--mdc-theme-text-secondary-on-surface, #555);
        margin-top: 2px;
    }

    .metric-sub {
        font-size: 0.65rem;
        font-weight: 600;
        text-transform: uppercase;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
        letter-spacing: 0.3px;
    }

</style>
