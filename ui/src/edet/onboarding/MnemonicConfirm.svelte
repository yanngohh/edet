<script lang="ts">
  /**
   * Step 3 of onboarding: prove the user wrote the mnemonic down by asking
   * them to retype three randomly chosen positions.
   *
   * Three strikes and we force a regeneration (back to step 2). We compare
   * each answer against the in-memory mnemonic held by the onboarding store;
   * at no point is the plaintext phrase persisted.
   *
   * On Tauri: after the user correctly retypes the phrase we install edet
   * under the derived agent key before advancing to BackupExport. This is
   * the earliest safe moment to install — the seed is confirmed in memory
   * and the conductor is ready — and BackupExport needs an active
   * AppClient to call appInfo().
   *
   * On hc-spin/browser: the app is already installed by hc-spin; we skip
   * the install step and advance directly.
   */
  import { _ } from 'svelte-i18n';
  import { get } from 'svelte/store';
  import { getContext } from 'svelte';
  import Textfield from '@smui/textfield';
  import Button, { Label } from '@smui/button';

  import { onboardingState, goToStep, advance, markMnemonicConfirmed } from '../../common/onboardingStore';
  import { pickDistinctIndices } from '../../common/mnemonic';
  import { clientContext } from '../../contexts';
  import { tauriBridge } from '../../common/tauriBridge';

  const NUM_CHECKS = 3;
  const MAX_ATTEMPTS = 3;

  // Must be called at component init time, not inside an event handler.
  const appContext = getContext(clientContext) as any;

  let indices: number[] = [];
  let answers: string[] = [];
  let attempts = 0;
  let error: string | null = null;
  let installing = false;

  $: mnemonicWords = (get(onboardingState).mnemonic ?? '').split(' ');

  // Initialise once the mnemonic is available. `mnemonicWords` may be empty
  // briefly if the store is repopulated.
  $: if (mnemonicWords.length === 12 && indices.length === 0) {
    indices = pickDistinctIndices(NUM_CHECKS, mnemonicWords.length);
    answers = indices.map(() => '');
  }

  async function onSubmit() {
    error = null;
    const correct = indices.every((idx, i) => {
      return mnemonicWords[idx]?.toLowerCase().trim() === answers[i]?.toLowerCase().trim();
    });
    if (correct) {
      markMnemonicConfirmed();

      // Tauri path: import the agent seed into lair, install edet, and
      // open the AppWebsocket before advancing. BackupExport needs the
      // client to be live when it mounts.
      if (tauriBridge.isTauri()) {
        installing = true;
        try {
          const state = get(onboardingState);
          if (!state.derived) throw new Error('derived keys missing');
          await appContext.installAndConnect(state.derived.agentSeed);
        } catch (e: any) {
          error = e?.message ?? String(e);
          installing = false;
          return;
        }
        installing = false;
      }

      advance();
      return;
    }
    attempts += 1;
    if (attempts >= MAX_ATTEMPTS) {
      error = $_(
        'onboarding.confirm.tooManyAttempts',
        { default: "Too many incorrect attempts. Let's generate a new phrase." },
      );
      // Short delay so the user can read the message before being redirected.
      setTimeout(() => goToStep('mnemonic-generate'), 1500);
      return;
    }
    error = $_(
      'onboarding.confirm.incorrect',
      { values: { remaining: MAX_ATTEMPTS - attempts }, default: "That doesn't match. {remaining} attempts remaining." },
    );
  }
</script>

<div class="step">
  <h2 class="step-title">{$_('onboarding.confirm.title', { default: 'Confirm your recovery phrase' })}</h2>
  <p class="step-body">
    {$_('onboarding.confirm.body', {
      default: 'Type the missing words to confirm you have a copy of your phrase.',
    })}
  </p>

  {#each indices as idx, i}
    <div class="field">
      <Textfield
        bind:value={answers[i]}
        label={$_('onboarding.confirm.wordAt', { values: { n: idx + 1 }, default: 'Word #{n}' })}
        input$autocomplete="off"
        input$autocapitalize="off"
        input$spellcheck="false"
      />
    </div>
  {/each}

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <div class="actions-row">
    <Button variant="outlined" on:click={() => goToStep('mnemonic-generate')} disabled={installing}>
      <Label>{$_('onboarding.confirm.back', { default: 'Show phrase again' })}</Label>
    </Button>
    <Button
      variant="raised"
      on:click={onSubmit}
      disabled={answers.some((a) => a.trim().length === 0) || installing}
    >
      <Label>
        {#if installing}
          <span class="btn-busy">
            <span class="spinner" aria-hidden="true"></span>
            {$_('onboarding.confirm.installing')}
          </span>
        {:else}
          {$_('onboarding.confirm.submit')}
        {/if}
      </Label>
    </Button>
  </div>
</div>

<style>
  .btn-busy {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .spinner {
    display: inline-block;
    width: 14px;
    height: 14px;
    border: 2px solid currentColor;
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    flex-shrink: 0;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .step {
    display: flex;
    flex-direction: column;
    gap: 18px;
    max-width: 520px;
    margin: 0 auto;
  }
  .step-title {
    margin: 0;
    color: var(--mdc-theme-primary);
  }
  .step-body {
    margin: 0;
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.5;
  }
  .field :global(.mdc-text-field) {
    width: 100%;
  }
  .error {
    color: var(--mdc-theme-error, #d32f2f);
    font-weight: 600;
    margin: 0;
  }
  .actions-row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
</style>
