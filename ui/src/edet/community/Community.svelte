<script lang="ts">
    import { _ } from 'svelte-i18n';
    import { get } from 'svelte/store';
    import Button, { Label } from '@smui/button';
    import Textfield from '@smui/textfield';
    import Fab from '@smui/fab';
    import { Icon } from '@smui/common';

    import AgentAvatar from '../../components/AgentAvatar.svelte';
    import MemberChip from '../../components/MemberChip.svelte';
    import Explain from '../../components/Explain.svelte';
    import NumericInput from '../../common/NumericInput.svelte';
    import { membersList, listTruncation, networkView, activeBase, refresh } from '../../lib/node';
    import { currentActorId, nicknames, setNickname } from '../../lib/actors';
    import { memberName, shortAddress } from '../../lib/display';
    import { epochDate } from '../../lib/epoch';
    import * as api from '../../lib/api';
    import { tx } from '../../lib/api';
    import { hexToBytes } from '../../lib/crypto';
    import { send } from '../../lib/submit';
    import { formatNumber, parseNumber } from '../../common/functions';
    import { errorStore } from '../../common/errorStore';
    import { MAX_ADJUSTMENT, priceCounterparty, subjectivePricing } from '../../lib/pricing';

    let busy = false;

    let expanded: number | null = null;
    let detail: api.MemberDetail | null = null;
    let renaming: number | null = null;
    let renameDraft = '';
    let rotating: number | null = null;
    let rotateKeyHex = '';

    $: me = $currentActorId;
    $: dust = $networkView?.dust ?? 0.01;

    async function toggleDetail(id: number) {
        if (expanded === id) {
            expanded = null;
            detail = null;
            return;
        }
        expanded = id;
        detail = null;
        try {
            const d = await api.memberDetail(get(activeBase), id);
            if (expanded === id) detail = (d as any).error ? null : d;
        } catch {
            detail = null;
        }
    }

    /**
     * Guardian recovery: request a key rotation for a member who lost
     * their device. They create a NEW recovery phrase on their new device
     * and hand you the derived public key, pasted here — recovery never
     * exposes their secret. When the guardian threshold is above one, the
     * request goes to the other guardians' Requests inbox for co-signing;
     * the rotation then waits out the veto window (the old key can block a
     * theft) before anyone can finalize it.
     */
    async function rotateKeys(id: number) {
        if (me === null || !detail?.guardian) return;
        let key: number[];
        try {
            const bytes = hexToBytes(rotateKeyHex.trim());
            if (bytes.length !== 32) throw new Error('bad length');
            key = Array.from(bytes);
        } catch {
            errorStore.pushError($_('community.badKey', { default: 'The public key must be 64 hex characters.' }));
            return;
        }
        busy = true;
        try {
            const ok = await send(
                tx.rotateRequest(id, [key], detail.guardian.guardians, detail.guardian.threshold),
            );
            if (ok) {
                rotating = null;
                rotateKeyHex = '';
                await refresh();
            }
        } finally {
            busy = false;
        }
    }

    function startRename(id: number) {
        renaming = id;
        renameDraft = $nicknames[id] ?? '';
    }

    function saveRename() {
        if (renaming === null) return;
        setNickname(renaming, renameDraft);
        renaming = null;
    }

    /** A signed percentage, so "no rule" reads as nothing rather than as 0%. */
    function offsetLabel(v: number): string {
        if (v === 0) return $_('community.priceNone', { default: 'the community’s' });
        const pct = Math.round(Math.abs(v) * 100);
        return v > 0
            ? $_('community.priceWorse', { values: { pct }, default: `+${pct}% riskier` })
            : $_('community.priceBetter', { values: { pct }, default: `−${pct}% safer` });
    }
</script>

