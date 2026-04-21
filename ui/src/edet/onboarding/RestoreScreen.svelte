<script lang="ts">
  /**
   * Recover an existing edet identity on a fresh install.
   *
   * This screen is only reachable through the Tauri shell (the browser has
   * no way to re-seed lair-keystore). It:
   *   1. Lets the user pick their `.edet-backup` file.
   *   2. Collects the 12-word phrase.
   *   3. Verifies the phrase matches the file's fingerprint.
   *   4. Re-derives the keys, imports the agent seed into lair, installs the
   *      happ with that agent key, and grafts the stored source chain.
   *   5. Hands off to the normal App boot flow.
   *
   * In dev mode (no Tauri) we show an explanation rather than pretending to
   * restore anything.
   */
  import { _ } from 'svelte-i18n';
  import { createEventDispatcher } from 'svelte';
  import Button, { Label } from '@smui/button';
  import Textfield from '@smui/textfield';

  import { tauriBridge } from '../../common/tauriBridge';
  import { isValidMnemonic, deriveKeys } from '../../common/mnemonic';
  import {
    parseEnvelope,
    decodeBackup,
    BackupFormatError,
    BackupDecryptError,
    MnemonicMismatchError,
  } from '../../common/backup';
  import { writeCurrentBackup } from '../../common/backupStorage';
  import { wrapAndStoreBackupKey } from '../../common/deviceKey';

  const dispatch = createEventDispatcher<{ restored: void; cancel: void }>();

  let phrase = '';
  let fileBytes: Uint8Array | null = null;
  let fileSelected = false;
  let busy = false;
  let error: string | null = null;
  let forkWarningAcknowledged = false;

  $: isValid = isValidMnemonic(phrase);

  async function pickFile() {
    error = null;
    try {
      const bytes = await tauriBridge.importBackupFile();
      if (!bytes) return;
      try {
        parseEnvelope(bytes);
        fileBytes = bytes;
        fileSelected = true;
      } catch (e) {
        if (e instanceof BackupFormatError) {
          fileBytes = null;
          fileSelected = false;
          error = e.message;
        }
      }
    } catch (e: any) {
      error = e?.message ?? String(e);
    }
  }

  async function restore() {
    if (!fileBytes || !isValid) return;
    busy = true;
    error = null;
    try {
      const derived = await deriveKeys(phrase);
      const payload = decodeBackup(fileBytes, derived.backupKey, derived.fingerprint);

      if (!tauriBridge.isTauri()) {
        throw new Error($_('recovery.needsTauri'));
      }

      const agentPubKey = await tauriBridge.importLairSeed(derived.agentSeed);
      await tauriBridge.installAppWithAgentKey(agentPubKey);
      if (payload.source_chain.length > 0 && payload.dna_hash.length === 39) {
        await tauriBridge.graftSourceChain(
          [payload.dna_hash, agentPubKey],
          payload.source_chain as any,
        );
      }

      await writeCurrentBackup(fileBytes, payload.agent_pub_key);
      await wrapAndStoreBackupKey(derived.backupKey);

      dispatch('restored');
    } catch (e: any) {
      if (e instanceof MnemonicMismatchError) {
        error = $_('recovery.wrongPhrase');
      } else if (e instanceof BackupDecryptError) {
        error = $_('recovery.corrupted');
      } else if (e instanceof BackupFormatError) {
        error = e.message;
      } else {
        error = e?.message ?? String(e);
      }
    } finally {
      busy = false;
    }
  }
</script>

<div class="screen">
  <h1 class="title">{$_('recovery.title')}</h1>

  {#if !tauriBridge.isTauri()}
    <p class="error">{$_('recovery.desktopOnly')}</p>
    <Button variant="outlined" on:click={() => dispatch('cancel')}>
      <Label>{$_('recovery.cancel')}</Label>
    </Button>
  {:else}
    <p class="body">{$_('recovery.body')}</p>

    <Button variant="raised" on:click={pickFile} disabled={busy}>
      <Label>
        {fileSelected
          ? $_('recovery.reselect')
          : $_('recovery.pickFile')}
      </Label>
    </Button>

    <div class="field">
      <Textfield
        bind:value={phrase}
        label={$_('recovery.phrase')}
        textarea
        input$rows={3}
        input$autocomplete="off"
        input$autocapitalize="off"
        input$spellcheck="false"
      />
    </div>

    <div class="fork-warning">
      <strong>{$_('recovery.forkWarningHeader')}</strong>
      <p>{$_('recovery.forkWarningBody')}</p>
      <label class="fork-warning-check">
        <input type="checkbox" bind:checked={forkWarningAcknowledged} />
        <span>{$_('recovery.forkWarningAcknowledge')}</span>
      </label>
    </div>

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    <div class="actions">
      <Button variant="outlined" on:click={() => dispatch('cancel')} disabled={busy}>
        <Label>{$_('recovery.cancel')}</Label>
      </Button>
      <Button
        variant="raised"
        on:click={restore}
        disabled={busy || !fileBytes || !isValid || !forkWarningAcknowledged}
      >
        <Label>
          {#if busy}
            <span class="btn-busy">
              <span class="spinner" aria-hidden="true"></span>
              {$_('recovery.restoring')}
            </span>
          {:else}
            {$_('recovery.restore')}
          {/if}
        </Label>
      </Button>
    </div>
  {/if}
</div>

<style>
  .screen {
    max-width: 560px;
    margin: 48px auto;
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    background: var(--mdc-theme-surface, #fff);
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    border-radius: 12px;
  }
  :global(.dark-theme) .screen {
    background: #1e1e1e;
    border-color: rgba(255, 255, 255, 0.1);
  }
  .title {
    margin: 0;
    color: var(--mdc-theme-primary);
  }
  .body {
    margin: 0;
    line-height: 1.5;
    color: var(--mdc-theme-on-surface);
  }
  .field :global(.mdc-text-field) {
    width: 100%;
  }
  .error {
    margin: 0;
    color: var(--mdc-theme-error, #d32f2f);
    font-weight: 600;
  }
  .actions {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  .fork-warning {
    padding: 12px 16px;
    border-radius: 8px;
    border: 1px solid rgba(251, 192, 45, 0.4);
    background: rgba(251, 192, 45, 0.08);
    color: var(--mdc-theme-on-surface);
  }
  .fork-warning strong {
    color: #fbc02d;
  }
  .fork-warning p {
    margin: 6px 0 10px 0;
    line-height: 1.5;
  }
  .fork-warning-check {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }

  /* Inline spinner that stays inside the button label */
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
</style>
