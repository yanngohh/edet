<script lang="ts">
    import { _ } from 'svelte-i18n';
    import Button, { Label } from '@smui/button';

    import NumericInput from '../../common/NumericInput.svelte';

    import MemberChip from '../../components/MemberChip.svelte';
    import AddressInput from '../../components/AddressInput.svelte';
    import Explain from '../../components/Explain.svelte';
    import { proposalsView, paramsView, myMember, networkView } from '../../lib/node';
    import { currentActorId } from '../../lib/actors';
    import { memberName } from '../../lib/display';
    import { tx, proposalKinds, type ProposalView } from '../../lib/api';
    import { send } from '../../lib/submit';
    import { assentVerdict } from '../../lib/governance';
    import { formatNumber, parseNumber } from '../../common/functions';

    type KindChoice = 'param_change' | 'redenominate' | 'suspend' | 'unsuspend' | 'validator_power' | 'seed_amendment';

    let showNew = false;
    let kindChoice: KindChoice = 'param_change';
    let paramKey = 'RiskK';
    let paramValueStr = '';
    let redenomNumStr = '1';
    let redenomDenStr = '1';
    let targetMember: number | null = null;
    let powerStr = '1';
    let seedAmountStr = '';
    let busy = false;

    $: me = $currentActorId;
    $: props = $proposalsView;
    $: prms = $paramsView;
    // Assent weight is the share of the community's EXTERNAL seed that
    // has assented — genesis plus every endorsed amendment — not a headcount.
    // This line read `ceil(theta_adopt * active_members)` until the seed
    // became the measure, which described a quorum the ledger has never run.
    $: threshold = props ? props.theta_adopt * props.external_seed : 0;
    $: selectedParam = prms?.governed.find((g) => g.key === paramKey);

    // Who may assent, and why: `lib/governance.ts`. Members propose; the
    // ceremony enacts (§Governance), and offering the button to
    // everybody and let `ET-GOV-007` explain itself afterwards.
    $: dust = $networkView?.dust ?? 0.01;

    function kindLabel(p: ProposalView): string {
        const k = p.kind;
        switch (k.type) {
            case 'param_change':
                return $_('governance.kind.paramChange', {
                    values: { key: k.key, value: formatNumber(k.value, 3) },
                    default: `Change ${k.key} to ${formatNumber(k.value, 3)}`,
                });
            case 'redenominate':
                return $_('governance.kind.redenominate', {
                    values: { num: k.num, den: k.den },
                    default: `Re-denominate every amount by ${k.num}/${k.den}`,
                });
            case 'suspend':
                return $_('governance.kind.suspend', { values: { member: $memberName(k.member) }, default: `Suspend ${$memberName(k.member)}` });
            case 'unsuspend':
                return $_('governance.kind.unsuspend', { values: { member: $memberName(k.member) }, default: `Unsuspend ${$memberName(k.member)}` });
            case 'validator_power':
                return $_('governance.kind.validatorPower', {
                    values: { member: $memberName(k.member), power: k.power },
                    default: `Set ${$memberName(k.member)}'s validator power to ${k.power}`,
                });
            case 'seed_amendment':
                // The beneficiary is the proposal's author, always — the kind
                // carries no member of its own, because a supply is a consent
                // only its holder can give.
                return $_('governance.kind.seedAmendment', {
                    values: { member: $memberName(p.author), amount: formatNumber(k.amount, 2) },
                    default: `Seat ${$memberName(p.author)} as an underwriter for ${formatNumber(k.amount, 2)} brought from outside`,
                });
            default:
                return k.type;
        }
    }

    function buildKind(): Record<string, unknown> | null {
        switch (kindChoice) {
            case 'param_change':
                return proposalKinds.paramChange(paramKey, parseNumber(paramValueStr));
            case 'redenominate':
                return proposalKinds.redenominate(Number(redenomNumStr) || 1, Number(redenomDenStr) || 1);
            case 'suspend':
                return targetMember === null ? null : proposalKinds.suspend(targetMember);
            case 'unsuspend':
                return targetMember === null ? null : proposalKinds.unsuspend(targetMember);
            case 'validator_power':
                return targetMember === null ? null : proposalKinds.validatorPower(targetMember, Number(powerStr) || 0);
            case 'seed_amendment': {
                const amount = parseNumber(seedAmountStr);
                return amount > 0 ? proposalKinds.seedAmendment(amount) : null;
            }
        }
    }

    async function propose() {
        if (me === null) return;
        const kind = buildKind();
        if (!kind) return;
        busy = true;
        try {
            if (await send(tx.propose(me, kind))) showNew = false;
        } finally {
            busy = false;
        }
    }

    async function assent(p: ProposalView) {
        if (me === null) return;
        busy = true;
        try {
            await send(tx.assent(me, p.id));
        } finally {
            busy = false;
        }
    }
