<script lang="ts">
  /**
   * Pre-tour (and post-tour) onboarding wizard.
   *
   * Renders the appropriate step component for the current onboarding
   * step. When the store advances to `tour`, App.svelte unmounts this
   * overlay (so Shepherd can target the real app DOM) and runs the tour
   * directly; once the tour finishes, App.svelte advances the store to
   * `economics-intro` and this overlay remounts for the final explainer.
   */
  import { _ } from 'svelte-i18n';
  import { tick } from 'svelte';

  import { onboardingStep } from '../../common/onboardingStore';

  import LocaleSetupStep from './LocaleSetupStep.svelte';
  import NetworkChoiceStep from './NetworkChoiceStep.svelte';
  import ActorChoiceStep from './ActorChoiceStep.svelte';
  import EconomicsIntroStep from './EconomicsIntroStep.svelte';

  /**
   * The overlay scrolls, and a step change swaps the CONTENT without moving
   * it: leave the explainer from its Continue button at the bottom and the
   * next step opens halfway down itself, below its own title. Reset on every
   * step, after the new content has rendered.
   */
  let overlay: HTMLDivElement | undefined;
  $: if ($onboardingStep && overlay) void scrollToTop();

  async function scrollToTop() {
    await tick();
    overlay?.scrollTo({ top: 0, behavior: 'auto' });
  }
</script>

<div class="overlay" bind:this={overlay} role="dialog" aria-modal="true" aria-labelledby="onboarding-title">
  <div class="shell">
    <header class="shell-header">
      <span id="onboarding-title" class="shell-title">
        {$_('onboarding.header', { default: 'Welcome to edet' })}
      </span>
      <span class="shell-step">
        {#if $onboardingStep === 'locale-setup'}
          {$_('onboarding.steps.locale', { default: 'Language & formats' })}
        {:else if $onboardingStep === 'network-choice'}
          {$_('onboarding.steps.network', { default: 'Your network' })}
        {:else if $onboardingStep === 'actor-choice'}
          {$_('onboarding.steps.actor', { default: 'Who are you?' })}
        {:else if $onboardingStep === 'economics-intro'}
          {$_('onboarding.steps.economics', { default: 'How edet works' })}
        {/if}
      </span>
    </header>

    <div class="shell-body">
      {#if $onboardingStep === 'locale-setup'}
        <LocaleSetupStep />
      {:else if $onboardingStep === 'network-choice'}
        <NetworkChoiceStep />
      {:else if $onboardingStep === 'actor-choice'}
        <ActorChoiceStep />
      {:else if $onboardingStep === 'economics-intro'}
        <EconomicsIntroStep />
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
