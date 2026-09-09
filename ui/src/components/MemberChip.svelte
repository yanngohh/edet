<script lang="ts">
    import AgentAvatar from './AgentAvatar.svelte';
    import { addressOf, memberName } from '../lib/display';

    export let memberId: number;
    export let size = 24;
    /** Extra text after the name (e.g. a status). */
    export let detail: string = '';
</script>

<span class="member-chip" title={$addressOf(memberId) ?? undefined}>
    <AgentAvatar {memberId} {size} />
    <span class="member-chip-name">{$memberName(memberId)}</span>
    {#if detail}
        <span class="member-chip-detail">{detail}</span>
    {/if}
</span>

<style>
    .member-chip {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        vertical-align: middle;
    }
    .member-chip-name {
        font-weight: 500;
    }
    .member-chip-detail {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
        font-size: 0.85em;
    }
</style>
