<script lang="ts">
  /**
   * Backup & security card (Settings): vault status, passphrase-encrypted
   * backup export/import, and the key-loss disclosure.
   */
  import { _ } from 'svelte-i18n';
  import Explain from '../components/Explain.svelte';
  import Button, { Label } from '@smui/button';
  import Textfield from '@smui/textfield';

  import {
    custodyDowngrade,
    custodyDowngradeAcknowledged,
    custodyUnreadable,
    deviceKeyBackend,
    isLocked,
    lockState,
    setPassphrase,
    vaultMeta,
  } from '../common/vault';
  import {
    encodeBackup,
    decodeBackup,
    suggestedBackupFileName,
    BackupDecryptError,
    BackupFormatError,
    type BackupPayload,
  } from '../common/backup';
  import {
    chooseActor,
    currentActorId,
    keyring,
    nicknames,
    rememberSeed,
    setNickname,
    setPendingSeed,
  } from '../lib/actors';
  import { copyText } from '../lib/display';
  import { formatDateTime } from '../common/functions';
  import { errorStore } from '../common/errorStore';

  /** The passphrase controls. Empty in both fields means "remove it". */
  let passOne = '';
  let passTwo = '';
  let passBusy = false;
  let hasPassphrase = false;
  void isLocked().then((v) => (hasPassphrase = v));

  /**
   * Set, change or remove the vault passphrase.
   *
   * The seed ring is handed in and re-sealed under the new key in one call —
   * writing the lock record without re-sealing would lock the member out of
   * seeds the old key still encrypts.
   */
  async function applyPassphrase(): Promise<void> {
    if (passBusy) return;
    if (passOne !== passTwo) {
      errorStore.pushError($_('settings.backup.vaultPassMismatch', { default: 'The two passphrases do not match.' }));
      return;
    }
    passBusy = true;
    try {
      await setPassphrase($keyring, passOne === '' ? null : passOne);
      hasPassphrase = passOne !== '';
      passOne = '';
      passTwo = '';
    } catch (e) {
      errorStore.pushError(e instanceof Error ? e.message : String(e));
    } finally {
      passBusy = false;
    }
  }

  let exportPass = '';
  let exportPass2 = '';
  let exporting = false;
  let importText = '';
  let importPass = '';
  let importing = false;
  let fileInput: HTMLInputElement;

  // The reserved 'pending' slot is an identity-in-waiting, not a member.
  $: identityCount = Object.keys($keyring).filter((k) => k !== 'pending').length;

  function payload(): BackupPayload {
    return {
      schema_version: 2,
      seeds: $keyring,
      nicknames: $nicknames,
      actor: $currentActorId,
    };
  }

  function validExportPass(): boolean {
    if (exportPass.length < 8) {
      errorStore.pushError($_('settings.backup.passTooShort', { default: 'Use a passphrase of at least 8 characters.' }));
      return false;
    }
    if (exportPass !== exportPass2) {
      errorStore.pushError($_('settings.backup.passMismatch', { default: 'The passphrases do not match.' }));
      return false;
    }
    return true;
  }

  async function doExport(mode: 'download' | 'copy') {
    if (!validExportPass()) return;
    exporting = true;
    // Let the button state paint before the (CPU-bound) scrypt run.
    await new Promise((r) => setTimeout(r, 30));
    try {
      const text = encodeBackup(payload(), exportPass);
      if (mode === 'download') {
        const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
        const a = document.createElement('a');
        a.href = url;
        a.download = suggestedBackupFileName();
        document.body.appendChild(a);
        a.click();
        a.remove();
        setTimeout(() => URL.revokeObjectURL(url), 5000);
        errorStore.pushError($_('settings.backup.exported', { default: 'Backup file exported.' }), 'warning');
      } else if (await copyText(text)) {
        errorStore.pushError($_('common.copied', { default: 'Copied to clipboard' }), 'warning');
      }
      exportPass = '';
      exportPass2 = '';
    } finally {
      exporting = false;
    }
  }

  function onPickFile() {
    const f = fileInput?.files?.[0];
    if (!f) return;
    void f.text().then((t) => (importText = t));
  }

  async function doImport() {
    if (!importText.trim()) {
      errorStore.pushError($_('settings.backup.noFile', { default: 'Choose a backup file or paste its contents first.' }));
      return;
    }
    importing = true;
    await new Promise((r) => setTimeout(r, 30));
    try {
      const p = decodeBackup(importText, importPass);
      let imported = 0;
      for (const [id, seed] of Object.entries(p.seeds)) {
        if (id === 'pending') {
          setPendingSeed(seed);
        } else {
          rememberSeed(Number(id), seed);
        }
        imported++;
      }
      for (const [id, name] of Object.entries(p.nicknames ?? {})) {
        // Local names on this device win over imported ones.
        if (!$nicknames[Number(id)]) setNickname(Number(id), name);
      }
      if ($currentActorId === null && p.actor !== null && p.actor !== undefined) {
        chooseActor(p.actor);
      }
      importText = '';
      importPass = '';
      if (fileInput) fileInput.value = '';
      errorStore.pushError(
        $_('settings.backup.imported', {
          values: { count: imported },
          default: `Backup restored — ${imported} identities imported into the vault.`,
        }),
        'warning',
      );
    } catch (e) {
      if (e instanceof BackupDecryptError) {
        errorStore.pushError($_('settings.backup.wrongPass', { default: 'Wrong passphrase or corrupted backup.' }));
      } else if (e instanceof BackupFormatError) {
        errorStore.pushError($_('settings.backup.badFile', { default: 'This is not a valid edet backup file.' }));
      } else {
        errorStore.pushError(String(e));
      }
    } finally {
      importing = false;
    }
  }
