<script lang="ts">
    import { _ } from 'svelte-i18n';
    import { onDestroy, onMount } from 'svelte';
    import Button, { Label } from '@smui/button';
    import CircularProgress from '@smui/circular-progress';
    import Fab from '@smui/fab';
    import { Icon } from '@smui/common';

    import AgentAvatar from '../../components/AgentAvatar.svelte';
    import QrCodeDisplay from '../../components/QrCodeDisplay.svelte';
    import StatCard from '../../components/StatCard.svelte';
    import Explain from '../../components/Explain.svelte';
    import EditWallet from './EditWallet.svelte';
    import PendingKeyWallet from './PendingKeyWallet.svelte';
    import { myMember, networkView, paramsView, firstLoadDone } from '../../lib/node';
    import { allowanceState, hasNoStanding } from '../../lib/allowance';
    import { currentActorId, heldSeed, keyring, pendingIdentity } from '../../lib/actors';
    import { buildPayQrPayload, mintInvite, type Invite } from '../../lib/invite';
    import { chainIdIfDeclared } from '../../lib/submit';
    import { nowUnixSecs } from '../../lib/session';
    import { resumeActorChoice } from '../../common/onboardingStore';
    import { acceptancePolicy } from '../../lib/policy';
    import { copyText, memberName, amountsSealed, formatAmount } from '../../lib/display';
    import { epochDate } from '../../lib/epoch';
    import { formatNumber } from '../../common/functions';
    import { errorStore } from '../../common/errorStore';

    let editing = false;

    /**
     * The "pay me" QR carries a standing invitation to be bought from, so a
     * buyer with no account yet can park their purchase in the pool charged
     * to this member (`lib/invite.ts`). Minted once the seed and the chain
     * are both known — this screen mounts under the first-run wizard, before
     * either is — and again on a timer: it expires in a day and binds this
     * device's signature, so a fresh nonce keeps a captured one short-lived.
     * Without a seed or a declared chain the QR is the bare address, which
     * every buyer with an account reads as before.
     */
    const INVITE_TTL_SECS = 86_400;
    const INVITE_REMINT_MS = 10 * 60 * 1000;
    let invite: Invite | null = null;
    let inviteTimer: ReturnType<typeof setInterval> | null = null;

    function remintInvite() {
        const id = $currentActorId;
        const seed = id === null ? null : heldSeed(id);
        const chain = chainIdIfDeclared();
        invite = seed && chain ? mintInvite(seed, chain, nowUnixSecs() + INVITE_TTL_SECS) : null;
    }

    // Booleans and a string, so the statement re-runs when something it
    // reads actually flips, never on every poll of the network view.
    $: chainKnown = $networkView?.chain_id ?? '';
    $: seedKnown = $keyring !== undefined && $currentActorId !== null && heldSeed($currentActorId) !== null;
    $: if (seedKnown && chainKnown) remintInvite();

    onMount(() => {
        inviteTimer = setInterval(remintInvite, INVITE_REMINT_MS);
    });
    onDestroy(() => {
        if (inviteTimer !== null) clearInterval(inviteTimer);
    });

    $: me = $myMember;
    $: qrValue = me ? buildPayQrPayload($networkView?.chain_id ?? '', me.address, invite) : '';
    $: net = $networkView;
    $: loaded = !!me;
    $: dust = net?.dust ?? 0.01;
    // Operation bonds. Absent for a read without full access — treated as
    // unknown throughout (the card and both notes simply do not render)
    // rather than displayed as zero, which would read as "no allowance
    // left" and warn about a state the member is not in.
    $: bondPos = me?.operation_bond;
    $: bondAllowance = $paramsView?.bond_free_allowance;
    // Standing and the write allowance, decided in `lib/allowance.ts` and
    // tested there, because both readings on this page were wrong and neither
    // could be tested here. `conferrable` is the quantity the bond gate reads;
    // this page asked `capacity`, which is a different question and answers
    // ZERO for a founding underwriter.
    $: writeState = allowanceState(me ?? null, dust);
    // Sustained saturation is the precondition for forfeiture: past
    // `bond_forfeit_epochs` consecutive saturated epochs, anyone may crank
    // `ForfeitBonds` and the held reservations stop coming back. This is the
    // one bond state with a permanent consequence, so it gets an alert.
    $: forfeitAfter = $paramsView?.bond_forfeit_epochs;
    $: bondAtRisk = !!bondPos && forfeitAfter !== undefined && bondPos.saturated_epochs >= forfeitAfter;
    // Nobody is backing this account. It is NOT a rejection and must never read
    // as one — an account starts at zero by arithmetic rather than by rule, and
    // the way out is the uninsured tier: somebody takes a first risk, it
    // settles, and the settlement is what confers standing. The same state, and
    // the same advice, for a member whose backing has since lapsed.
    $: bootstrap = hasNoStanding(me ?? null, dust);
    // How many more rows this member's standing carries. `seat_reach` is what
    // the ledger nets a seat out of and `unit` is what one costs, so the count
    // is the ledger's own arithmetic rather than a second rule beside it.
    $: newcomersLeft = bondPos && bondPos.unit > 0 ? Math.floor(bondPos.seat_reach / bondPos.unit) : 0;

    async function copyAddress() {
        if (!me) return;
        if (await copyText(me.address)) {
            errorStore.pushError($_('common.copied', { default: 'Copied to clipboard' }), 'warning');
        }
    }
