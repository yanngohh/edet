<script lang="ts">
    /**
     * The wallet's acceptance rule (My Wallet → FAB): two thresholds over the advisory
     * risk score that decide, without you, which incoming requests are signed
     * and which are declined — and which band stays yours to judge.
     */
    import { _ } from 'svelte-i18n';
    import { createEventDispatcher } from 'svelte';
    import Button, { Label } from '@smui/button';
    import Checkbox from '@smui/checkbox';
    import Dialog, { Title, Content, Actions } from '@smui/dialog';
    import FormField from '@smui/form-field';
    import Slider from '@smui/slider';

    import ActionPage from '../../components/ActionPage.svelte';
    import Explain from '../../components/Explain.svelte';
    import NumericInput from '../../common/NumericInput.svelte';
    import { parseNumber } from '../../common/functions';
    import { acceptancePolicy, DEFAULT_POLICY, normalizePolicy, setPolicy } from '../../lib/policy';
    import { backgroundArmed, needsBatteryExemption, openBatteryOptimisationSettings } from '../../lib/background';
    import { subjectivePricing, setPricing } from '../../lib/pricing';
    import { paramsView } from '../../lib/node';
    import { riskK } from '../../lib/risk';

    const dispatch = createEventDispatcher<{ close: void }>();

    const STEP = 0.01;
    /** The personal-K slider's step, and the width of "no rule". */
    const K_STEP = 0.05;
    /** Its ceiling. `Params::safe_range(RiskK)` is (0.25, 5.0); a member's own
     *  reading need not stay inside a range governance set for the community,
     *  but there is no sense past the point where every account reads risky. */
    const K_MAX = 5;

    let auto = $acceptancePolicy.auto;
    let background = $acceptancePolicy.background;
    /**
     * The one-time Doze briefing, shown between saving the switch on and the
     * OS screen where the exemption is granted.
     *
     * **A redirect with no sentence in front of it is a member dropped into a
     * settings screen they did not ask for**, with no idea which of its
     * controls they came for. So the instruction comes first and the redirect
     * is what its button does. It fires only where there is such a screen
     * (Android), only on the transition from off to on — turning the mode on
     * is when the member is deciding about it, and every later save would be
     * nagging — and only while the exemption has NOT already been granted
     * (`needsBatteryExemption`), because a prompt for something already done
     * is one people learn to dismiss unread.
     */
    let askBattery = false;
    let accept = $acceptancePolicy.accept;
    let reject = $acceptancePolicy.reject;
    // NumericInput is the app's one numeric control and it binds text —
    // parsed locale-aware on save, exactly like the amounts everywhere else.
    let maxAmountStr = String($acceptancePolicy.maxAmount);

    // The member's own K, in place of the governed one. The slider's default
    // position IS the governed value, and the reading beside it says so — a
    // dial whose zero is somebody else's number has to show that number.
    $: governedK = riskK($paramsView);
    let personalK: number | null = $subjectivePricing.personalK;
    $: kValue = personalK ?? governedK;

    // Turning the rule off turns this off with it, on screen and not only on
    // save: a box left visibly ticked while disabled reads as "still on", and
    // `save` stores `auto && background` — so the tick would be claiming
    // something the next save contradicts.
    $: if (!auto) background = false;

    // MDC's range slider needs the thumbs apart; a collapsed band would also
    // leave nothing to review.
    $: if (reject < accept + STEP) reject = Math.min(1, accept + STEP);

    $: acceptPct = Math.round(accept * 100);
    $: rejectPct = Math.round(reject * 100);

    async function save() {
        const wasBackground = $acceptancePolicy.background;
        const maxAmount = parseNumber(maxAmountStr);
        setPolicy({
            auto,
            // No meaning without the rule itself, and a stored `true` under a
            // rule that is off would arm the moment the rule came back on —
            // which is not what the member last decided.
            background: auto && background,
            accept,
            reject,
            maxAmount: Number.isFinite(maxAmount) ? Math.max(0, maxAmount) : 0,
        });
        // Within a step of the governed value is "no rule", not "the same rule
        // written down": a stored personal K would go on meaning the old
        // number after governance moved it.
        const mine = Math.abs(kValue - governedK) < K_STEP / 2 ? null : kValue;
        setPricing({ ...$subjectivePricing, personalK: mine });
        // The rule is SAVED either way: the briefing is about the platform
        // around it, not about whether the member meant to turn it on.
        if (auto && background && !wasBackground && (await needsBatteryExemption())) {
            askBattery = true;
            return;
        }
        dispatch('close');
    }

    function openBattery() {
        askBattery = false;
        void openBatteryOptimisationSettings();
        dispatch('close');
    }

    function restoreDefaults() {
        const d = normalizePolicy(DEFAULT_POLICY);
        auto = d.auto;
        background = d.background;
        accept = d.accept;
        reject = d.reject;
        maxAmountStr = String(d.maxAmount);
        personalK = null;
    }
