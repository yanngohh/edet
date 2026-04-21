<script lang="ts">
  /**
   * Step 2 of onboarding: ask the user whether they are setting up a brand
   * new identity or restoring an existing one from a backup file.
   *
   * The two paths diverge from here:
   *  - "Create new identity" → generate a fresh mnemonic, confirm it,
   *    save an initial backup file, then walk through the tour.
   *  - "Restore from backup" → hand control to the RestoreBackup step
   *    which reuses the standalone RestoreScreen and, on success, jumps
   *    straight to the tour.
   *
   * Runtime gating — Restore requires privileged conductor access
   * (lair-seed import + admin-websocket install + graft_records). Only
   * the Tauri desktop shell provides these; in hc-spin / browser
   * contexts the Restore card renders as a disabled button with a
   * hover tooltip rather than routing to a dead-end screen. Mobile
   * support is deferred.
   *
   * See `onboardingStore.chooseNext()` for the full decision matrix.
   */
  import { _ } from 'svelte-i18n';

  import { choosePath, advance } from '../../common/onboardingStore';
  import { tauriBridge } from '../../common/tauriBridge';

  // `isTauri()` is stable across the lifetime of the renderer so we
  // evaluate it once at component init; there's no value in re-reading
  // it on every render.
  const restoreAvailable = tauriBridge.isTauri();

  function onCreate() {
    choosePath('create');
    advance();
  }

  function onRestore() {
    // Defensive: a disabled button should not fire, but if CSS/DOM
    // somehow lets a click slip through we still refuse to proceed
    // rather than show the dead-end RestoreScreen.
    if (!restoreAvailable) return;
    choosePath('restore');
    advance();
  }
</script>

<div class="step">
  <h2 class="step-title">
    {$_('onboarding.pathChoice.title', { default: 'Create or restore your identity' })}
  </h2>
  <p class="step-body">
    {$_('onboarding.pathChoice.body', {
      default:
        "If this is your first time using edet on this device, create a new identity. If you already have a 12-word recovery phrase and a backup file from a previous install, restore them now.",
    })}
  </p>

  <div class="cards">
    <button type="button" class="card" on:click={onCreate}>
      <span class="card-icon material-icons" aria-hidden="true">add_circle_outline</span>
      <span class="card-title">{$_('onboarding.pathChoice.createTitle', { default: 'Create new identity' })}</span>
      <span class="card-body">
        {$_('onboarding.pathChoice.createBody', {
          default:
            "Generate a new 12-word recovery phrase. Use this if you've never set up edet before on any device.",
        })}
      </span>
    </button>

    <button
      type="button"
      class="card"
      on:click={onRestore}
      disabled={!restoreAvailable}
      aria-disabled={!restoreAvailable}
      title={!restoreAvailable
        ? $_('onboarding.pathChoice.restoreUnavailable', {
            default: 'Restore is available in the edet desktop app only. Mobile support is coming.',
          })
        : undefined}
    >
      <span class="card-icon material-icons" aria-hidden="true">restore</span>
      <span class="card-title">{$_('onboarding.pathChoice.restoreTitle', { default: 'Restore from backup' })}</span>
      <span class="card-body">
        {$_('onboarding.pathChoice.restoreBody', {
          default:
            'Bring your wallet, reputation, and history to this device. Requires both your 12-word recovery phrase AND the backup file you previously saved.',
        })}
      </span>
      {#if !restoreAvailable}
        <span class="card-unavailable">
          {$_('onboarding.pathChoice.restoreUnavailable', {
            default: 'Restore is available in the edet desktop app only. Mobile support is coming.',
          })}
        </span>
      {/if}
    </button>
  </div>
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
  .step-body {
    margin: 0;
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.5;
  }
  .cards {
    display: grid;
    grid-template-columns: 1fr;
    gap: 14px;
  }
  @media (min-width: 560px) {
    .cards {
      grid-template-columns: 1fr 1fr;
    }
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 20px;
    border-radius: 8px;
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    background: var(--mdc-theme-surface, #fff);
    text-align: left;
    cursor: pointer;
    font-family: inherit;
    color: inherit;
    transition: border-color 0.15s ease, box-shadow 0.15s ease, transform 0.05s ease;
  }
  .card:hover,
  .card:focus {
    border-color: var(--mdc-theme-primary);
    box-shadow: 0 4px 14px rgba(98, 0, 238, 0.12);
    outline: none;
  }
  .card:active {
    transform: translateY(1px);
  }
  .card[disabled] {
    opacity: 0.55;
    cursor: not-allowed;
    /* Keep the element interactive enough for the `title` tooltip to
       fire on hover, but avoid the hover-lift effect reserved for
       actionable cards. */
    box-shadow: none;
    border-color: var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    transform: none;
  }
  .card[disabled]:hover,
  .card[disabled]:focus {
    border-color: var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    box-shadow: none;
  }
  :global(.dark-theme) .card {
    background: #1e1e1e;
    border-color: rgba(255, 255, 255, 0.1);
  }
  :global(.dark-theme) .card:hover,
  :global(.dark-theme) .card:focus {
    border-color: var(--mdc-theme-primary);
    box-shadow: 0 4px 14px rgba(156, 39, 176, 0.25);
  }
  :global(.dark-theme) .card[disabled]:hover,
  :global(.dark-theme) .card[disabled]:focus {
    border-color: rgba(255, 255, 255, 0.1);
    box-shadow: none;
  }
  .card-icon {
    font-size: 28px !important;
    color: var(--mdc-theme-primary);
  }
  .card-title {
    font-weight: 600;
    font-size: 1.05rem;
    color: var(--mdc-theme-on-surface);
  }
  .card-body {
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.45;
    font-size: 0.95rem;
  }
  /* Inline reason shown beneath the Restore card's body when the
     runtime doesn't support the feature. Visible to every user so
     they understand the greyed-out state even without hovering to
     trigger the native `title` tooltip. */
  .card-unavailable {
    margin-top: 4px;
    font-size: 0.85rem;
    font-style: italic;
    color: var(--mdc-theme-text-secondary-on-surface, rgba(0, 0, 0, 0.6));
  }
</style>
