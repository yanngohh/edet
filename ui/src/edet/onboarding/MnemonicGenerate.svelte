<script lang="ts">
  /**
   * Step 2 of onboarding: generate + display the BIP-39 recovery mnemonic.
   *
   * The mnemonic is kept in memory in the onboarding store until the
   * confirmation step writes the derived keys to storage; if the user
   * abandons the wizard the mnemonic is zeroed. The screen deliberately
   * does not persist the plaintext phrase anywhere.
   */
  import { _ } from 'svelte-i18n';
  import { onMount, onDestroy } from 'svelte';
  import Button, { Label } from '@smui/button';
  import Icon from '@smui/icon-button';
  import Snackbar, { Label as SnackLabel } from '@smui/snackbar';

  import { generateNewMnemonic, deriveKeys } from '../../common/mnemonic';
  import { setMnemonic, advance } from '../../common/onboardingStore';
  import { copyToClipboard } from '../../common/clipboard';

  let words: string[] = [];
  let acknowledged = false;
  let copySnackbar: Snackbar;

  onMount(() => {
    regenerate();
  });

  onDestroy(() => {
    // Clear local bindings on unmount — the orchestrator keeps authoritative
    // state in `onboardingStore`.
    words = [];
  });

  async function regenerate() {
    const mnemonic = generateNewMnemonic();
    const derived = await deriveKeys(mnemonic);
    words = mnemonic.split(' ');
    setMnemonic(mnemonic, derived);
  }

  function copy() {
    copyToClipboard(words.join(' '));
    copySnackbar?.open();
  }

  function onContinue() {
    if (!acknowledged) return;
    advance();
  }
</script>

<div class="step">
  <h2 class="step-title">
    {$_('onboarding.mnemonic.title', { default: 'Write down these 12 words' })}
  </h2>

  <div class="warning" role="alert">
    <i class="material-icons" aria-hidden="true">warning</i>
    <div>
      <strong>{$_('onboarding.mnemonic.warningHeader', { default: 'This is your only recovery phrase.' })}</strong>
      <p>
        {$_('onboarding.mnemonic.warningBody', {
          default:
            "edet keeps an encrypted backup of your account on this device. To restore your wallet, reputation, and history on another device you will need BOTH this 12-word phrase AND your backup file. If you lose the phrase, no one — not even us — can recover your account.",
        })}
      </p>
    </div>
  </div>

  <ol class="words" aria-label={$_('onboarding.mnemonic.wordsLabel', { default: 'Recovery phrase' })}>
    {#each words as word, idx}
      <li><span class="word-index">{idx + 1}</span><span class="word-value">{word}</span></li>
    {/each}
  </ol>

  <div class="actions-row">
    <Button on:click={copy} variant="outlined">
      <Icon class="material-icons">content_copy</Icon>
      <Label>{$_('onboarding.mnemonic.copy', { default: 'Copy to clipboard' })}</Label>
    </Button>
    <Button on:click={regenerate} variant="outlined">
      <Icon class="material-icons">refresh</Icon>
      <Label>{$_('onboarding.mnemonic.regenerate', { default: 'Regenerate' })}</Label>
    </Button>
  </div>

  <label class="confirm">
    <input type="checkbox" bind:checked={acknowledged} />
    <span>{$_('onboarding.mnemonic.acknowledge', { default: "I have written down my 12 words and stored them safely." })}</span>
  </label>

  <div class="actions-row end">
    <Button variant="raised" disabled={!acknowledged} on:click={onContinue}>
      <Label>{$_('onboarding.mnemonic.continue', { default: 'Continue' })}</Label>
    </Button>
  </div>

  <Snackbar bind:this={copySnackbar} leading>
    <SnackLabel>{$_('onboarding.mnemonic.copied', { default: 'Recovery phrase copied' })}</SnackLabel>
  </Snackbar>
</div>

<style>
  .step {
    display: flex;
    flex-direction: column;
    gap: 18px;
    max-width: 640px;
    margin: 0 auto;
  }
  .step-title {
    margin: 0;
    color: var(--mdc-theme-primary);
  }
  .warning {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 16px;
    border-radius: 8px;
    border: 1px solid rgba(211, 47, 47, 0.45);
    background: rgba(211, 47, 47, 0.06);
    color: var(--mdc-theme-on-surface);
  }
  .warning i {
    color: var(--mdc-theme-error, #d32f2f);
    font-size: 28px;
    flex-shrink: 0;
  }
  .warning p {
    margin: 6px 0 0 0;
    line-height: 1.5;
  }
  .words {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
  }
  @media (max-width: 520px) {
    .words {
      grid-template-columns: repeat(2, 1fr);
    }
  }
  .words li {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 10px 12px;
    background: var(--mdc-theme-background, #fafafa);
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    border-radius: 6px;
    font-family: monospace;
    font-size: 1.05rem;
  }
  :global(.dark-theme) .words li {
    background: rgba(255, 255, 255, 0.04);
    border-color: rgba(255, 255, 255, 0.1);
  }
  .word-index {
    color: var(--mdc-theme-text-secondary-on-surface);
    font-size: 0.75rem;
    font-weight: 600;
  }
  .word-value {
    font-weight: 600;
    color: var(--mdc-theme-on-surface);
  }
  .actions-row {
    display: flex;
    gap: 12px;
  }
  .actions-row.end {
    justify-content: flex-end;
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px;
    background: var(--mdc-theme-background, #fafafa);
    border-radius: 6px;
  }
  :global(.dark-theme) .confirm {
    background: rgba(255, 255, 255, 0.04);
  }
</style>
