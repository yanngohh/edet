<script lang="ts">
  /**
   * Onboarding wrapper around `RestoreScreen`.
   *
   * The RestoreScreen component already handles the full
   * mnemonic-in / backup-file-in / graft-on-success flow (Tauri path)
   * and the "desktop only" explanation (browser path). This wrapper
   * plugs its `restored` and `cancel` events into the onboarding store
   * so the wizard advances correctly:
   *
   *  - `restored` → connect the AppWebsocket (app is now installed),
   *                 mark restore complete, then advance to the tour.
   *  - `cancel`   → clear the path choice and return to PathChoiceStep.
   */
  import { getContext } from 'svelte';
  import {
    clearPathChoice,
    markRestoreCompleted,
    advance,
  } from '../../common/onboardingStore';
  import { clientContext } from '../../contexts';

  import RestoreScreen from './RestoreScreen.svelte';

  const appContext = getContext(clientContext) as any;

  async function onRestored() {
    // The app is installed by RestoreScreen; open the websocket now so
    // the tour finds an active client and all components render properly.
    try {
      await appContext.connectOnly();
    } catch (e: any) {
      console.error('[RestoreBackupStep] connectOnly failed:', e);
    }
    markRestoreCompleted();
    advance();
  }

  function onCancel() {
    clearPathChoice();
    advance();
  }
</script>

<div class="step">
  <RestoreScreen on:restored={onRestored} on:cancel={onCancel} />
</div>

<style>
  /* The inner RestoreScreen already owns padding and layout; we simply
     centre it inside the onboarding shell. */
  .step {
    display: flex;
    justify-content: center;
  }
</style>