<div class="main-container flex-column">
    <Explain summary={$_('community.summary', { default: 'Everyone on your community ledger.' })}>
        {$_('community.intro', {
            default:
                'Capacity is what the community has put behind each of them — it is earned by carrying a debt and paying it, so it starts at zero for everybody and nobody can grant it directly.',
        })}
    </Explain>

    {#if $listTruncation['/members']}
        <p class="truncated">
            {$_('community.truncated', {
                values: { shown: $listTruncation['/members']?.shown, total: $listTruncation['/members']?.total },
                default: `This node sent ${$listTruncation['/members']?.shown} of ${$listTruncation['/members']?.total} members. Somebody missing from this list may still be on the ledger — look them up by their address.`,
            })}
        </p>
    {/if}

    <div class="member-list">
        {#each $membersList as m (m.id)}
            <div class="member-row" class:me={m.id === me}>
                <button class="member-main" type="button" on:click={() => toggleDetail(m.id)} aria-expanded={expanded === m.id}>
                    <AgentAvatar memberId={m.id} size={36} />
                    <span class="member-name-block">
                        <span class="member-name">{$memberName(m.id)}{m.id === me ? ' · ' + $_('community.you', { default: 'you' }) : ''}</span>
                        {#if $nicknames[m.id]}
                            <span class="member-address">{shortAddress(m.address)}</span>
                        {/if}
                        <span class="pill status-{m.status}">{$_('members.status.' + m.status, { default: m.status })}</span>
                    </span>
                    <span class="member-stats">
                        <span class="stat"><span class="k">{$_('community.capacity', { default: 'capacity' })}</span> {m.capacity === undefined ? $_('community.capacityUnknown', { default: 'not yet known' }) : formatNumber(m.capacity)}</span>
                        <span class="stat"><span class="k">{$_('community.debt', { default: 'debt' })}</span> {formatNumber(m.debt)}</span>
                        <span class="stat"><span class="k">{$_('community.confers', { default: 'may confer' })}</span> {formatNumber(m.conferrable ?? 0)}</span>
                        {#if (m.supply ?? 0) > 0}
                            <span class="stat"><span class="k">{$_('community.underwrites', { default: 'underwrites' })}</span> {formatNumber(m.supply ?? 0)}</span>
                        {/if}
                        {#if (m.open_default ?? 0) > dust}
                            <span class="stat bad"><span class="k">{$_('community.default', { default: 'default' })}</span> {formatNumber(m.open_default ?? 0)}</span>
                        {/if}
                    </span>
                    <i class="material-icons expand-icon" aria-hidden="true">{expanded === m.id ? 'expand_less' : 'expand_more'}</i>
                </button>

                {#if expanded === m.id}
                    <div class="member-detail">
                        {#if renaming === m.id}
                            <div class="form-row">
                                <Textfield label={$_('community.rename', { default: 'Local display name' })} bind:value={renameDraft} />
                                <Button variant="outlined" on:click={saveRename}><Label>{$_('common.save', { default: 'Save' })}</Label></Button>
                            </div>
                        {:else}
                            <Button variant="outlined" on:click={() => startRename(m.id)}>
                                <Label>{$_('community.renameBtn', { default: 'Rename locally' })}</Label>
                            </Button>
                        {/if}

                        {#if detail}
                            <code class="detail-address">{detail.address}</code>
                            <div class="detail-grid">
                                <span>{$_('community.joined', { values: { date: $epochDate(detail.joined_epoch) }, default: `Joined ${$epochDate(detail.joined_epoch)}` })}</span>
                                <span>{$_('community.backedBy', { values: { n: (detail.backers ?? []).length }, default: `Backed by ${(detail.backers ?? []).length}` })}</span>
                                {#if detail.is_validator}<span>{$_('community.validator', { default: 'Charter validator' })}</span>{/if}
                            </div>
                            <!--
                                Who has actually put standing behind this
                                member. Not a sponsorship and not a vouch: an
                                edge appears when somebody carried this member's
                                debt and was repaid, so the list is a record of
                                settled trade rather than of anybody's opinion.
                                That is why it cannot be bought, and why
                                selling earns none of it.
                            -->
                            {#if (detail.backers ?? []).length > 0}
                                <div class="sponsor-row">
                                    <span class="k">{$_('community.backers', { default: 'Backed by' })}:</span>
                                    {#each detail.backers ?? [] as b}
                                        <MemberChip memberId={b.member} size={20} />
                                    {/each}
                                </div>
                            {/if}

                            <!--
                                What this member knows about that member and the
                                ledger does not. It moves the app's own score
                                for them and nothing else: it is never sent
                                anywhere, nobody else can read it, and it
                                cannot reach past the guards the acceptance
                                rule already applies.
                            -->
                            {#if m.id !== me}
                                <div class="price-row">
                                    <span class="k">{$_('community.priceThem', { default: 'Your own reading' })}:</span>
                                    <input
                                        type="range"
                                        min={-MAX_ADJUSTMENT}
                                        max={MAX_ADJUSTMENT}
                                        step="0.05"
                                        value={$subjectivePricing.adjustments[m.id] ?? 0}
                                        aria-label={$_('community.priceThem', { default: 'Your own reading' })}
                                        on:change={(e) =>
                                            priceCounterparty($subjectivePricing, m.id, Number(e.currentTarget.value))}
                                    />
                                    <span class="price-value">
                                        {offsetLabel($subjectivePricing.adjustments[m.id] ?? 0)}
                                    </span>
                                </div>
                                <Explain
                                    summary={$_('community.priceSummary', {
                                        default: 'Your own reading of this member, kept on this device.',
                                    })}
                                >
                                    {$_('community.priceNote', {
                                        default:
                                            'The community scores them from what it has put behind them and what they owe. Move this to the right if you think worse of them than that, to the left if better. It changes only what your own wallet decides for you.',
                                    })}
                                </Explain>
                            {/if}

                            <div class="detail-actions">
                                {#if me !== null && m.id !== me && detail.guardian && detail.guardian.guardians.includes(me) && !detail.pending_rotation}
                                    {#if rotating === m.id}
                                        <div class="rotate-form">
                                            <Explain
                                                summary={$_('community.rotateSummary', {
                                                    default: 'Paste the public key from their NEW device.',
                                                })}
                                            >
                                                {$_('community.rotateBody', {
                                                    default:
                                                        'It must have been created there with a fresh recovery phrase. If the guardian threshold is above one, the other guardians will find this in their Requests inbox to co-sign. The rotation waits out the veto window before it can be finalized.',
                                                })}
                                            </Explain>
                                            <div class="form-row">
                                                <Textfield
                                                    label={$_('community.rotateKey', { default: 'New public key (64 hex chars)' })}
                                                    bind:value={rotateKeyHex}
                                                    style="min-width: 320px;"
                                                />
                                                <Button variant="raised" disabled={busy || !rotateKeyHex.trim()} on:click={() => rotateKeys(m.id)}>
                                                    <Label>{$_('community.rotateConfirm', { default: 'Request rotation' })}</Label>
                                                </Button>
                                                <Button variant="outlined" on:click={() => { rotating = null; rotateKeyHex = ''; }}>
                                                    <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
                                                </Button>
                                            </div>
                                        </div>
                                    {:else}
                                        <Button variant="outlined" disabled={busy} on:click={() => { rotating = m.id; rotateKeyHex = ''; }}>
                                            <Label>{$_('community.rotate', { default: 'Recover account (rotate keys)' })}</Label>
                                        </Button>
                                    {/if}
                                {/if}
                            </div>
                        {:else}
                            <p class="intro">{$_('common.loading', { default: 'Loading…' })}</p>
                        {/if}
                    </div>
                {/if}
            </div>
        {/each}
    </div>
</div>

<style>
    .price-row {
        display: flex;
        align-items: center;
        gap: 12px;
        flex-wrap: wrap;
    }
    .price-value {
        font-size: 0.9rem;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        font-variant-numeric: tabular-nums;
    }
    .price-note {
        margin: 0;
        font-size: 0.85rem;
        line-height: 1.45;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .main-container {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
        gap: 14px;
        max-width: var(--edet-column);
    }
    .intro {
        margin: 0;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        line-height: 1.5;
    }
    .truncated {
        margin: 0;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        line-height: 1.5;
        font-style: italic;
    }
    .form-row {
        display: flex;
        gap: 14px;
        align-items: flex-end;
        flex-wrap: wrap;
    }
    .member-list {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .member-row {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 8px;
        background: var(--mdc-theme-surface, #fff);
        overflow: hidden;
    }
    :global(.dark-theme) .member-row {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }
    .member-row.me {
        border-color: var(--mdc-theme-primary);
    }
    .member-main {
        width: 100%;
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 10px 14px;
        background: none;
        border: none;
        font: inherit;
        color: var(--mdc-theme-on-surface);
        cursor: pointer;
        text-align: left;
    }
    .member-name-block {
        display: flex;
        flex-direction: column;
        gap: 3px;
        min-width: 120px;
    }
    .member-name {
        font-weight: 600;
    }
    .member-address {
        font-family: monospace;
        font-size: 0.75rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }
    .member-stats {
        display: flex;
        gap: 16px;
        flex-wrap: wrap;
        margin-left: auto;
        font-size: 0.88rem;
    }
    .stat .k {
        color: var(--mdc-theme-text-secondary-on-surface, #888);
        font-size: 0.75rem;
        margin-right: 4px;
    }
    .stat.bad {
        color: #d32f2f;
    }
    .expand-icon {
        color: var(--mdc-theme-text-secondary-on-surface, #999);
    }
    .member-detail {
        border-top: 1px dashed var(--mdc-theme-text-hint-on-background, #ddd);
        padding: 12px 14px;
        display: flex;
        flex-direction: column;
        gap: 12px;
    }
    .detail-address {
        font-family: monospace;
        font-size: 0.78rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
        overflow-wrap: anywhere;
    }
    .detail-grid {
        display: flex;
        gap: 18px;
        flex-wrap: wrap;
        font-size: 0.9rem;
        color: var(--mdc-theme-text-secondary-on-surface, #555);
    }
    .sponsor-row {
        display: flex;
        gap: 10px;
        align-items: center;
        flex-wrap: wrap;
        font-size: 0.9rem;
    }
    .sponsor-row .k {
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }
    .detail-actions {
        display: flex;
        gap: 12px;
        align-items: flex-end;
        flex-wrap: wrap;
    }
    .vouch-form {
        display: flex;
        gap: 8px;
        align-items: flex-end;
    }
    .rotate-form {
        display: flex;
        flex-direction: column;
        gap: 10px;
        width: 100%;
        border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
        border-radius: 8px;
        padding: 12px;
    }
    .pill {
        font-size: 0.72rem;
        padding: 1px 8px;
        border-radius: 999px;
        border: 1px solid currentColor;
        align-self: flex-start;
    }
    .status-active { color: #2e7d32; }
    .status-suspended { color: #d32f2f; }
    .status-exited { color: #616161; }
    :global(.dark-theme) .status-active { color: #81c784; }
    :global(.dark-theme) .status-suspended { color: #e57373; }
</style>