</script>

<div class="main-container flex-column">
    <Explain
        summary={$_('governance.summary', {
            values: { threshold: formatNumber(threshold, 2), seed: formatNumber(props?.external_seed ?? 0, 2) },
            default: `Changes enact when underwriters holding ${formatNumber(threshold, 2)} of the ${formatNumber(props?.external_seed ?? 0, 2)} external seed concur.`,
        })}
    >
        {$_('governance.intro', {
            default:
                'Constitutional changes happen by proposal and assent, and the vote belongs to the backing brought from outside the community. Every governed constant lives inside a hard safe range.',
        })}
    </Explain>

    <div class="header-row">
        <h3 class="section-title">{$_('governance.proposalsTitle', { default: 'Proposals' })}</h3>
        <Button variant="raised" disabled={me === null} on:click={() => (showNew = !showNew)}>
            <Label>{$_('governance.new', { default: 'New proposal' })}</Label>
        </Button>
    </div>

    {#if showNew}
        <div class="new-box">
            <label class="kind-select">
                <span class="k">{$_('governance.kindLabel', { default: 'Kind' })}</span>
                <select bind:value={kindChoice}>
                    <option value="param_change">{$_('governance.kinds.paramChange', { default: 'Change a governed constant' })}</option>
                    <option value="redenominate">{$_('governance.kinds.redenominate', { default: 'Re-denominate the unit' })}</option>
                    <option value="suspend">{$_('governance.kinds.suspend', { default: 'Suspend a member' })}</option>
                    <option value="unsuspend">{$_('governance.kinds.unsuspend', { default: 'Unsuspend a member' })}</option>
                    <option value="validator_power">{$_('governance.kinds.validatorPower', { default: 'Set validator power' })}</option>
                    <option value="seed_amendment">{$_('governance.kinds.seedAmendment', { default: 'Bring backing from outside' })}</option>
                </select>
            </label>

            {#if kindChoice === 'param_change'}
                <div class="form-row">
                    <label class="kind-select">
                        <span class="k">{$_('governance.param', { default: 'Constant' })}</span>
                        <select bind:value={paramKey}>
                            {#each prms?.governed ?? [] as g (g.key)}
                                <option value={g.key}>{g.key}</option>
                            {/each}
                        </select>
                    </label>
                    <NumericInput
                        min={selectedParam?.min ?? 0}
                        bind:value={paramValueStr}
                        label={selectedParam
                            ? $_('governance.valueIn', {
                                  values: { min: selectedParam.min, max: selectedParam.max },
                                  default: `Value (${selectedParam.min} – ${selectedParam.max})`,
                              })
                            : $_('governance.value', { default: 'Value' })}
                    />
                </div>
                {#if selectedParam}
                    <p class="note">
                        {$_('governance.currentValue', {
                            values: { value: formatNumber(selectedParam.value, 3) },
                            default: `Current value: ${formatNumber(selectedParam.value, 3)}`,
                        })}
                    </p>
                {/if}
            {:else if kindChoice === 'redenominate'}
                <div class="form-row">
                    <NumericInput integer min={1} bind:value={redenomNumStr} label={$_('governance.num', { default: 'Numerator' })} />
                    <NumericInput integer min={1} bind:value={redenomDenStr} label={$_('governance.den', { default: 'Denominator' })} />
                </div>
                <Explain
                    tone="notice"
                    icon="info"
                    summary={$_('governance.redenomSummary', {
                        default: 'Re-denomination rescales every amount on the ledger atomically.',
                    })}
                >
                    {$_('governance.redenomNote', {
                        default: 'Prices, debts, stakes and history all move together; relative positions are untouched.',
                    })}
                </Explain>
            {:else if kindChoice === 'seed_amendment'}
                <div class="form-row">
                    <NumericInput min={0} bind:value={seedAmountStr} label={$_('governance.seedAmount', { default: 'Amount you stand behind' })} />
                </div>
                <Explain
                    tone="notice"
                    icon="info"
                    summary={$_('governance.seedSummary', {
                        default: 'You are asking to be seated as an underwriter. It is a liability, not a rank.',
                    })}
                >
                    {$_('governance.seedNote', {
                        default:
                            'The backing you bring comes from outside the community: if the members your standing reaches fail, this much of the loss becomes a debt you owe in goods and services. Nobody can propose this on your behalf, and you do not vote on your own.',
                    })}
                </Explain>
            {:else}
                <div class="form-row">
                    <AddressInput bind:value={targetMember} label={$_('governance.member', { default: "Member's address" })} />
                    {#if kindChoice === 'validator_power'}
                        <NumericInput integer min={0} bind:value={powerStr} label={$_('governance.power', { default: 'Power (0 removes)' })} />
                    {/if}
                </div>
            {/if}

            <div class="form-row">
                <Button variant="raised" disabled={busy} on:click={propose}>
                    <Label>{$_('governance.submit', { default: 'Submit proposal' })}</Label>
                </Button>
                <Button variant="outlined" on:click={() => (showNew = false)}>
                    <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
                </Button>
            </div>
        </div>
    {/if}

    {#if (props?.proposals ?? []).length === 0}
        <p class="empty">{$_('governance.none', { default: 'No proposals yet.' })}</p>
    {:else}
        <div class="proposal-list">
            {#each props?.proposals ?? [] as p (p.id)}
                {@const verdict = assentVerdict(me, $myMember, p, dust)}
                <div class="proposal-card" class:enacted={p.enacted}>
                    <div class="proposal-head">
                        <span class="proposal-id">#{p.id}</span>
                        <span class="proposal-kind">{kindLabel(p)}</span>
                        {#if p.enacted}
                            <span class="pill enacted-pill">{$_('governance.enacted', { default: 'enacted' })}</span>
                        {:else}
                            <span class="pill open-pill">
                                {$_('governance.assents', {
                                    values: { count: formatNumber(p.assented_seed, 2), threshold: formatNumber(threshold, 2) },
                                    default: `${formatNumber(p.assented_seed, 2)} / ${formatNumber(threshold, 2)} of the seed`,
                                })}
                            </span>
                        {/if}
                    </div>
                    <div class="proposal-body">
                        <span class="author">
                            {$_('governance.by', { default: 'proposed by' })} <MemberChip memberId={p.author} size={20} />
                        </span>
                        {#if verdict === 'assented'}
                            <span class="note">{$_('governance.youAssented', { default: 'You assented.' })}</span>
                        {:else if verdict === 'no-mandate'}
                            <span class="note">
                                {$_('governance.noMandate', {
                                    default:
                                        'Assent belongs to the underwriters a ceremony seated: the vote is a share of the backing brought from outside the community. Yours came from inside it, so you may propose, and this is not yours to sign.',
                                })}
                            </span>
                        {:else if verdict === 'own-amendment'}
                            <span class="note">
                                {$_('governance.ownAmendment', {
                                    default:
                                        'This amendment seats you, so it is not yours to assent to — it would be a vote cast with the very backing it grants.',
                                })}
                            </span>
                        {:else if verdict === 'offer'}
                            <Button variant="outlined" disabled={busy} on:click={() => assent(p)}>
                                <Label>{$_('governance.assent', { default: 'Assent' })}</Label>
                            </Button>
                        {/if}
                    </div>
                </div>
            {/each}
        </div>
    {/if}

    <h3 class="section-title">{$_('governance.paramsTitle', { default: 'Governed constants' })}</h3>
    <div class="params-table-wrap">
        <table class="params-table">
            <thead>
                <tr>
                    <th>{$_('governance.paramCol', { default: 'Constant' })}</th>
                    <th>{$_('governance.valueCol', { default: 'Value' })}</th>
                    <th>{$_('governance.rangeCol', { default: 'Safe range' })}</th>
                </tr>
            </thead>
            <tbody>
                {#each prms?.governed ?? [] as g (g.key)}
                    <tr>
                        <td class="mono">{g.key}</td>
                        <td>{formatNumber(g.value, 3)}</td>
                        <td class="mono">{g.min} – {g.max}</td>
                    </tr>
                {/each}
            </tbody>
        </table>
    </div>
</div>

<style>
    .main-container {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
        gap: 14px;
        max-width: var(--edet-column);
    }
    .header-row {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 16px;
        flex-wrap: wrap;
    }
    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin: 8px 0 0;
        font-weight: 500;
    }
    .new-box {
        border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
        border-radius: 8px;
        padding: 14px;
        display: flex;
        flex-direction: column;
        gap: 12px;
    }
    .kind-select {
        display: flex;
        flex-direction: column;
        gap: 4px;
        min-width: 220px;
    }
    .kind-select .k {
        font-size: 0.8rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .kind-select select {
        padding: 10px 12px;
        border: 1px solid var(--mdc-theme-text-hint-on-background, #e0e0e0);
        border-radius: 4px;
        background: var(--mdc-theme-surface, #fff);
        color: var(--mdc-theme-on-surface);
        font-size: 1rem;
        font-family: inherit;
    }
    :global(.dark-theme) .kind-select select {
        background: #2a2a2a;
        border-color: rgba(255, 255, 255, 0.2);
        color: #fff;
    }
    .form-row {
        display: flex;
        gap: 14px;
        align-items: flex-end;
        flex-wrap: wrap;
    }
    .proposal-list {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .proposal-card {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-left: 4px solid var(--mdc-theme-primary);
        border-radius: 8px;
        padding: 12px 16px;
        background: var(--mdc-theme-surface, #fff);
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    :global(.dark-theme) .proposal-card {
        background: #1e1e1e;
        border-top-color: rgba(255, 255, 255, 0.1);
        border-right-color: rgba(255, 255, 255, 0.1);
        border-bottom-color: rgba(255, 255, 255, 0.1);
    }
    .proposal-card.enacted {
        border-left-color: #43a047;
        opacity: 0.85;
    }
    .proposal-head {
        display: flex;
        align-items: center;
        gap: 12px;
        flex-wrap: wrap;
    }
    .proposal-id {
        font-family: monospace;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }
    .proposal-kind {
        font-weight: 600;
    }
    .pill {
        margin-left: auto;
        font-size: 0.72rem;
        padding: 2px 10px;
        border-radius: 999px;
        border: 1px solid currentColor;
    }
    .enacted-pill { color: #2e7d32; }
    .open-pill { color: #1565c0; }
    :global(.dark-theme) .enacted-pill { color: #81c784; }
    :global(.dark-theme) .open-pill { color: #64b5f6; }
    .proposal-body {
        display: flex;
        align-items: center;
        gap: 14px;
        flex-wrap: wrap;
    }
    .author {
        font-size: 0.88rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .params-table-wrap {
        overflow-x: auto;
    }
    .params-table {
        border-collapse: collapse;
        width: 100%;
    }
    .params-table th,
    .params-table td {
        text-align: left;
        padding: 8px 12px;
        border-bottom: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.08));
        font-size: 0.9rem;
    }
    .params-table th {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        font-weight: 600;
    }
    .mono {
        font-family: monospace;
    }
    .note {
        margin: 0;
        font-size: 0.82rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }
    .empty {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
</style>
