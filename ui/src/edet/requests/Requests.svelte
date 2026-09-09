<script lang="ts">
    import { _ } from 'svelte-i18n';
    import Button, { Label } from '@smui/button';

    import MemberChip from '../../components/MemberChip.svelte';
    import Explain from '../../components/Explain.svelte';
    import OfferIntake from '../../components/OfferIntake.svelte';
    import { membersList, paramsView, pendingView } from '../../lib/node';
    import { currentActorId } from '../../lib/actors';
    import { memberName } from '../../lib/display';
    import { epochDate } from '../../lib/epoch';
    import { checkPending, declinePending, rejectionMessage, signPending } from '../../lib/submit';
    import { autoDecisions, scoredDebtor } from '../../lib/autosign';
    import { acceptancePolicy } from '../../lib/policy';
    import { riskOf } from '../../lib/risk';
    import { hasRule, subjectivePricing, subjectiveRiskOf } from '../../lib/pricing';
    import { txSummary } from '../../lib/txSummary';
    import { formatDateTime, formatNumber } from '../../common/functions';
    import { partyKeyHex, partyMember, samePartyAs, type PendingEntryView } from '../../lib/api';

    let busy = false;
    // Review dry-runs, keyed by digest, refreshed lazily per entry.
    let checks: Record<string, { ok: boolean; code?: string }> = {};

    $: me = $currentActorId;
    $: awaiting = $pendingView?.awaiting_me ?? [];
    $: mine = $pendingView?.mine ?? [];
    $: summarize = (e: PendingEntryView) =>
        txSummary(e.tx, { t: $_, nameOf: $memberName, fmt: (n, d) => formatNumber(n, d ?? 2), dateOf: $epochDate });

    // The same score the acceptance rule read when it left this one to you.
    $: scoreOf = (e: PendingEntryView): number | null => {
        if (me === null) return null;
        const debtor = scoredDebtor(e.tx, me);
        if (debtor === null) return null;
        const row = $membersList.find((m) => m.id === debtor);
        // riskOf itself returns null both for "no row" and for "row present
        // but its risk fields are hidden" (authenticated reads) —
        // either way this review score is unavailable, never guessed.
        return riskOf(row, $paramsView);
    };
    // **The member's own price, where they have set one** — shown BESIDE the
    // ledger's rather than in place of it. The protocol's score is the
    // community's reading; a client that replaced it silently would be
    // presenting a private judgement as a public fact.
    $: mineOf = (e: PendingEntryView): number | null => {
        if (me === null) return null;
        const debtor = scoredDebtor(e.tx, me);
        if (debtor === null || !hasRule($subjectivePricing, debtor)) return null;
        const row = $membersList.find((m) => m.id === debtor);
        if (!row || row.capacity === undefined || row.debt === undefined) return null;
        if (row.d_in === undefined || row.d_out === undefined) return null;
        return subjectiveRiskOf(debtor, row as never, $paramsView, $subjectivePricing);
    };
    // Held above the decline threshold means the cold-start guard kept it:
    // a counterparty with no settled history is never declined by machine.
    $: heldAsNewcomer = (r: number) => $acceptancePolicy.auto && r >= $acceptancePolicy.reject;
    $: pct = (r: number) => Math.round(r * 100);

    $: for (const e of awaiting) {
        if (!(e.digest in checks)) {
            checks[e.digest] = { ok: true };
            void checkPending(e).then((r) => {
                checks = { ...checks, [e.digest]: r };
            });
        }
    }

    /**
     * Who this request is still waiting on.
     *
     * A required party may be a KEY rather than a member — the
     * newcomer whose account this very trade will seat — and there is no name
     * to look up for somebody the ledger has not met. They are shown by the
     * short form of their key, which is the only name they have.
     */
    function waitingOn(e: PendingEntryView): string {
        return e.required
            .filter((p) => !e.signed_by.some((s) => samePartyAs(s, p)))
            .map((p) => {
                const id = partyMember(p);
                if (id !== null) return $memberName(id);
                const hex = partyKeyHex(p) ?? '';
                // One expression for the short form, used as the value AND
                // inside the default: written twice, the inline copy carried
                // the ellipsis as literal text where `en.json` carries it
                // inside `{key}`, and the drift gate reads the fixed text.
                const short = `${hex.slice(0, 8)}…`;
                return $_('requests.newcomer', {
                    values: { key: short },
                    default: `a new account (${short})`,
                });
            })
            .join(', ');
    }

    /**
     * The short form of the key that opened this entry on my invitation, or
     * null when a member opened it. One expression for the value AND the
     * default, for the same reason `waitingOn` has one: the ellipsis lives
     * inside `{key}` in `en.json`, and the drift gate reads the fixed text.
     */
    function openedByKey(e: PendingEntryView): string | null {
        const hex = e.opener ? partyKeyHex(e.opener) : null;
        return hex === null ? null : `${hex.slice(0, 8)}…`;
    }

    async function sign(e: PendingEntryView) {
        busy = true;
        try {
            await signPending(e);
        } finally {
            busy = false;
        }
    }

    async function decline(e: PendingEntryView) {
        busy = true;
        try {
            await declinePending(e);
        } finally {
            busy = false;
        }
    }
