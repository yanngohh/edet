<script lang="ts">
    /**
     * The passphrase gate: shown before anything else when this device's vault
     * has one, and dismissed only by entering it.
     *
     * **What it is worth, stated plainly.** The device key alone is available
     * to whoever holds the phone unlocked — that is what an OS keychain gives
     * a running app. A passphrase is the second factor: it is never stored,
     * and the seeds cannot be read by anybody, this app included, until it is
     * entered. It is also unrecoverable, which is why the copy below says so
     * and points at the recovery phrase rather than at a reset.
     */
    import { _ } from 'svelte-i18n';
    import Button, { Label } from '@smui/button';
    import Textfield from '@smui/textfield';
    import CircularProgress from '@smui/circular-progress';

    import { unlock } from '../common/vault';

    export let onUnlocked: () => void;

    let passphrase = '';
    let busy = false;
    let wrong = false;

    async function submit(): Promise<void> {
        if (busy || passphrase === '') return;
        busy = true;
        wrong = false;
        try {
            // The derivation is deliberately slow (scrypt at N=2^15), which is
            // what makes a short passphrase worth having — so this is an
            // awaited call with a spinner rather than a synchronous check.
            if (await unlock(passphrase)) {
                passphrase = '';
                onUnlocked();
                return;
            }
            wrong = true;
        } finally {
            busy = false;
        }
    }
</script>

<div class="full-page-center">
    <form class="unlock" on:submit|preventDefault={submit}>
        <i class="material-icons unlock-icon" aria-hidden="true">lock</i>
        <h2>{$_('unlock.title', { default: 'Unlock your identity' })}</h2>
        <p class="muted">
            {$_('unlock.body', {
                default:
                    'Your seeds are encrypted with this passphrase as well as with this device. Nobody can read them without it — this app included.',
            })}
        </p>
        <Textfield
            bind:value={passphrase}
            type="password"
            label={$_('unlock.passphrase', { default: 'Passphrase' })}
            input$autocomplete="current-password"
            input$autofocus
            style="width: 100%"
            invalid={wrong}
        />
        {#if wrong}
            <p class="error" role="alert">
                {$_('unlock.wrong', { default: 'That passphrase does not open this vault.' })}
            </p>
        {/if}
        <Button variant="raised" type="submit" disabled={busy || passphrase === ''} style="width: 100%">
            {#if busy}
                <CircularProgress style="height: 20px; width: 20px;" indeterminate />
            {:else}
                <Label>{$_('unlock.action', { default: 'Unlock' })}</Label>
            {/if}
        </Button>
        <p class="muted small">
            {$_('unlock.forgotten', {
                default:
                    'Forgotten it? There is no reset — that is what makes it worth having. Restore this identity from its recovery phrase on a fresh profile.',
            })}
        </p>
    </form>
</div>

<style>
    .unlock {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 14px;
        max-width: 380px;
        text-align: center;
    }
    .unlock-icon {
        font-size: 40px;
        color: var(--mdc-theme-primary, #6200ee);
    }
    h2 {
        margin: 0;
        font-size: 1.25rem;
    }
    .muted {
        margin: 0;
        color: var(--mdc-theme-text-secondary-on-background, #666);
    }
    .small {
        font-size: 0.85rem;
    }
    .error {
        margin: 0;
        color: var(--mdc-theme-error, #d32f2f);
    }
</style>
