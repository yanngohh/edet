<script lang="ts">
    /** One wallet/dashboard metric with a plain-language help toggle. */
    export let label: string;
    export let value: string;
    export let icon: string = '';
    export let help: string = '';
    /** A second line under the value, always shown: the same quantity said
     *  another way, where the figure alone does not place it. */
    export let detail: string = '';
    /** Visual accent: 'default' | 'good' | 'warn' | 'bad'. */
    export let tone: 'default' | 'good' | 'warn' | 'bad' = 'default';

    let showHelp = false;
</script>

<div class="stat-card tone-{tone}">
    <div class="stat-top">
        <span class="stat-label">
            {#if icon}<i class="material-icons stat-icon" aria-hidden="true">{icon}</i>{/if}
            {label}
        </span>
        {#if help}
            <button
                class="help-btn"
                type="button"
                aria-label={`Help: ${label}`}
                aria-expanded={showHelp}
                on:click={() => (showHelp = !showHelp)}
            >
                <i class="material-icons" aria-hidden="true">help_outline</i>
            </button>
        {/if}
    </div>
    <div class="stat-value">{value}</div>
    {#if detail}<div class="stat-detail">{detail}</div>{/if}
    {#if showHelp && help}
        <p class="stat-help">{help}</p>
    {/if}
</div>

<style>
    .stat-detail {
        font-size: 0.82rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
        font-variant-numeric: tabular-nums;
    }
    .stat-card {
        background: var(--mdc-theme-surface, #fff);
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-left: 4px solid var(--mdc-theme-primary);
        border-radius: 8px;
        padding: 12px 16px;
        display: flex;
        flex-direction: column;
        gap: 4px;
        min-width: 0;
    }
    /* Three sides, never the shorthand and never the left.
       `border-color` sets all four, and `:global(.dark-theme) .stat-card` is
       two classes to `.tone-good`'s one — so restating the left edge here
       outranked every tone and painted all seven cards primary in the dark
       theme while the light theme showed them correctly. The left edge is a
       status channel; it belongs to the tone classes alone. */
    :global(.dark-theme) .stat-card {
        background: #1e1e1e;
        border-top-color: rgba(255, 255, 255, 0.1);
        border-right-color: rgba(255, 255, 255, 0.1);
        border-bottom-color: rgba(255, 255, 255, 0.1);
    }
    .tone-good {
        border-left-color: #43a047;
    }
    .tone-warn {
        border-left-color: #f9a825;
    }
    .tone-bad {
        border-left-color: #d32f2f;
    }
    .stat-top {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 8px;
    }
    .stat-label {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-size: 0.8rem;
        letter-spacing: 0.02em;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .stat-icon {
        font-size: 16px;
        color: var(--mdc-theme-primary);
    }
    .stat-value {
        font-size: 1.4rem;
        font-weight: 600;
        color: var(--mdc-theme-on-surface);
        overflow-wrap: anywhere;
    }
    .help-btn {
        background: none;
        border: none;
        padding: 0;
        cursor: pointer;
        color: var(--mdc-theme-text-secondary-on-surface, #999);
        display: inline-flex;
    }
    .help-btn i {
        font-size: 18px;
    }
    .stat-help {
        margin: 4px 0 0;
        font-size: 0.82rem;
        line-height: 1.45;
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
</style>