</script>

<div class="main-container flex-column">
    <Explain summary={$_('requests.summary', { default: 'Transactions that need more than one signature meet here.' })}>
        {$_('requests.intro', {
            default:
                'What others ask of you, and what you sent that still waits on them. Nothing binds until every required party has signed from their own device.',
        })}
    </Explain>

    {#if !$acceptancePolicy.auto}
        <Explain
            tone="notice"
            icon="tune"
            summary={$_('requests.autoOffSummary', { default: 'Your acceptance rule is off, so every request waits here.' })}
        >
            {$_('requests.autoOff', {
                default: 'Turn it on from My Wallet to have safe purchases signed for you.',
            })}
        </Explain>
    {/if}

    <h3 class="section-title">{$_('requests.awaitingTitle', { default: 'Awaiting your signature' })}</h3>
    <OfferIntake />
    {#if awaiting.length === 0}
        <p class="empty">{$_('requests.noneAwaiting', { default: 'Nothing waits on you.' })}</p>
    {:else}
        <div class="list">
            {#each awaiting as e (e.digest)}
                <div class="request-card">
                    <div class="request-head">
                        <span class="summary">{summarize(e)}</span>
                    </div>
                    <div class="request-meta">
                        <span class="from">
                            {$_('requests.from', { default: 'from' })}
                            {#if openedByKey(e) !== null}
                                <!-- Opened by a key on this member's invitation: the
                                     newcomer's own first purchase, charged to me. -->
                                {$_('requests.newcomer', {
                                    values: { key: openedByKey(e) },
                                    default: `a new account (${openedByKey(e)})`,
                                })}
                            {:else}
                                <MemberChip memberId={e.initiator} size={20} />
                            {/if}
                        </span>
                        {#if scoreOf(e) !== null}
                            <span class="risk-pill">
                                {#if heldAsNewcomer(scoreOf(e) ?? 0)}
                                    {$_('requests.heldNewcomer', {
                                        values: { risk: pct(scoreOf(e) ?? 0) },
                                        default: `risk ${pct(scoreOf(e) ?? 0)}% — no settled history yet, so this one is yours to judge`,
                                    })}
                                {:else if $acceptancePolicy.auto}
                                    {$_('requests.heldBand', {
                                        values: {
                                            risk: pct(scoreOf(e) ?? 0),
                                            accept: pct($acceptancePolicy.accept),
                                            reject: pct($acceptancePolicy.reject),
                                        },
                                        default: `risk ${pct(scoreOf(e) ?? 0)}% — inside your review band ${pct($acceptancePolicy.accept)}–${pct($acceptancePolicy.reject)}%`,
                                    })}
                                {:else}
                                    {$_('requests.riskOnly', {
                                        values: { risk: pct(scoreOf(e) ?? 0) },
                                        default: `risk ${pct(scoreOf(e) ?? 0)}%`,
                                    })}
                                {/if}
                            </span>
                        {/if}
                        {#if mineOf(e) !== null}
                            <span class="risk-pill">
                                {$_('requests.yourPrice', {
                                    values: { risk: pct(mineOf(e) ?? 0) },
                                    default: `your rule: ${pct(mineOf(e) ?? 0)}%`,
                                })}
                            </span>
                        {/if}
                        <span class="digest mono">{e.digest.slice(0, 12)}…</span>
                    </div>
                    {#if checks[e.digest] && !checks[e.digest].ok}
                        <p class="invalid-note" role="alert">
                            {$_('requests.invalidNow', {
                                values: { reason: rejectionMessage(checks[e.digest].code ?? 'ET-UNKNOWN') },
                                default: `As things stand this would be rejected: ${rejectionMessage(checks[e.digest].code ?? 'ET-UNKNOWN')}`,
                            })}
                        </p>
                    {/if}
                    <div class="actions">
                        <Button variant="raised" disabled={busy} on:click={() => sign(e)}>
                            <Label>{$_('requests.sign', { default: 'Sign' })}</Label>
                        </Button>
                        <Button variant="outlined" disabled={busy} on:click={() => decline(e)}>
                            <Label>{$_('requests.decline', { default: 'Decline' })}</Label>
                        </Button>
                    </div>
                </div>
            {/each}
        </div>
    {/if}

    <h3 class="section-title">{$_('requests.mineTitle', { default: 'Waiting on others' })}</h3>
    {#if mine.length === 0}
        <p class="empty">{$_('requests.noneMine', { default: 'No open requests of yours.' })}</p>
    {:else}
        <div class="list">
            {#each mine as e (e.digest)}
                <div class="request-card mine">
                    <div class="request-head">
                        <span class="summary">{summarize(e)}</span>
                    </div>
                    <div class="request-meta">
                        <span>
                            {$_('requests.waitingOn', {
                                values: { names: waitingOn(e) },
                                default: `Waiting on: ${waitingOn(e)}`,
                            })}
                        </span>
                        <span class="digest mono">{e.digest.slice(0, 12)}…</span>
                    </div>
                    <div class="actions">
                        {#if me !== null && (e.initiator === me || e.required.some((p) => partyMember(p) === me))}
                            <Button variant="outlined" disabled={busy} on:click={() => decline(e)}>
                                <Label>{$_('requests.withdraw', { default: 'Withdraw' })}</Label>
                            </Button>
                        {/if}
                    </div>
                </div>
            {/each}
        </div>
    {/if}

    {#if $autoDecisions.length > 0}
        <h3 class="section-title">{$_('requests.autoTitle', { default: 'Decided by your rule' })}</h3>
        <div class="list">
            {#each $autoDecisions as d (d.digest)}
                <div class="request-card auto" class:declined={d.band === 'reject'}>
                    <div class="request-head">
                        <span class="summary">{txSummary(d.tx, { t: $_, nameOf: $memberName, fmt: (n, dd) => formatNumber(n, dd ?? 2), dateOf: $epochDate })}</span>
                    </div>
                    <div class="request-meta">
                        <span class="risk-pill">
                            {d.band === 'accept'
                                ? $_('requests.autoSigned', {
                                      values: { risk: pct(d.risk) },
                                      default: `signed for you — risk ${pct(d.risk)}%`,
                                  })
                                : $_('requests.autoDeclined', {
                                      values: { risk: pct(d.risk) },
                                      default: `declined for you — risk ${pct(d.risk)}%`,
                                  })}
                        </span>
                        <!-- The list survives a restart, which is the point:
                             the member was elsewhere while it was written. A
                             date is what makes it readable afterwards. -->
                        <span class="when">{formatDateTime(d.at)}</span>
                        <span class="digest mono">{d.digest.slice(0, 12)}…</span>
                    </div>
                </div>
            {/each}
        </div>
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
    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin: 8px 0 0;
        font-weight: 500;
    }
    .list {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .request-card {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-left: 4px solid #1565c0;
        border-radius: 8px;
        padding: 12px 16px;
        background: var(--mdc-theme-surface, #fff);
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    /* The three theme sides, plus the BASE edge — `#1565c0` is unreadable on
       #1e1e1e. This ties on specificity with the status modifiers below and
       loses to them on source order, so `mine`, `auto` and `declined` still
       show; only the default edge is re-themed. */
    :global(.dark-theme) .request-card {
        background: #1e1e1e;
        border-top-color: rgba(255, 255, 255, 0.1);
        border-right-color: rgba(255, 255, 255, 0.1);
        border-bottom-color: rgba(255, 255, 255, 0.1);
        border-left-color: #64b5f6;
    }
    .request-card.mine {
        border-left-color: var(--mdc-theme-text-secondary-on-surface, #9e9e9e);
        opacity: 0.92;
    }
    .request-card.auto {
        border-left-color: #2e7d32;
        opacity: 0.9;
    }
    .request-card.auto.declined {
        border-left-color: #c62828;
    }
    .risk-pill {
        font-size: 0.75rem;
        padding: 2px 10px;
        border-radius: 999px;
        border: 1px solid currentColor;
        color: var(--mdc-theme-text-secondary-on-surface, #777);
        white-space: nowrap;
    }
    .summary {
        font-weight: 600;
        line-height: 1.45;
    }
    .request-meta {
        display: flex;
        gap: 16px;
        align-items: center;
        flex-wrap: wrap;
        font-size: 0.85rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .from {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .digest {
        margin-left: auto;
    }
    .when {
        font-variant-numeric: tabular-nums;
        opacity: 0.75;
    }
    .mono {
        font-family: monospace;
    }
    .invalid-note {
        margin: 0;
        font-size: 0.85rem;
        color: #b71c1c;
        background: rgba(211, 47, 47, 0.06);
        border-left: 3px solid rgba(211, 47, 47, 0.5);
        padding: 6px 10px;
        border-radius: 0 4px 4px 0;
    }
    :global(.dark-theme) .invalid-note {
        color: #ef9a9a;
        background: rgba(211, 47, 47, 0.12);
    }
    .actions {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }
    .empty {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
</style>
