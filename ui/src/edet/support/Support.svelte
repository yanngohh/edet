<script lang="ts">
    /**
     * Support circle: who your sales clear debts for, and who lists you.
     * Configuring the breakdown is an action — it opens behind the FAB
     * (→ EditSupportShares). Approving a supporter stays inline: it is a
     * per-row decision on a list you are already reading.
     *
     * **What that decision is, since it was measured** a
     * drain cannot cost the member drained toward any money, and it is not what
     * keeps strangers out — the cap does that. What it costs is the standing
     * those obligations would have conferred, because routing a claim is a
     * debtor swap and a stake is only ever placed by a transition the creditor
     * signed. Calling the approval a moderation gate against
     * unwanted coupling, which is the smaller half of it and was the paper's
     * wrong reason. The trade is relief now against standing later, and the
     * member cannot weigh it without seeing which of their own obligations are
     * in reach — which is what `lib/support.ts` computes.
     */
    import { _ } from 'svelte-i18n';
    import Button, { Label } from '@smui/button';
    import Fab from '@smui/fab';
    import { Icon } from '@smui/common';

    import MemberChip from '../../components/MemberChip.svelte';
    import Explain from '../../components/Explain.svelte';
    import EditSupportShares from './EditSupportShares.svelte';
    import { membersList, myMember, paramsView } from '../../lib/node';
    import { currentActorId } from '../../lib/actors';
    import { resumeActorChoice } from '../../common/onboardingStore';
    import { memberHue } from '../../lib/display';
    import { tx } from '../../lib/api';
    import { EMPTY_REACH, reachBySupporter, supportPrompt } from '../../lib/support';
    import { send } from '../../lib/submit';
    import { formatNumber } from '../../common/functions';

    let editing = false;
    let busy = false;

    $: me = $currentActorId;
    $: mine = $myMember;
    /**
     * **Absent is not empty.** `beneficiaries` and `supporters` sit behind
     * `full_access` in `views.rs`, so a read the node did not answer with the
     * viewer credential omits them entirely — and `?? []` then rendered
     * "Nobody lists you yet" over a listing that exists. The page said a thing
     * it did not know, on the one screen whose whole subject is who has asked
     * something of you. `reachUnknown` two lines down already refuses this
     * merge for the amount; the lists themselves did not.
     */
    $: relationshipsVisible = mine?.supporters !== undefined;
    // Self first, then the circle — the 0.5-style breakdown order.
    $: viewBens = [...(mine?.beneficiaries ?? [])].sort((a, b) =>
        a.member === me ? -1 : b.member === me ? 1 : a.member - b.member,
    );
    $: viewTotal = viewBens.reduce((s, b) => s + b.weight, 0);
    $: addressById = new Map($membersList.map((m) => [m.id, m.address]));
    // What each supporter's sale could take out of this member's own book —
    // the ledger's selection rule (Active only, oldest first, up to the cap),
    // mirrored so the row can name the amount the decision turns on.
    $: reach = reachBySupporter(mine, $paramsView?.dust ?? 0);
    // Resolved in the script rather than the markup: `{@const}` may only be an
    // immediate child of a block in Svelte 4, and the row needs both the amount
    // and which of the four sentences applies to it.
    $: prompts = new Map(
        (mine?.supporters ?? []).map((s) => [s.member, supportPrompt(reach.get(s.member) ?? EMPTY_REACH, s.drain_cap)]),
    );

    function hueOf(member: number): number {
        return memberHue(addressById.get(member) ?? null, member);
    }

    async function setApproval(supporter: number, approved: boolean) {
        if (me === null) return;
        busy = true;
        try {
            await send(tx.approveSupporter(me, supporter, approved));
        } finally {
            busy = false;
        }
    }
</script>

