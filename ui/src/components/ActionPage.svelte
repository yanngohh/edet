<script lang="ts">
    /**
     * The frame every FAB-opened page shares: a back arrow, a title, and the
     * page body. Sections stay read-only surfaces; anything that creates or
     * configures opens one of these over the section, and closes back onto
     * it (list → FAB → page → back).
     */
    import { _ } from 'svelte-i18n';
    import { createEventDispatcher } from 'svelte';
    import IconButton from '@smui/icon-button';

    export let title: string;
    /** Material icon shown next to the title. */
    export let icon = '';

    const dispatch = createEventDispatcher<{ close: void }>();
</script>

<div class="action-page flex-column">
    <div class="page-head">
        <IconButton
            class="material-icons"
            aria-label={$_('common.back', { default: 'Back' })}
            on:click={() => dispatch('close')}
        >
            arrow_back
        </IconButton>
        <h3 class="page-title">
            {#if icon}<i class="material-icons" aria-hidden="true">{icon}</i>{/if}
            {title}
        </h3>
    </div>
    <div class="page-body flex-column">
        <slot />
    </div>
</div>

<style>
    .action-page {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
        gap: 16px;
    }
    .page-head {
        display: flex;
        align-items: center;
        gap: 4px;
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
    }
    .page-title {
        margin: 0;
        display: flex;
        align-items: center;
        gap: 8px;
        color: var(--mdc-theme-primary);
        font-weight: 500;
        font-size: 1.15rem;
    }
    .page-body {
        gap: 16px;
        max-width: var(--edet-form);
    }
</style>
