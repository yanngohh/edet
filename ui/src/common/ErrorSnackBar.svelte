<script lang="ts">
    import Snackbar, { Label, Actions } from '@smui/snackbar';
    import IconButton from '@smui/icon-button';
    import { errorStore } from './errorStore';
    import { onDestroy } from 'svelte';

    let snackbar: Snackbar;
    let currentError: string = "";
    let currentType: string = "error";
    let lastId: number | null = null;

    const unsubscribe = errorStore.subscribe(errors => {
        if (errors.length > 0) {
            const latest = errors[errors.length - 1];
            if (latest.id !== lastId) {
                currentError = latest.message;
                currentType = latest.type ?? 'error';
                lastId = latest.id;
                if (snackbar) {
                    snackbar.open();
                }
            }
        }
    });

    onDestroy(unsubscribe);
</script>

<!-- The live region is outside the Snackbar so it is always in the DOM.
     When an error appears, screen readers using assertive (role="alert") will
     interrupt the current announcement; polite is used for info/warning.
     SMUI Snackbar may use role="status" internally, which is polite-only.
     This explicit live region guarantees errors are announced immediately. -->
<div
    role={currentType === 'error' ? 'alert' : 'status'}
    aria-live={currentType === 'error' ? 'assertive' : 'polite'}
    aria-atomic="true"
    class="sr-live-region">
    {currentError}
</div>

<Snackbar bind:this={snackbar} leading class="error-snackbar" aria-hidden="true">
    <Label>{currentError}</Label>
    <Actions>
        <IconButton class="material-icons" title="Dismiss" aria-label="Dismiss notification">close</IconButton>
    </Actions>
</Snackbar>

<style>
    /* Visually hidden live region — content announced by screen readers but invisible.
       Using clip-path instead of display:none so the element stays in accessibility tree. */
    .sr-live-region {
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

    :global(.error-snackbar) {
        --mdc-theme-error: #f44336;
    }
</style>
