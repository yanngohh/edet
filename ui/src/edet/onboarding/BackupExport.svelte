<script lang="ts">
  /**
   * Step 4 of onboarding: persist the first backup bundle and offer to export
   * a copy to the filesystem.
   *
   * At this point the mnemonic has been generated, confirmed, and the derived
   * keys are cached in the onboarding store. We:
   *   1. Dump the source chain via `tauriBridge` (a stub in dev mode).
   *   2. Encrypt + msgpack-encode the payload with the backup key.
   *   3. Write the ciphertext to local persistence (`backupStorage`).
   *   4. Prompt the user to save an external copy.
   *
   * The copy prompt can be skipped — the internal copy is already safe
   * against a reinstall on the same device — but we strongly encourage it.
   */
  import { _ } from 'svelte-i18n';
  import { get } from 'svelte/store';
  import { getContext, onMount } from 'svelte';
  import type { AppClient } from '@holochain/client';
  import Button, { Label } from '@smui/button';
  import CircularProgress from '@smui/circular-progress';

  import { clientContext } from '../../contexts';
  import { onboardingState, markBackupSaved, advance } from '../../common/onboardingStore';
  import {
    writeCurrentBackup,
    exportBackupCopy,
  } from '../../common/backupStorage';
  import { encodeBackup, suggestedBackupFileName } from '../../common/backup';
  import type { BackupPayload } from '../../common/backup';
  import { tauriBridge } from '../../common/tauriBridge';
  import { wrapAndStoreBackupKey } from '../../common/deviceKey';

  const client: AppClient = (getContext(clientContext) as any).getClient();

  let preparing = true;
  let saving = false;
  let exported = false;
  let errorMessage: string | null = null;

  onMount(async () => {
    try {
      await prepareBackup();
    } catch (e: any) {
      errorMessage = e?.message ?? String(e);
    } finally {
      preparing = false;
    }
  });

  async function prepareBackup() {
    const state = get(onboardingState);
    if (!state.derived || !state.mnemonic) {
      throw new Error('Internal state missing: mnemonic not yet generated');
    }

    // Source-chain dump. In the Tauri shell this hits the admin websocket;
    // in dev it returns an empty array (acceptable — the initial backup is
    // still useful as a "please protect your mnemonic" trigger).
    let sourceChain: unknown[] = [];
    try {
      const appInfo = await client.appInfo();
      const cellInfo = appInfo?.cell_info?.edet?.[0];
      const cellId =
        cellInfo && 'provisioned' in cellInfo
          ? (cellInfo as any).provisioned?.cell_id
          : (cellInfo as any)?.cell_id;
      if (cellId) {
        sourceChain = await tauriBridge.dumpSourceChain(cellId);
      }
    } catch (e) {
      console.warn('BackupExport: dumpSourceChain failed, saving empty chain', e);
    }

    const dnaHash = (await client.appInfo())?.cell_info?.edet?.[0];
    const cellInfo = dnaHash as any;
    const cellIdPair: [Uint8Array, Uint8Array] | undefined =
      cellInfo?.provisioned?.cell_id ?? cellInfo?.cell_id;

    const payload: BackupPayload = {
      schema_version: 1,
      created_at_us: Date.now() * 1000,
      app_id: 'edet',
      dna_hash: cellIdPair ? cellIdPair[0] : new Uint8Array(0),
      agent_pub_key: client.myPubKey,
      source_chain: sourceChain,
    };

    const ciphertext = encodeBackup(payload, state.derived.backupKey, state.derived.fingerprint);
    await writeCurrentBackup(ciphertext, client.myPubKey);
    // Persist the mnemonic-derived backup key wrapped with a fresh device
    // key so the scheduler can produce fresh backups after the mnemonic
    // leaves memory. This does NOT weaken recovery — the mnemonic alone
    // still unlocks any existing bundle on any device.
    await wrapAndStoreBackupKey(state.derived.backupKey);
    markBackupSaved();
  }

  async function onExportCopy() {
    saving = true;
    try {
      const ok = await exportBackupCopy(suggestedBackupFileName());
      exported = ok;
    } catch (e: any) {
      errorMessage = e?.message ?? String(e);
    } finally {
      saving = false;
    }
  }
</script>

<div class="step">
  <h2 class="step-title">{$_('onboarding.backup.title', { default: 'Save your backup file' })}</h2>

  {#if preparing}
    <div class="center"><CircularProgress class="circular-progress" indeterminate /></div>
    <p>{$_('onboarding.backup.preparing', { default: 'Preparing your first backup…' })}</p>
  {:else if errorMessage}
    <p class="error" role="alert">{errorMessage}</p>
    <div class="actions-row end">
      <Button variant="outlined" on:click={() => advance()}>
        <Label>{$_('onboarding.backup.skip', { default: 'Continue without saving a copy' })}</Label>
      </Button>
    </div>
  {:else}
    <p class="step-body">
      {$_('onboarding.backup.body', {
        default:
          'Your encrypted backup is stored on this device. For full recovery on a new device you should also save a copy in a place you control — a password manager, a USB drive, or your cloud storage. The file is useless without your 12-word phrase.',
      })}
    </p>

    <div class="actions-row">
      <Button variant="raised" on:click={onExportCopy} disabled={saving}>
        <Label>
          {exported
            ? $_('onboarding.backup.exportedAgain', { default: 'Save another copy' })
            : $_('onboarding.backup.export', { default: 'Save backup file…' })}
        </Label>
      </Button>
      <Button variant="outlined" on:click={() => advance()}>
        <Label>
          {exported
            ? $_('onboarding.backup.continue', { default: 'Continue' })
            : $_('onboarding.backup.skip', { default: 'Skip for now' })}
        </Label>
      </Button>
    </div>

    {#if exported}
      <p class="ok">
        {$_('onboarding.backup.savedConfirmation', {
          default: "Backup copy saved. You can export it again any time from Settings → Backup & Recovery.",
        })}
      </p>
    {/if}
  {/if}
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
  .center {
    display: flex;
    justify-content: center;
  }
  .actions-row {
    display: flex;
    gap: 12px;
  }
  .actions-row.end {
    justify-content: flex-end;
  }
  .error {
    color: var(--mdc-theme-error, #d32f2f);
    font-weight: 600;
  }
  .ok {
    color: var(--mdc-theme-secondary, #2e7d32);
    font-weight: 500;
  }
</style>