</script>

{#if editing}
    <EditWallet on:close={() => (editing = false)} />
{:else if $currentActorId === null && $pendingIdentity}
    <!--
        A key exists and the ledger has no row for it yet. That is a wallet
        with real contents — the key, the QR that gets it traded with, and
        the zeroes that are genuinely true of it — not an empty state.
    -->
    <PendingKeyWallet />
{:else if $currentActorId === null}
    <!--
        No key at all: somebody who skipped identity setup. The landing view
        is the first thing they read, so the way out belongs in the page and
        not behind Settings → Identity.
    -->
    <div class="center-container flex-column" style="gap: 12px;">
        <p>{$_('myWallet.noActor', { default: 'A wallet needs a key. This device holds none yet — look around, and set one up when you want to trade.' })}</p>
        <Button variant="raised" on:click={() => resumeActorChoice()}>
            <Label>{$_('app.browsing.cta', { default: 'Set one up' })}</Label>
        </Button>
    </div>
{:else if !loaded && !$firstLoadDone}
    <div class="center-container">
        <CircularProgress class="circular-progress" indeterminate/>
    </div>
{:else if !me}
    <div class="center-container">
        <p>{$_('myWallet.unknownMember', { default: 'This member does not exist on the ledger (anymore).' })}</p>
    </div>
{:else}
    <div class="flex-column main-container">
        <div class="wallet-header" data-tour="wallet-header">
            <AgentAvatar memberId={me.id} size={64} />
            <div class="wallet-id flex-column">
                <span class="wallet-name">{$memberName(me.id)}</span>
                <span class="wallet-sub">
                    {$_('myWallet.memberSince', {
                        values: { date: $epochDate(me.joined_epoch) },
                        default: `member since ${$epochDate(me.joined_epoch)}`,
                    })}
                </span>
                <span class="badges">
                    <span class="pill status-{me.status}">{$_('members.status.' + me.status, { default: me.status })}</span>
                    {#if me.is_validator}
                        <span class="pill validator">{$_('myWallet.validator', { default: 'validator' })}</span>
                    {/if}
                </span>
            </div>
            <!--
                **A row of its own, and not a line in the column beside the
                avatar.** The address is 42 monospace characters; that column
                is the viewport less about 172 px of avatar, padding and copy
                button, so holding all 42 there needs a 7.5 px font on a 360 dp
                handset. On its own row it has the whole card, and the size
                below fits it at every width a phone has.
            -->
            <span class="wallet-address">
                <code>{me.address}</code>
                <button
                    type="button"
                    class="copy-btn"
                    aria-label={$_('common.copy', { default: 'Copy' })}
                    title={$_('common.copy', { default: 'Copy' })}
                    on:click={copyAddress}
                >
                    <i class="material-icons" aria-hidden="true">content_copy</i>
                </button>
            </span>
            <div class="wallet-qr" data-tour="wallet-qr">
                <QrCodeDisplay value={qrValue} size={invite ? 168 : 132} level={invite ? 'L' : 'M'} />
                <span class="qr-hint">
                    {$_('myWallet.qrHint', { default: 'Selling? Have the buyer scan your address.' })}
                </span>
            </div>
        </div>

        <!--
            The single most important sentence in the product.

            A newcomer's capacity is zero, and nothing they can do ALONE will
            change it: it is what the community has put behind them, and the
            community has not met them yet. Presented as a number that says
            "0", that reads as a rejection — and it is not one. It is the
            starting value of a quantity that is earned, and the way it is
            earned is that somebody takes a first, uninsured risk, the trade
            settles, and the settlement confers standing.

            So the banner says what to DO, not what is missing. Never "you have
            no credit".
        -->
        {#if bootstrap}
            <div class="bootstrap-banner flex-row">
                <Icon class="material-icons bootstrap-banner-icon">stars</Icon>
                <span class="bootstrap-banner-text">
                    {$_('myWallet.bootstrapEligible', {
                        default:
                            'Nobody is backing you right now. That is where everybody starts, and where standing goes if trade does not renew it — it is not a refusal. Trade anyway: your obligations are uninsured until somebody carries one, and paying one back is what makes the community stand behind you. Until then your free actions come with the counterparty, so somebody with standing has to be on the trade. The one exception is the action your seat paid for: you can nominate guardians once on your own.',
                    })}
                </span>
            </div>
        {/if}


        {#if (me.open_default ?? 0) > (net?.dust ?? 0.01)}
            <div class="default-note" role="alert">
                <!-- `plain` so the alert keeps its own red and its own size:
                     a tone that repaints would erase the thing that makes
                     this an alert. -->
                <Explain
                    tone="plain"
                    summary={$_('myWallet.openDefaultSummary', {
                        values: { amount: formatNumber(me.open_default ?? 0) },
                        default: `You have an open default of ${formatNumber(me.open_default ?? 0)}.`,
                    })}
                >
                    {$_('myWallet.openDefaultNote', {
                        default:
                            'The backing it drew stays consumed until you cure it, and your free actions are held until then — sell to your creditor; their purchase discharges it.',
                    })}
                </Explain>
            </div>
        {/if}

        {#if bondAtRisk}
            <div class="default-note" role="alert">
                <Explain
                    tone="plain"
                    summary={$_('myWallet.bondForfeitSummary', {
                        values: { epochs: bondPos?.saturated_epochs ?? 0 },
                        default: `Your activity allowance has been fully used for ${bondPos?.saturated_epochs ?? 0} epochs running.`,
                    })}
                >
                    {$_('myWallet.bondForfeitNote', {
                        default:
                            'Amounts set aside this long can be forfeited by anyone — they stop coming back to you. Slow down, or settle debt to free capacity.',
                    })}
                </Explain>
            </div>
        {:else if writeState === 'spent'}
            <div class="probation-note" role="note">
                <Explain
                    tone="plain"
                    summary={$_('myWallet.bondSpentSummary', { default: 'You have used every free action for this epoch.' })}
                >
                    {$_('myWallet.bondSpentNote', {
                        default:
                            'Further actions set aside a small part of what you have to lose until the next epoch — it comes back to you, and nobody receives it. Settling, curing and leaving stay free regardless.',
                    })}
                </Explain>
            </div>
        {/if}

        <div class="metrics-grid" data-tour="wallet-metrics" data-loaded={loaded ? 'true' : 'false'}>
            <!-- ONE card, because the model has one quantity. Two would be
                 an "Available to spend" card beside this one showing
                 `capacity - debt`, and `capacity` is ALREADY net of the credit
                 standing on the member: an insured obligation reserves flow
                 into them, so the cut the node answers with has it subtracted
                 out. Subtracting the debt again under-reported a member's own
                 limit by exactly their insured debt, and presented two numbers
                 for the question this design exists to have one answer to. -->
            <StatCard
                label={$_('myWallet.capacity', { default: 'Capacity' })}
                value={formatAmount(formatNumber, me.capacity, $amountsSealed)}
                icon="speed"
                tone="good"
                help={$_('myWallet.capacityHelp', {
                    default: 'How much debt the community will stand behind for you. It is already net of the credit standing on you, so it is the ceiling on your next insured purchase. You may still buy beyond it — that part is simply not covered.',
                }) + ($amountsSealed ? ' ' + $_('contracts.sealedTitle', { default: 'Rounded up to conceal the exact figure — not the precise amount.' }) : '')}
            />
            <StatCard
                label={$_('myWallet.debt', { default: 'You owe' })}
                value={formatAmount(formatNumber, me.debt, $amountsSealed)}
                icon="trending_down"
                tone={me.debt > 0 ? 'warn' : 'default'}
                help={$_('myWallet.debtHelp', {
                    default: "Your outstanding debt across all contracts. It clears when you sell (your buyer takes it over), when you settle directly, or when a supporter's sale drains into it.",
                }) + ($amountsSealed ? ' ' + $_('contracts.sealedTitle', { default: 'Rounded up to conceal the exact figure — not the precise amount.' }) : '')}
            />
            <StatCard
                label={$_('myWallet.conferrable', { default: 'You may back others up to' })}
                value={formatAmount(formatNumber, me.conferrable ?? 0, $amountsSealed)}
                icon="volunteer_activism"
                help={$_('myWallet.conferrableHelp', {
                    default: 'What you may put behind somebody else. It costs you nothing and it is not a balance you spend — back twenty people and your own capacity is unchanged. Only credit actually outstanding is bounded.',
                })}
            />
            <StatCard
                label={$_('myWallet.backers', { default: 'People backing you' })}
                value={String((me.backers ?? []).length)}
                icon="group"
                help={$_('myWallet.backersHelp', {
                    default: 'Members who have put standing behind you, by carrying and being repaid. Standing is evidence of having owed and paid — it cannot be bought, and selling earns none of it.',
                })}
            />
            {#if bondPos}
                <!-- **A row is a stock and the allowance is a rate**, so the two
                     are separate cards. Bringing somebody in holds a piece of
                     your standing for as long as their account exists and
                     nothing gives it back — what refills every epoch is the
                     card beside this one. Stated in PEOPLE rather than in an
                     amount, because that is what the member is deciding. -->
                <StatCard
                    label={$_('myWallet.newcomers', { default: 'Newcomers you can bring in' })}
                    value={String(newcomersLeft)}
                    icon="person_add"
                    tone={newcomersLeft === 0 ? 'warn' : 'default'}
                    help={$_('myWallet.newcomersHelp', {
                        default: 'How many more people your standing carries into the ledger. Bringing somebody in holds a small part of what backs you for as long as their account exists — it is not a fee, nobody receives it, and unlike your activity allowance it does not come back at the next epoch. More backing on you opens more.',
                    })}
                />
                <StatCard
                    label={$_('myWallet.bondAllowance', { default: 'Activity allowance' })}
                    value={bondAllowance !== undefined
                        ? $_('myWallet.bondAllowanceValue', {
                              values: { left: bondPos.free_remaining, total: bondAllowance },
                              default: `${bondPos.free_remaining} of ${bondAllowance}`,
                          })
                        : String(bondPos.free_remaining)}
                    icon="bolt"
                    tone={bondAtRisk ? 'bad' : writeState === 'ok' ? 'good' : 'warn'}
                    help={$_('myWallet.bondAllowanceHelp', {
                        values: { reserved: formatNumber(bondPos.encumbered) },
                        default: `Free actions left this epoch. Beyond them, each action sets aside a small part of what you have to lose and gives it back a short while later — the ledger charges no fees, so it is never spent and nobody receives it. Currently set aside: ${formatNumber(bondPos.encumbered)}. The allowance comes with standing, so it is zero while nobody is backing you. Settling, curing, transferring and leaving are always free.`,
                    })}
                />
            {/if}
            <StatCard
                label={$_('myWallet.openDefault', { default: 'Open default' })}
                value={formatNumber(me.open_default ?? 0)}
                icon="report"
                tone={(me.open_default ?? 0) > (net?.dust ?? 0.01) ? 'bad' : 'good'}
                help={$_('myWallet.openDefaultHelp', {
                    default: 'Defaulted amount not yet cured. A default does not hand back the backing it drew, so your standing stays consumed until you cure it.',
                })}
            />
        </div>

        <!-- The acceptance rule is a wallet setting, so its current state
             belongs on the wallet; the FAB opens it. -->
        <div class="rule-row">
            <i class="material-icons rule-icon" aria-hidden="true">tune</i>
            {#if $acceptancePolicy.auto}
                <span>
                    {$_('myWallet.ruleOn', {
                        values: {
                            accept: Math.round($acceptancePolicy.accept * 100),
                            reject: Math.round($acceptancePolicy.reject * 100),
                        },
                        default: `Acceptance rule: purchases scoring at or below ${Math.round($acceptancePolicy.accept * 100)}% risk are signed for you, at or above ${Math.round($acceptancePolicy.reject * 100)}% declined, the rest held for your review.`,
                    })}
                </span>
            {:else}
                <span>
                    {$_('myWallet.ruleOff', {
                        default: 'Acceptance rule off: every incoming request waits for your signature.',
                    })}
                </span>
            {/if}
        </div>
    </div>

    <Fab
        color="primary"
        class="fab-edit"
        aria-label={$_('acceptance.title', { default: 'Acceptance rule' })}
        title={$_('acceptance.title', { default: 'Acceptance rule' })}
        on:click={() => (editing = true)}
    >
        <Icon class="material-icons">edit</Icon>
    </Fab>
{/if}

<style>
    .main-container {
        width: 100%;
        margin: 0;
        padding: 16px;
        box-sizing: border-box;
        gap: 16px;
        max-width: var(--edet-column);
    }

    .wallet-header {
        display: flex;
        align-items: center;
        gap: 16px;
        padding: 16px;
        border-radius: 12px;
        background: var(--mdc-theme-surface, #fff);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        flex-wrap: wrap;
    }
    .wallet-qr {
        margin-left: auto;
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 6px;
        max-width: 168px;
    }
    .qr-hint {
        font-size: 0.75rem;
        text-align: center;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        line-height: 1.35;
    }
    :global(.dark-theme) .wallet-header {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }
    .wallet-name {
        font-size: 1.3rem;
        font-weight: 600;
    }
    .wallet-address {
        /* Its own line in the wrapping header, at any width. */
        flex-basis: 100%;
        display: flex;
        align-items: center;
        gap: 4px;
        min-width: 0;
    }
    .wallet-address code {
        font-family: monospace;
        /* 42 characters at monospace's 0.6em advance is 25.2em, and the row
           has the card's width less about 92 px of padding and copy button. So
           the size that fits is (w − 92) / 25.2, which 2.8vw is under from
           320 dp up; the cap is what every screen wide enough gets. Measured
           on a 408 dp handset, where 0.8rem overflowed by exactly one
           character — the failure this replaces. */
        font-size: min(0.8rem, 2.8vw);
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        /* Last resort only: with the size above it never fires, and without it
           a narrower device than any tested would overflow the card instead. */
        overflow-wrap: anywhere;
    }
    .copy-btn {
        background: none;
        border: none;
        padding: 2px;
        cursor: pointer;
        color: var(--mdc-theme-text-secondary-on-surface, #999);
        display: inline-flex;
        flex-shrink: 0;
    }
    .copy-btn:hover {
        color: var(--mdc-theme-primary);
    }
    .copy-btn i {
        font-size: 16px;
    }
    .wallet-sub {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        font-size: 0.85rem;
    }
    .badges {
        margin-top: 6px;
        display: flex;
        gap: 6px;
        flex-wrap: wrap;
    }
    .pill {
        font-size: 0.72rem;
        padding: 2px 10px;
        border-radius: 999px;
        border: 1px solid currentColor;
    }
    .status-active { color: #2e7d32; }
    .status-suspended { color: #d32f2f; }
    .status-exited { color: #616161; }
    .pill.validator { color: #1565c0; }
    :global(.dark-theme) .status-active { color: #81c784; }
    :global(.dark-theme) .status-suspended { color: #e57373; }
    :global(.dark-theme) .pill.validator { color: #64b5f6; }

    .metrics-grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
        gap: 12px;
    }

    .rule-row {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 10px 14px;
        border: 1px dashed var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.2));
        border-radius: 8px;
        font-size: 0.85rem;
        line-height: 1.45;
        color: var(--mdc-theme-text-secondary-on-surface, #666);


    }
    .rule-icon {
        font-size: 20px;
        color: var(--mdc-theme-primary);
        flex-shrink: 0;
    }

    .bootstrap-banner {
        align-items: center;
        gap: 8px;
        padding: 8px 12px;
        background: rgba(76, 175, 80, 0.08);
        border-left: 3px solid rgba(76, 175, 80, 0.5);
        border-radius: 0 4px 4px 0;
        font-size: 0.85rem;
        color: #2e7d32;
    }
    :global(.bootstrap-banner-icon) {
        font-size: 20px !important;
        width: 20px;
        height: 20px;
        color: #43a047;
    }
    .bootstrap-banner-text {
        line-height: 1.4;
    }
    :global(.dark-theme) .bootstrap-banner {
        background: rgba(76, 175, 80, 0.12);
        border-left-color: #81c784;
        color: #a5d6a7;
    }
    :global(.dark-theme) :global(.bootstrap-banner-icon) {
        color: #81c784;
    }

    .probation-note,
    .default-note {
        padding: 8px 12px;
        border-radius: 0 4px 4px 0;
        font-size: 0.85rem;
        line-height: 1.4;
    }
    .probation-note {
        background: rgba(249, 168, 37, 0.08);
        border-left: 3px solid rgba(249, 168, 37, 0.6);
        color: #8d6e00;
    }
    :global(.dark-theme) .probation-note {
        color: #ffe082;
        background: rgba(249, 168, 37, 0.12);
    }
    .default-note {
        background: rgba(211, 47, 47, 0.06);
        border-left: 3px solid rgba(211, 47, 47, 0.5);
        color: #b71c1c;
    }
    :global(.dark-theme) .default-note {
        color: #ef9a9a;
        background: rgba(211, 47, 47, 0.12);
    }
</style>
