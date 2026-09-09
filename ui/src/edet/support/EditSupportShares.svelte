<script lang="ts">
    /**
     * Support configuration (Support → FAB): the waterfill editor for how
     * your sales are split. Shares always total 100%; your own share is the
     * part that clears your own debts.
     */
    import { _ } from 'svelte-i18n';
    import { createEventDispatcher } from 'svelte';
    import Button, { Label } from '@smui/button';
    import IconButton from '@smui/icon-button';
    import Slider from '@smui/slider';

    import ActionPage from '../../components/ActionPage.svelte';
    import BondNotice from '../../components/BondNotice.svelte';
    import MemberChip from '../../components/MemberChip.svelte';
    import AddressInput from '../../components/AddressInput.svelte';
    import { firstLoadDone, membersList, myMember } from '../../lib/node';
    import { currentActorId } from '../../lib/actors';
    import { memberHue, memberName } from '../../lib/display';
    import { errorStore } from '../../common/errorStore';
    import { tx } from '../../lib/api';
    import { send } from '../../lib/submit';
    import { formatNumber, formatPercentage } from '../../common/functions';
    import { appendShare, normalizeShares, rebalanceShares, removeShare, SHARE_PRECISION } from '../../lib/waterfill';

    interface Row {
        member: number;
        share: number; // 0..1, all rows total 1
    }

    const dispatch = createEventDispatcher<{ close: void }>();

    let rows: Row[] = [];
    let newMember: number | null = null;
    /** The picker itself, so the field can be emptied once its value is used. */
    let addressField: AddressInput;
    let busy = false;

    $: me = $currentActorId;
    $: addressById = new Map($membersList.map((m) => [m.id, m.address]));

    function hueOf(member: number): number {
        return memberHue(addressById.get(member) ?? null, member);
    }

    /**
     * Seed the editor once, from the listing on the ledger.
     *
     * **Driven by the reactive block below, never called from the script
     * body.** `$: me = $currentActorId` is assigned during the component's
     * first UPDATE, which runs after the instance script — so a top-level
     * `seed()` read `me` as `undefined`, and `=== null` does not catch
     * `undefined`. It seeded the self row as `{ member: undefined, share: 1 }`,
     * that row serialised to `[null, 1]`, and the node refused the whole
     * envelope: `entries[0][0]: invalid type: null, expected u64`. The wallet
     * reported the 422 as "Could not reach the node", which is the one thing
     * it was not.
     */
    function seed(): void {
        if (me == null) return;
        const bens = $myMember?.beneficiaries ?? [];
        let list: Row[] = bens.map((b) => ({ member: b.member, share: b.weight }));
        if (list.length === 0) {
            // No listing on the ledger = everything clears you: start the
            // breakdown from that implicit state.
            list = [{ member: me, share: 1 }];
        } else if (!list.some((r) => r.member === me)) {
            list.unshift({ member: me, share: 0 });
        }
        list.sort((a, b) => (a.member === me ? -1 : b.member === me ? 1 : a.member - b.member));
        const shares = normalizeShares(list.map((r) => r.share));
        rows = list.map((r, i) => ({ member: r.member, share: shares[i] }));
    }

    let seeded = false;
    /**
     * `firstLoadDone` as well as the id, because seeding before the member
     * view has landed reads an EMPTY beneficiary list — and saving that would
     * erase a listing the member already has. The 422 was loud; that would
     * have been silent.
     */
    $: if (!seeded && me != null && $firstLoadDone) {
        seed();
        seeded = true;
    }

    function onSlider(i: number) {
        const shares = rebalanceShares(rows.map((r) => r.share), i);
        rows = rows.map((r, j) => ({ ...r, share: shares[j] }));
    }

    /**
     * `member` defaults to whatever the input resolved to, and is passed
     * explicitly by the scan: a `bind:` propagates on the next update, so the
     * scan's own id has to travel with its event rather than be read back.
     */
    function addRow(member: number | null = newMember) {
        if (member === null || rows.some((r) => r.member === member)) return;
        const shares = appendShare(rows.map((r) => r.share));
        const added = member;
        rows = [...rows.map((r, j) => ({ ...r, share: shares[j] })), { member: added, share: shares[shares.length - 1] }];
        newMember = null;
        // The address is in the list now; leaving it in the box invites a
        // second Add that the duplicate guard would silently swallow.
        addressField?.clear();
    }

    function removeRow(i: number) {
        if (rows[i].member === me) return; // the self share stays; set it to 0 instead
        const shares = removeShare(rows.map((r) => r.share), i);
        rows = rows.filter((_, j) => j !== i).map((r, j) => ({ ...r, share: shares[j] }));
    }

    async function save() {
        if (me == null) return;
        const dust = 1 / SHARE_PRECISION / 2;
        // **A row at 0% is not saved, so say so rather than drop it.**
        // `appendShare` starts a new beneficiary at 0% by design — the member
        // raises the slider afterwards — and the listing sent to the ledger
        // filters those out. Adding somebody and saving without touching their
        // slider therefore wrote nothing at all, and said nothing about it.
        // Your OWN row is exempt: 0% there is the documented way to say your
        // sales support only your circle.
        const dropped = rows.filter((r) => r.member !== me && r.share <= dust);
        if (dropped.length > 0) {
            errorStore.pushError(
                $_('support.zeroShare', {
                    values: { member: $memberName(dropped[0].member) },
                    default: `${$memberName(dropped[0].member)} is at 0% and would not be listed — raise their share, or remove the row.`,
                }),
            );
            return;
        }
        const entries: [number, number][] = rows
            .filter((r) => r.share > dust)
            .map((r) => [r.member, r.share]);
        busy = true;
        try {
            if (await send(tx.listBeneficiaries(me, entries))) dispatch('close');
        } finally {
            busy = false;
        }
    }