{#if editing}
    <EditSupportShares on:close={() => (editing = false)} />
{:else}
    <div class="main-container flex-column">
        <Explain summary={$_('support.summary', { default: 'Every sale you make is split by these shares.' })}>
            {$_('support.intro', {
                default:
                    'Your own share clears your debts (your buyer takes them over), the rest clears the debts of the people you list. Listing needs no consent; draining does: each beneficiary must approve you as a supporter.',
            })}
        </Explain>

        <h3 class="section-title">{$_('support.beneficiariesTitle', { default: 'How your sales are shared' })}</h3>
        <Explain
            summary={$_('support.selfEarnsNothingSummary', {
                default: 'Your own share clears your debts without earning you standing.',
            })}
        >
            {$_('support.selfEarnsNothing', {
                default:
                    'Your buyer takes them over, and only a debt you settle with the person you owe writes anything behind you.',
            })}
        </Explain>
        {#if !relationshipsVisible}
            <p class="empty">
                {$_('support.sharesUnknown', {
                    default: 'Your breakdown is not visible here — this node answered without your credential.',
                })}
            </p>
        {:else if viewBens.length === 0}
            <div class="edge-list">
                <!-- No listing on the ledger: the implicit breakdown is 100% you.
                     Only reachable once the lists ARE visible: rendering it from
                     an absent field claimed "everything clears you", which is a
                     statement about the ledger and not about this reader. -->
                <div class="edge-row">
                    <span class="swatch" style={`background: hsl(${me === null ? 0 : hueOf(me)}, 62%, 52%);`}></span>
                    {#if me !== null}<MemberChip memberId={me} detail={$_('community.you', { default: 'you' })} />{/if}
                    <span class="weight">100%</span>
                    <span class="pill you">{$_('support.selfPill', { default: 'clears your debts' })}</span>
                </div>
            </div>
        {:else}
            <div
                class="share-bar"
                role="img"
                aria-label={$_('support.sharesBar', { default: 'Distribution of your support shares' })}
            >
                {#each viewBens as b (b.member)}
                    <div
                        class="share-seg"
                        style={`width: ${viewTotal > 0 ? (b.weight / viewTotal) * 100 : 0}%; background: hsl(${hueOf(b.member)}, 62%, 52%);`}
                        title={`${Math.round(viewTotal > 0 ? (b.weight / viewTotal) * 100 : 0)}%`}
                    ></div>
                {/each}
            </div>
            <div class="edge-list">
                {#each viewBens as b (b.member)}
                    <div class="edge-row">
                        <span class="swatch" style={`background: hsl(${hueOf(b.member)}, 62%, 52%);`}></span>
                        <MemberChip memberId={b.member} detail={b.member === me ? $_('community.you', { default: 'you' }) : ''} />
                        <span class="weight">
                            {viewTotal > 0 ? Math.round((b.weight / viewTotal) * 100) : 0}%
                        </span>
                        {#if b.member === me}
                            <span class="pill you">{$_('support.selfPill', { default: 'clears your debts' })}</span>
                        {:else}
                            <span class="pill" class:ok={b.approved} class:pending={!b.approved}>
                                {b.approved
                                    ? $_('support.approved', { default: 'approved' })
                                    : $_('support.awaiting', { default: 'awaiting their approval' })}
                            </span>
                            <!-- The share is a weight; THIS is what the edge can
                                 actually carry. Zero for a pair that has never
                                 settled anything, approved or not — so an
                                 approval alone is not a working route, and the
                                 page must not imply it is. -->
                            <span class="pill" class:pending={!b.drain_cap}>
                                {b.drain_cap
                                    ? $_('support.capUpTo', {
                                          values: { amount: formatNumber(b.drain_cap, 2) },
                                          default: `up to ${formatNumber(b.drain_cap, 2)}`,
                                      })
                                    : $_('support.capNone', { default: 'no route yet — settle a trade between you' })}
                            </span>
                        {/if}
                    </div>
                {/each}
            </div>
        {/if}

        <h3 class="section-title">{$_('support.supportersTitle', { default: 'People supporting you' })}</h3>
        <Explain
            summary={$_('support.supportersSummary', {
                default: 'These members list you: their sales can clear your debts, once you approve them.',
            })}
        >
            {$_('support.supportersIntro', {
                default:
                    'Their sales clear your debts oldest first, and nothing drains through until you approve. Approving is a real trade, not a formality. A debt cleared for you is discharged without earning you anything, while paying it yourself is the one act that builds your standing and with it your credit limit — so the choice is relief now against standing later, and it is yours alone. Approval is also not enough to make a route: an edge carries at most what the two of you have staked in each other, which is nothing until you have traded and settled.',
            })}
        </Explain>
        {#if !relationshipsVisible}
            <p class="empty">
                {$_('support.supportersUnknown', {
                    default:
                        'Who lists you is not visible here — this node answered without your credential. Anyone supporting you is still waiting on your approval.',
                })}
            </p>
        {:else if (mine?.supporters ?? []).length === 0}
            <p class="empty">{$_('support.noSupporters', { default: 'Nobody lists you yet.' })}</p>
        {:else}
            <div class="edge-list">
                {#each mine?.supporters ?? [] as s (s.member)}
                    <div class="edge-row">
                        <MemberChip memberId={s.member} />
                        <span class="weight">{formatNumber(s.weight, 2)}</span>
                        <!-- The quantity the decision turns on, and the page had
                             no way to see it: what this supporter's next sale
                             could take out of your own book, and therefore the
                             standing that settlement would have written. "No
                             route" and "nothing to clear" are different facts
                             and are never merged. -->
                        <span class="reach">
                            {#if prompts.get(s.member) === 'trade-off'}
                                {$_('support.reachClears', {
                                    values: { amount: formatNumber(reach.get(s.member)?.clears ?? 0, 2) },
                                    default: `could clear up to ${formatNumber(reach.get(s.member)?.clears ?? 0, 2)} of what you owe — standing you would have earned by paying it yourself`,
                                })}
                            {:else if prompts.get(s.member) === 'nothing-to-clear'}
                                {$_('support.reachNothing', { default: 'you owe nothing they could clear right now' })}
                            {:else if prompts.get(s.member) === 'unknown'}
                                <!-- The book was not served to this reader.
                                     "Cannot say" is the honest answer and it is
                                     not the same sentence as "nothing". -->
                                {$_('support.reachUnknown', { default: 'your obligations are not visible here' })}
                            {/if}
                            <!-- `no-route` deliberately says NOTHING here. It is
                                 the same fact the cap pill beside it carries,
                                 and a row with no edge was printing that one
                                 sentence twice, side by side. -->
                        </span>
                        <!-- The cap belongs to the EDGE, not to the approval, so
                             it shows either way: approving does not create it and
                             revoking does not take it away. It used to appear
                             only once approved, which read as though the approval
                             were what opened the route. -->
                        <span class="pill" class:pending={!s.drain_cap}>
                            {s.drain_cap
                                ? $_('support.capUpTo', {
                                      values: { amount: formatNumber(s.drain_cap, 2) },
                                      default: `up to ${formatNumber(s.drain_cap, 2)}`,
                                  })
                                : $_('support.capNone', { default: 'no route yet — settle a trade between you' })}
                        </span>
                        {#if s.approved}
                            <span class="pill ok">{$_('support.approved', { default: 'approved' })}</span>
                            <Button variant="outlined" disabled={busy} on:click={() => setApproval(s.member, false)}>
                                <Label>{$_('support.revoke', { default: 'Revoke' })}</Label>
                            </Button>
                        {:else}
                            <span class="pill pending">{$_('support.pending', { default: 'pending' })}</span>
                            <Button variant="raised" disabled={busy} on:click={() => setApproval(s.member, true)}>
                                <Label>{$_('support.approve', { default: 'Approve' })}</Label>
                            </Button>
                        {/if}
                    </div>
                {/each}
            </div>
        {/if}
    </div>

    <!-- Same rule as the trade FAB: offered to everyone, and the first
         signature is what asks for an identity. -->
    <Fab
        color="primary"
        class="fab-edit"
        aria-label={$_('support.edit', { default: 'Edit shares' })}
        title={$_('support.edit', { default: 'Edit shares' })}
        on:click={() => (me === null ? resumeActorChoice() : (editing = true))}
    >
        <Icon class="material-icons">edit</Icon>
    </Fab>
{/if}

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
    .share-bar {
        display: flex;
        height: 14px;
        border-radius: 999px;
        overflow: hidden;
        background: var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.08));
    }
    .share-seg {
        height: 100%;
        transition: width 120ms ease;
    }
    .swatch {
        width: 12px;
        height: 12px;
        border-radius: 3px;
        flex-shrink: 0;
    }
    .edge-list {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .edge-row {
        display: flex;
        align-items: center;
        gap: 14px;
        padding: 10px 14px;
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 8px;
        background: var(--mdc-theme-surface, #fff);
        flex-wrap: wrap;
    }
    :global(.dark-theme) .edge-row {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }
    .weight {
        font-weight: 600;
        min-width: 48px;
    }
    .pill {
        font-size: 0.72rem;
        padding: 2px 10px;
        border-radius: 999px;
        border: 1px solid currentColor;
    }
    .pill.ok { color: #2e7d32; }
    .pill.pending { color: #f9a825; }
    .pill.you { color: #1565c0; }
    :global(.dark-theme) .pill.ok { color: #81c784; }
    :global(.dark-theme) .pill.pending { color: #ffe082; }
    :global(.dark-theme) .pill.you { color: #64b5f6; }
    .empty {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .reach {
        flex-basis: 100%;
        font-size: 0.8rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
</style>
