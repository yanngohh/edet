<script lang="ts" context="module">
    /** Instance counter, so `aria-controls` names one region deterministically
     *  — a random id would differ between renders and defeat an e2e selector. */
    let seq = 0;
</script>

<script lang="ts">
    /**
     * One line of copy, with the rest of it folded behind that line.
     *
     * Every screen carried its explanation as a paragraph pinned open at the
     * top — 25,000 characters across the app against 2,000 behind the wallet's
     * `?` — read once by a newcomer and then re-read every day by somebody who
     * already knows it. This keeps the SENTENCE that says what the screen or
     * the notice IS, which is the part a member re-reads to orient, and folds
     * the mechanism behind ONE affordance in ONE place, so the resting state
     * is short and the explanation is still a tap away.
     *
     * **Not `StatCard`'s `?`, and it must not become it.** That one defines a
     * NUMBER, beside the number, and belongs where a figure needs a unit. This
     * one carries a screen's standing description or a decision's consequence.
     * Sprinkling either through a body is how the app got here.
     *
     * The open state is per instance and NOT persisted: a paragraph that
     * collapses itself because a store remembers a subscription is not a
     * choice the member made, and an expander whose state depends on history
     * cannot be screenshotted or tested. Expanding is the member's click and
     * lasts as long as the page.
     */
    import { _ } from 'svelte-i18n';

    /** The line that always shows. A dozen words: what this is, not how it works. */
    export let summary: string;
    /** Material icon before the summary; '' for none. */
    export let icon = '';
    /**
     * `muted` is the screen's standing description — the old `.intro`.
     * `notice` is a consequence of what is on screen RIGHT NOW: it fires on a
     * condition, so it reads smaller and carries its own icon.
     * `plain` sets neither colour nor size, for use INSIDE a container that
     * already carries them — an alert banner, whose red would otherwise be
     * overwritten and whose 0.85rem would be shrunk a second time.
     */
    export let tone: 'muted' | 'notice' | 'plain' = 'muted';

    let open = false;
    const id = `explain-${(seq += 1)}`;

    $: label = open
        ? $_('common.explainLess', { default: 'Hide the details' })
        : $_('common.explainMore', { default: 'What this means' });
</script>

{#if $$slots.default}
    <div class="explain {tone}">
        <button
            type="button"
            class="row"
            aria-expanded={open}
            aria-controls={id}
            on:click={() => (open = !open)}
        >
            {#if icon}<i class="material-icons lead" aria-hidden="true">{icon}</i>{/if}
            <span class="summary">{summary}</span>
            <i class="material-icons chev" class:open aria-hidden="true">expand_more</i>
            <span class="sr-only">{label}</span>
        </button>
        <!-- Rendered whether or not it is open, so `aria-controls` names an
             element that exists; `hidden` is what a screen reader follows. -->
        <div class="detail" {id} hidden={!open}><slot /></div>
    </div>
{:else}
    <p class="explain {tone} bare">
        {#if icon}<i class="material-icons lead" aria-hidden="true">{icon}</i>{/if}
        <span class="summary">{summary}</span>
    </p>
{/if}

<style>
    .explain {
        margin: 0;
        line-height: 1.5;
    }
    .muted {
        color: var(--mdc-theme-text-secondary-on-surface, #666);
    }
    .bare {
        display: flex;
        gap: 0.6em;
        align-items: flex-start;
    }
    /* The whole line is the target: on a handset a bare chevron is a 24px
       hit area beside 300px of text that looks like it should respond. */
    .row {
        display: flex;
        gap: 0.6em;
        align-items: flex-start;
        width: 100%;
        background: none;
        border: none;
        padding: 2px 0;
        margin: 0;
        font: inherit;
        color: inherit;
        text-align: left;
        cursor: pointer;
    }
    .row:hover .summary,
    .row:focus-visible .summary {
        color: var(--mdc-theme-primary);
    }
    .row:focus-visible {
        outline: 2px solid var(--mdc-theme-primary);
        outline-offset: 2px;
        border-radius: 4px;
    }
    .summary {
        flex: 1 1 auto;
        min-width: 0;
    }
    .lead {
        font-size: 1.2em;
        flex: none;
        margin-top: 0.1em;
    }
    .chev {
        flex: none;
        font-size: 1.25em;
        opacity: 0.7;
        transition: transform 120ms ease;
    }
    .chev.open {
        transform: rotate(180deg);
    }
    /* The only thing that needs a reading width. The summary row above it is
       one line, so it takes the whole column and its chevron lands on the same
       right edge as every card and rule on the page. */
    .detail {
        padding: 6px 0 2px 0;
        max-width: var(--edet-measure);
    }
    .detail[hidden] {
        display: none;
    }

    /* A conditional consequence, not the standing description. Smaller and
       dimmer than the body it interrupts, the way `.stranger-note` was. */
    .notice {
        font-size: 0.85em;
        line-height: 1.45;
        opacity: 0.85;
    }

    .sr-only {
        position: absolute;
        width: 1px;
        height: 1px;
        padding: 0;
        margin: -1px;
        overflow: hidden;
        clip: rect(0, 0, 0, 0);
        white-space: nowrap;
        border: 0;
    }
</style>