</script>

<div class="setting-item flex-column">
  <span class="setting-label">{$_('settings.backup.title', { default: 'Backup & security' })}</span>

  <div class="vault-status">
    <i class="material-icons" aria-hidden="true">enhanced_encryption</i>
    <div class="flex-column">
      <span>
        {$_('settings.backup.vaultStatus', {
          values: { count: identityCount },
          default: `Identity vault: ${identityCount} seeds, encrypted at rest (XChaCha20-Poly1305, device-key wrapped).`,
        })}
      </span>
      {#if $deviceKeyBackend === 'keychain'}
        <span class="vault-sub">
          {$_('settings.backup.backendKeychain', {
            default: 'Device key held in the OS keychain — browser-storage dumps alone cannot open the vault.',
          })}
        </span>
      {:else if $deviceKeyBackend === 'keystore'}
        <span class="vault-sub">
          {$_('settings.backup.backendKeystore', {
            default: 'Device key wrapped by the hardware-backed Android Keystore — app-storage dumps alone cannot open the vault.',
          })}
        </span>
      {:else if $deviceKeyBackend === 'local'}
        <span class="vault-sub">
          {$_('settings.backup.backendLocal', {
            default: 'Device key in local app storage (wrapping only) — the desktop app uses the OS keychain; on Android the per-app sandbox isolates this storage.',
          })}
        </span>
      {/if}
      {#if $lockState !== 'none'}
        <span class="vault-sub">
          {$_('settings.backup.passphraseOn', {
            default: 'A passphrase is set: this device asks for it before it will open the vault or sign anything.',
          })}
        </span>
      {/if}
      {#if $vaultMeta && $vaultMeta.updated_at_ms > 0}
        <span class="vault-sub">
          {$_('settings.backup.lastSaved', {
            values: { time: formatDateTime($vaultMeta.updated_at_ms) },
            default: `Last saved: ${formatDateTime($vaultMeta.updated_at_ms)}`,
          })}
        </span>
      {/if}
    </div>
  </div>

  {#if $custodyUnreadable}
    <div class="custody-warning" role="alert">
      <i class="material-icons" aria-hidden="true">phonelink_lock</i>
      <div class="flex-column">
        <span>
          {$_('settings.backup.unreadable', {
            default:
              'This device cannot open the key it holds — it was restored or reset. Restore your identity from your recovery phrase.',
          })}
        </span>
        <span class="vault-sub">
          {$_('settings.backup.unreadableAdvice', {
            default:
              'Nothing is wrong with this phone’s key store. The key that protected your seeds stayed on the old device, and the copy that came across cannot be opened by anything. Your recovery phrase is the way back.',
          })}
        </span>
      </div>
    </div>
  {/if}

  {#if $custodyDowngrade !== null && !$custodyDowngradeAcknowledged}
    <div class="custody-warning" role="alert">
      <i class="material-icons" aria-hidden="true">warning</i>
      <div class="flex-column">
        <span>
          {$_('settings.backup.downgrade', {
            default:
              'This device could not reach its secure key store, so the key that encrypts your seeds is sitting in this app’s own storage beside them. Anything that can read the app’s data can open the vault.',
          })}
        </span>
        <span class="vault-sub">
          {$_('settings.backup.downgradeAdvice', {
            default:
              'Signing is blocked until you acknowledge this. A passphrase below restores a second factor; exporting a backup and restoring on a healthy device removes the problem.',
          })}
        </span>
        <Button variant="raised" on:click={() => custodyDowngradeAcknowledged.set(true)}>
          <Label>{$_('settings.backup.downgradeAck', { default: 'I understand — sign anyway' })}</Label>
        </Button>
      </div>
    </div>
  {/if}

  <div class="flex-column" style="gap: 8px;">
    <span class="setting-label">
      {hasPassphrase
        ? $_('settings.backup.passphraseChange', { default: 'Change or remove the vault passphrase' })
        : $_('settings.backup.passphraseSet', { default: 'Set a vault passphrase' })}
    </span>
    <div class="setting-description">
      <Explain
        tone="plain"
        summary={$_('settings.backup.passphraseSummary', { default: 'A second factor beside this device.' })}
      >
        {$_('settings.backup.passphraseHelp', {
          default:
            'Your seeds cannot be read without it, by anyone, this app included. It is never stored and cannot be reset — your recovery phrase is the way back. Leave both fields empty to remove one.',
        })}
      </Explain>
    </div>
    <Textfield bind:value={passOne} type="password" label={$_('settings.backup.vaultPassphrase', { default: 'Vault passphrase' })} input$autocomplete="new-password" style="width: 100%" />
    <Textfield bind:value={passTwo} type="password" label={$_('settings.backup.vaultPassphraseAgain', { default: 'Repeat it' })} input$autocomplete="new-password" style="width: 100%" />
    <Button variant="raised" disabled={passBusy} on:click={applyPassphrase}>
      <Label>
        {passOne === ''
          ? $_('settings.backup.passphraseRemove', { default: 'Remove the passphrase' })
          : $_('settings.backup.passphraseApply', { default: 'Apply' })}
      </Label>
    </Button>
  </div>

  <div class="setting-description">
      <Explain
        tone="plain"
        summary={$_('settings.backup.descriptionSummary', {
          default: 'A backup file carries the identities held on this device, encrypted with a passphrase you choose.',
        })}
      >
        {$_('settings.backup.description', {
          default:
            'It also carries your local names, and nothing else — the ledger itself lives on your community\'s nodes, and your recovery phrase is never included. Identities created from a phrase are restorable from the phrase alone; the backup matters for seeds that have no phrase, like members you admitted or recovered here.',
        })}
      </Explain>
  </div>

  <span class="subsetting-label">{$_('settings.backup.exportTitle', { default: 'Export' })}</span>
  <div class="row">
    <Textfield
      label={$_('settings.backup.passphrase', { default: 'Backup passphrase (min 8 chars)' })}
      type="password"
      bind:value={exportPass}
    />
    <Textfield
      label={$_('settings.backup.passphrase2', { default: 'Repeat passphrase' })}
      type="password"
      bind:value={exportPass2}
    />
  </div>
  <div class="row">
    <Button variant="raised" disabled={exporting} on:click={() => doExport('download')}>
      <Label>{$_('settings.backup.exportNow', { default: 'Export backup file' })}</Label>
    </Button>
    <Button variant="outlined" disabled={exporting} on:click={() => doExport('copy')}>
      <Label>{$_('settings.backup.exportCopy', { default: 'Copy backup to clipboard' })}</Label>
    </Button>
  </div>

  <span class="subsetting-label" style="margin-top: 16px;">{$_('settings.backup.importTitle', { default: 'Import' })}</span>
  <div class="row">
    <input
      bind:this={fileInput}
      type="file"
      accept=".edet,application/json,text/plain"
      on:change={onPickFile}
      aria-label={$_('settings.backup.chooseFile', { default: 'Choose backup file' })}
    />
  </div>
  <Textfield
    textarea
    label={$_('settings.backup.pasteLabel', { default: '…or paste the backup contents' })}
    bind:value={importText}
    style="width: 100%;"
  />
  <div class="row">
    <Textfield
      label={$_('settings.backup.importPass', { default: 'Passphrase' })}
      type="password"
      bind:value={importPass}
    />
    <Button variant="raised" disabled={importing} on:click={doImport}>
      <Label>{$_('settings.backup.importNow', { default: 'Restore backup' })}</Label>
    </Button>
  </div>

  <!-- Key-loss / key-compromise disclosure: the in-app home of the
       protocol's key-custody boundary, kept next to the backup tools. -->
  <div class="key-disclosure" role="note">
    <div class="key-disclosure-header">
      <i class="material-icons" aria-hidden="true">gpp_bad</i>
      <strong>{$_('settings.backup.keyLossTitle', { default: 'If your key is lost or stolen' })}</strong>
    </div>
    <div class="key-disclosure-item">
        <Explain
          tone="plain"
          summary={$_('settings.backup.keyLossSummary', {
            default: 'Lost: your recovery phrase re-derives your key on any device.',
          })}
        >
          {$_('settings.backup.keyLossBody', {
            default:
              'That is the primary path back. Without the phrase (and without a backup of this vault), the key is gone; if you nominated guardians in advance, a threshold of them can rotate your membership to a new key after the veto window, keeping your standing and history. Without guardians, the only way back is to start again with a new key: your old standing stays with the lost key, and its open debts will default at maturity, falling on whoever was backing them.',
          })}
        </Explain>
    </div>
    <div class="key-disclosure-item">
        <Explain
          tone="plain"
          summary={$_('settings.backup.keyTheftSummary', {
            default: 'Stolen: there is no freeze — whoever holds the key or phrase IS this identity.',
          })}
        >
          {$_('settings.backup.keyTheftBody', {
            default:
              'Act immediately: have your guardians request a rotation to a new key of yours (your old key would veto a thief\'s rotation — a thief\'s key can\'t stop your guardians), and warn your trading partners. Treat the phrase and backup files as equivalent to the key: guard all of them.',
          })}
        </Explain>
    </div>
  </div>
</div>

<style>
  .custody-warning {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 12px;
    border-radius: 8px;
    border: 1px solid var(--mdc-theme-error, #d32f2f);
    color: var(--mdc-theme-error, #d32f2f);
  }

  .setting-item {
    margin-bottom: 32px;
  }
  .setting-label {
    font-weight: 600;
    margin-bottom: 8px;
    color: var(--mdc-theme-on-surface);
    font-size: 1.1rem;
  }
  .setting-description {
    margin: 4px 0 12px 0;
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.5;
    font-size: 0.95rem;
  }
  .subsetting-label {
    font-weight: 500;
    margin-bottom: 4px;
    color: var(--mdc-theme-text-secondary-on-surface);
    font-size: 0.9rem;
    display: block;
  }
  .vault-status {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 14px;
    border-radius: 8px;
    border: 1px solid rgba(46, 125, 50, 0.4);
    background: rgba(46, 125, 50, 0.06);
    margin-bottom: 10px;
    font-size: 0.9rem;
    line-height: 1.45;
  }
  :global(.dark-theme) .vault-status {
    background: rgba(46, 125, 50, 0.12);
    border-color: rgba(46, 125, 50, 0.3);
  }
  .vault-status i {
    color: #2e7d32;
  }
  :global(.dark-theme) .vault-status i {
    color: #a5d6a7;
  }
  .vault-sub {
    color: var(--mdc-theme-text-secondary-on-surface, #666);
    font-size: 0.82rem;
  }
  .row {
    display: flex;
    gap: 12px;
    align-items: flex-end;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }
  input[type='file'] {
    font: inherit;
    color: var(--mdc-theme-on-surface);
  }
  .key-disclosure {
    margin-top: 16px;
    padding: 12px 16px;
    border-radius: 8px;
    border: 1px solid rgba(211, 47, 47, 0.35);
    background: rgba(211, 47, 47, 0.05);
  }
  :global(.dark-theme) .key-disclosure {
    border-color: rgba(244, 67, 54, 0.4);
    background: rgba(244, 67, 54, 0.08);
  }
  .key-disclosure-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--mdc-theme-on-surface);
  }
  .key-disclosure-header i {
    color: var(--mdc-theme-error, #d32f2f);
    font-size: 22px;
  }
  .key-disclosure-item {
    margin: 10px 0 0 0;
    font-size: 0.9rem;
    line-height: 1.5;
    color: var(--mdc-theme-text-secondary-on-surface);
  }
  .flex-column {
    display: flex;
    flex-direction: column;
  }
</style>
