<script lang="ts">
  /**
   * Pre-tour onboarding wizard.
   *
   * Renders the appropriate step component for the current onboarding
   * step. When the store advances to `tour`, `App.svelte` unmounts this
   * overlay (so Shepherd can target the real app DOM) and runs the tour
   * directly.
   */
  import { _ } from 'svelte-i18n';

  import { onboardingStep } from '../../common/onboardingStore';

  import LocaleSetupStep from './LocaleSetupStep.svelte';
  import PathChoiceStep from './PathChoiceStep.svelte';
  import MnemonicGenerate from './MnemonicGenerate.svelte';
  import MnemonicConfirm from './MnemonicConfirm.svelte';
  import BackupExport from './BackupExport.svelte';
  import RestoreBackupStep from './RestoreBackupStep.svelte';
</script>

<div class="overlay" role="dialog" aria-modal="true" aria-labelledby="onboarding-title">
  <div class="shell">
    <header class="shell-header">
      <span id="onboarding-title" class="shell-title">
        {$_('onboarding.header', { default: 'Welcome to edet' })}
      </span>
      <span class="shell-step">
        {#if $onboardingStep === 'locale-setup'}
          {$_('onboarding.steps.locale', { default: 'Language & formats' })}
        {:else if $onboardingStep === 'path-choice'}
          {$_('onboarding.steps.pathChoice', { default: 'Create or restore' })}
        {:else if $onboardingStep === 'mnemonic-generate' || $onboardingStep === 'mnemonic-confirm'}
          {$_('onboarding.steps.mnemonic', { default: 'Recovery phrase' })}
        {:else if $onboardingStep === 'backup-export'}
          {$_('onboarding.steps.backup', { default: 'Backup file' })}
        {:else if $onboardingStep === 'restore-backup'}
          {$_('onboarding.steps.restore', { default: 'Restore identity' })}
        {/if}
      </span>
    </header>

    <div class="shell-body">
      {#if $onboardingStep === 'locale-setup'}
        <LocaleSetupStep />
      {:else if $onboardingStep === 'path-choice'}
        <PathChoiceStep />
      {:else if $onboardingStep === 'mnemonic-generate'}
        <MnemonicGenerate />
      {:else if $onboardingStep === 'mnemonic-confirm'}
        <MnemonicConfirm />
      {:else if $onboardingStep === 'backup-export'}
        <BackupExport />
      {:else if $onboardingStep === 'restore-backup'}
        <RestoreBackupStep />
      {/if}
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 5000;
    background: var(--mdc-theme-background, #f7f7f7);
    overflow-y: auto;
  }
  :global(.dark-theme) .overlay {
    background: #121212;
  }
  .shell {
    max-width: 760px;
    margin: 48px auto;
    padding: 24px;
    background: var(--mdc-theme-surface, #fff);
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    border-radius: 12px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.08);
  }
  :global(.dark-theme) .shell {
    background: #1e1e1e;
    border-color: rgba(255, 255, 255, 0.1);
  }
  .shell-header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    border-bottom: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.08));
    padding-bottom: 12px;
    margin-bottom: 24px;
  }
  .shell-title {
    font-size: 1.25rem;
    font-weight: 600;
    color: var(--mdc-theme-primary);
  }
  .shell-step {
    font-size: 0.85rem;
    color: var(--mdc-theme-text-secondary-on-surface);
  }
</style>