</script>

<ActionPage
    icon="diversity_3"
    title={$_('support.beneficiariesTitle', { default: 'How your sales are shared' })}
    on:close={() => dispatch('close')}
>
    <!-- A distribution needs something to distribute. With only your own row
         the bar is one segment at 100%: a full-width block of colour beside
         nothing, which reads as a rendering artefact rather than as a share.
         The section behind this page applies the same rule. -->
    {#if rows.length > 1}
        <div
            class="share-bar"
            role="img"
            aria-label={$_('support.sharesBar', { default: 'Distribution of your support shares' })}
        >
            {#each rows as row (row.member)}
                <div
                    class="share-seg"
                    style={`width: ${row.share * 100}%; background: hsl(${hueOf(row.member)}, 62%, 52%);`}
                    title={formatPercentage(row.share * 100, 1)}
                ></div>
            {/each}
        </div>
    {/if}

    {#each rows as row, i (row.member)}
        <!-- No slider until there is something to divide. The share bar above
             is hidden for the same reason: with your own row alone the answer
             is 100% and a control that can only hold one value is not one —
             dragged, it snaps back, which reads as a broken widget rather than
             as a rule. -->
        <div class="slider-row" class:self={row.member === me} class:no-track={rows.length < 2}>
            <span class="swatch" style={`background: hsl(${hueOf(row.member)}, 62%, 52%);`}></span>
            <span class="slider-name">
                <MemberChip memberId={row.member} size={22} detail={row.member === me ? $_('community.you', { default: 'you' }) : ''} />
            </span>
            {#if rows.length > 1}
                <div class="slider-track">
                    <Slider
                        bind:value={row.share}
                        min={0}
                        max={1}
                        step={0.0001}
                        on:SMUISlider:input={() => onSlider(i)}
                        on:SMUISlider:change={() => onSlider(i)}
                    />
                </div>
            {/if}
            <span class="slider-pct">{formatNumber(row.share * 100, 1)}%</span>
            <span class="row-action">
                {#if row.member === me}
                    <span class="keep-slot" title={$_('support.selfPill', { default: 'clears your debts' })}>
                        <i class="material-icons self-icon" aria-hidden="true">person</i>
                    </span>
                {:else}
                    <IconButton
                        class="material-icons"
                        aria-label={$_('common.remove', { default: 'Remove' })}
                        title={$_('common.remove', { default: 'Remove' })}
                        on:click={() => removeRow(i)}
                    >
                        delete
                    </IconButton>
                {/if}
            </span>
        </div>
    {/each}

    <div class="add-row">
        <!-- A scan adds the row on its own: pointing a camera at one person
             and having it decode IS the choice, so a second tap on the button
             beside it asks the member to confirm something they just did. -->
        <AddressInput
            bind:this={addressField}
            bind:value={newMember}
            on:scanned={(e) => addRow(e.detail)}
            label={$_('support.beneficiary', { default: "Beneficiary's address" })}
            exclude={me === null ? rows.map((r) => r.member) : [me, ...rows.map((r) => r.member)]}
        />
        <Button variant="outlined" disabled={newMember === null} on:click={() => addRow()}>
            <Label>{$_('support.addRow', { default: 'Add beneficiary' })}</Label>
        </Button>
    </div>

    <BondNotice plan={tx.listBeneficiaries(me ?? 0, [])} />

    <div class="page-actions">
        <Button variant="raised" disabled={busy} on:click={save}><Label>{$_('common.save', { default: 'Save' })}</Label></Button>
        <Button variant="outlined" disabled={busy} on:click={() => dispatch('close')}>
            <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
        </Button>
    </div>
    <p class="note">
        {$_('support.weightsNote', {
            default:
                'Shares always total 100% — raising one lowers the others (waterfill). Your own share clears your debts partially, at the balance you choose; at 0% your sales support only your circle.',
        })}
    </p>
</ActionPage>

<style>
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
    }
    /* **A grid, because flex wraps in DOM ORDER and the track is third of
       five.** Measured on a 408 px handset: swatch 12 + name 131 + track 201
       and their gaps fill 365 of the row's 376, so the percentage and the
       action were pushed onto a second line and left sitting under the name,
       detached from the row they describe. Making the cells shrinkable did not
       fix it — flex fills the first line and evicts whatever comes last, which
       is exactly the two things the row exists to show.

       A grid puts each cell where it belongs at either width: on a narrow
       screen the identity, the number and the action share one line and the
       TRACK takes the line below, and above 720 px all five sit in a row. */
    .slider-row {
        display: grid;
        grid-template-columns: auto minmax(0, 1fr) auto auto;
        grid-template-areas:
            'swatch name pct action'
            'track track track track';
        align-items: center;
        column-gap: 10px;
        row-gap: 2px;
    }
    /* One beneficiary: there is no track to place. */
    .slider-row.no-track {
        grid-template-areas: 'swatch name pct action';
    }
    @media (min-width: 720px) {
        .slider-row {
            grid-template-columns: auto minmax(120px, 1fr) minmax(160px, 2fr) auto auto;
            grid-template-areas: 'swatch name track pct action';
        }
        .slider-row.no-track {
            grid-template-areas: 'swatch name . pct action';
        }
    }
    .slider-row.self {
        border-left: 3px solid #1565c0;
        padding-left: 8px;
        border-radius: 4px;
    }
    .swatch {
        grid-area: swatch;
    }
    .slider-name {
        grid-area: name;
        min-width: 0;
        display: inline-flex;
        overflow: hidden;
    }
    .slider-track {
        grid-area: track;
        min-width: 0;
    }
    .slider-pct {
        grid-area: pct;
        min-width: 52px;
        text-align: right;
        font-variant-numeric: tabular-nums;
        font-weight: 600;
    }
    .row-action {
        grid-area: action;
        display: inline-flex;
        justify-content: center;
    }
    .keep-slot {
        width: 48px;
        display: inline-flex;
        justify-content: center;
    }
    .self-icon {
        color: #1565c0;
        font-size: 20px;
    }
    :global(.dark-theme) .self-icon {
        color: #64b5f6;
    }
    .add-row {
        display: flex;
        gap: 12px;
        align-items: flex-end;
        flex-wrap: wrap;
        border-top: 1px dashed var(--mdc-theme-text-hint-on-background, #ddd);
        padding-top: 12px;
    }
    .page-actions {
        display: flex;
        gap: 10px;
        flex-wrap: wrap;
    }
    .note {
        margin: 0;
        font-size: 0.82rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
    }
</style>