</script>

<ActionPage icon="tune" title={$_('acceptance.title', { default: 'Acceptance rule' })} on:close={() => dispatch('close')}>
    <Explain summary={$_('acceptance.summary', { default: 'Set the rule once instead of judging every purchase.' })}>
        {$_('acceptance.intro', {
            default:
                'Every purchase you sell into needs your signature. The app scores the buyer from the ledger — what the community backs them for, what they already owe, and how fast they are taking on debt — and signs the safe ones, declines the bad ones, and keeps the middle for you.',
        })}
    </Explain>

    <FormField>
        <Checkbox bind:checked={auto} />
        <span slot="label">{$_('acceptance.autoLabel', { default: 'Decide incoming requests automatically' })}</span>
    </FormField>

    <FormField>
        <Checkbox bind:checked={background} disabled={!auto} />
        <span slot="label" class:off={!auto}>
            {$_('acceptance.backgroundLabel', { default: 'Keep deciding while the app is in the background' })}
        </span>
    </FormField>
    <Explain
        tone="notice"
        summary={$_('acceptance.backgroundSummary', {
            default: 'The rule works while the app is behind something else, not once it is closed.',
        })}
    >
        {$_('acceptance.backgroundNote', {
            default:
                'The rule signs on this device, so it decides for as long as this app is running. With this on, closing the window hides it instead, and a phone keeps a notice in your shade while edet works. Keep the phone on charge, or exempt edet from battery optimisation: a device left unplugged and still, with the screen off, has its network suspended by the system.',
        })}
    </Explain>
    <!-- What the SAVED rule is actually doing, which is not always what it
         asked for: a desktop with no system tray and a phone that refused the
         service both leave this device deciding only while the app is in
         front, and a member who is not told that believes otherwise. -->
    {#if $acceptancePolicy.background && !$backgroundArmed}
        <p class="note warn">
            {$_('acceptance.backgroundUnavailable', {
                default: 'This device could not keep the rule running in the background.',
            })}
        </p>
    {/if}

    <div class="band-card" class:off={!auto}>
        <div class="band-legend">
            <span class="legend safe">
                <i class="material-icons" aria-hidden="true">verified_user</i>
                {$_('acceptance.safe', { default: 'Signed for you' })}
                <b>≤ {acceptPct}%</b>
            </span>
            <span class="legend manual">
                <i class="material-icons" aria-hidden="true">visibility</i>
                {$_('acceptance.manual', { default: 'Held for your review' })}
                <b>{acceptPct}–{rejectPct}%</b>
            </span>
            <span class="legend unsafe">
                <i class="material-icons" aria-hidden="true">warning</i>
                {$_('acceptance.unsafe', { default: 'Declined for you' })}
                <b>≥ {rejectPct}%</b>
            </span>
        </div>

        <div
            class="band-bar"
            role="img"
            aria-label={$_('acceptance.bandBar', {
                values: { accept: acceptPct, reject: rejectPct },
                default: `Signed up to ${acceptPct}% risk, held between ${acceptPct}% and ${rejectPct}%, declined from ${rejectPct}%`,
            })}
        >
            <div class="seg safe" style={`width: ${acceptPct}%`}></div>
            <div class="seg manual" style={`width: ${rejectPct - acceptPct}%`}></div>
            <div class="seg unsafe" style={`width: ${100 - rejectPct}%`}></div>
        </div>

        <Slider range bind:start={accept} bind:end={reject} min={0} max={1} step={STEP} disabled={!auto} />

        <div class="threshold-row">
            <span>{$_('acceptance.acceptThreshold', { default: 'Sign at or below' })} <b>{acceptPct}%</b></span>
            <span>{$_('acceptance.rejectThreshold', { default: 'Decline at or above' })} <b>{rejectPct}%</b></span>
        </div>
    </div>

    <div class="ceiling">
        <NumericInput
            bind:value={maxAmountStr}
            disabled={!auto}
            label={$_('acceptance.maxAmount', { default: 'Never sign more than' })}
        />
        <Explain
            tone="notice"
            summary={$_('acceptance.maxAmountSummary', { default: 'Anything above this waits for you, however well they score.' })}
        >
            {$_('acceptance.maxAmountNote', {
                default:
                    'The score prices the buyer, not the size of what you are granting. Leave it at 0 and every request waits for you.',
            })}
        </Explain>
    </div>

    <div class="ceiling">
        <span class="k-label">
            {$_('acceptance.personalK', { default: 'How much backing you want to see' })}
        </span>
        <Slider bind:value={kValue} min={K_STEP} max={K_MAX} step={K_STEP} />
        <Explain
            tone="notice"
            summary={$_('acceptance.personalKSummary', { default: "Your own reading of the community's one scale." })}
        >
            {$_('acceptance.personalKNote', {
                default:
                    'Move it right to want more backing behind somebody before the app calls them safe, left to want less. Leave it where it starts and you are using the community’s number.',
            })}
        </Explain>
        <p class="note">
            {$_('acceptance.personalKCurrent', {
                values: { mine: kValue.toFixed(2), community: governedK.toFixed(2) },
                default: `Yours: ${kValue.toFixed(2)}. The community’s: ${governedK.toFixed(2)}.`,
            })}
        </p>
    </div>

    <Explain
        tone="notice"
        summary={$_('acceptance.scopeSummary', { default: 'Only purchases where you grant credit are decided this way.' })}
    >
        {$_('acceptance.scopeNote', {
            default:
                'A request the ledger would reject is never signed. Key recoveries, guardian changes, arbitration and governance always wait for you, whatever they score.',
        })}
    </Explain>
    <Explain
        tone="notice"
        summary={$_('acceptance.deviceSummary', { default: 'The rule and the signing key live on this device.' })}
    >
        {$_('acceptance.deviceNote', {
            default:
                'Requests are decided while the app is running, in front or in the background. Nothing can sign in your name once it is closed.',
        })}
    </Explain>

    <div class="page-actions">
        <Button variant="raised" on:click={() => void save()}><Label>{$_('common.save', { default: 'Save' })}</Label></Button>
        <Button variant="outlined" on:click={() => dispatch('close')}>
            <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
        </Button>
        <Button variant="outlined" on:click={restoreDefaults}>
            <Label>{$_('acceptance.reset', { default: 'Restore defaults' })}</Label>
        </Button>
    </div>
</ActionPage>

<Dialog bind:open={askBattery} aria-labelledby="battery-title" aria-describedby="battery-body">
    <Title id="battery-title">{$_('acceptance.batteryTitle', { default: 'One setting is Android’s, not ours' })}</Title>
    <Content id="battery-body">
        {$_('acceptance.batteryBody', {
            default:
                'Android suspends an app’s network when the phone is left unplugged and still with the screen off, and edet cannot exempt itself. On the next screen, allow edet to run in the background — set its battery use to unrestricted. Keeping the phone on charge does the same thing.',
        })}
    </Content>
    <Actions>
        <Button on:click={() => { askBattery = false; dispatch('close'); }}>
            <Label>{$_('acceptance.batteryLater', { default: 'Not now' })}</Label>
        </Button>
        <Button variant="raised" on:click={openBattery}>
            <Label>{$_('acceptance.batteryOpen', { default: 'Open battery settings' })}</Label>
        </Button>
    </Actions>
</Dialog>

<style>
    /* The label beside a disabled box, dimmed with it: MDC dims the control
       and leaves the text at full contrast, which reads as an active choice. */
    .off {
        opacity: 0.55;
    }
    .note.warn {
        color: #c62828;
    }
    :global(.dark-theme) .note.warn {
        color: #ef9a9a;
    }
    .band-card {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 12px;
        padding: 16px;
        background: var(--mdc-theme-surface, #fff);
        display: flex;
        flex-direction: column;
        gap: 12px;
    }
    :global(.dark-theme) .band-card {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }
    .band-card.off {
        opacity: 0.55;
    }
    .band-legend {
        display: flex;
        gap: 16px;
        flex-wrap: wrap;
        font-size: 0.85rem;
    }
    .legend {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .legend i {
        font-size: 18px;
    }
    .legend b {
        font-variant-numeric: tabular-nums;
    }
    .legend.safe { color: #2e7d32; }
    /* Darker than the amber of the band itself: this is text on white. */
    .legend.manual { color: #8d6e00; }
    .legend.unsafe { color: #c62828; }
    :global(.dark-theme) .legend.safe { color: #81c784; }
    :global(.dark-theme) .legend.manual { color: #ffe082; }
    :global(.dark-theme) .legend.unsafe { color: #ef9a9a; }
    .band-bar {
        display: flex;
        height: 14px;
        border-radius: 999px;
        overflow: hidden;
        background: var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.08));
    }
    .seg {
        height: 100%;
        transition: width 120ms ease;
    }
    .seg.safe { background: #43a047; }
    .seg.manual { background: #fbc02d; }
    .seg.unsafe { background: #e53935; }
    .threshold-row {
        display: flex;
        gap: 24px;
        flex-wrap: wrap;
        font-size: 0.9rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .threshold-row b {
        font-variant-numeric: tabular-nums;
        color: var(--mdc-theme-on-surface);
    }
    .k-label {
        font-size: 0.95rem;
        font-weight: 500;
    }
    .note {
        margin: 0;
        font-size: 0.82rem;
        line-height: 1.45;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }
    .page-actions {
        display: flex;
        gap: 10px;
        flex-wrap: wrap;
    }
</style>
