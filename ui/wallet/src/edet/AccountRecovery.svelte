<script lang="ts">
  /**
   * Account & recovery card (Settings): guardian nomination, pending key
   * rotations (veto / finalize), and voluntary exit.
   *
   * A guardian *requests* a rotation from their own device (Community →
   * member detail); here the account owner sees the pending rotation and
   * can veto it during the window — that veto is the anti-theft boundary.
   */
  import { _ } from 'svelte-i18n';
  import Explain from '../components/Explain.svelte';
  import Button, { Label } from '@smui/button';
  import Checkbox from '@smui/checkbox';
  import FormField from '@smui/form-field';

  import NumericInput from '../common/NumericInput.svelte';

  import MemberChip from '../components/MemberChip.svelte';
  import { myMember, networkView } from '../lib/node';
  import { membersList } from '../lib/node';
  import { epochDate } from '../lib/epoch';
  import { currentActorId, clearActor } from '../lib/actors';
  import { tx } from '../lib/api';
  import { send } from '../lib/submit';

  let editing = false;
  let chosen: Record<number, boolean> = {};
  let thresholdStr = '1';
  let vetoWindowStr = '3';

  $: me = $myMember;
  $: epoch = $networkView?.epoch ?? 0;
  $: candidates = $membersList.filter((m) => m.id !== $currentActorId && m.status === 'active');
  $: rotationReady =
    me?.pending_rotation && me.guardian
      ? epoch >= me.pending_rotation.opened_epoch + me.guardian.veto_window_epochs
      : false;

  function beginEdit() {
    chosen = {};
    for (const g of me?.guardian?.guardians ?? []) chosen[g] = true;
    thresholdStr = String(me?.guardian?.threshold ?? 1);
    vetoWindowStr = String(me?.guardian?.veto_window_epochs ?? 3);
    editing = true;
  }

  async function saveGuardians() {
    if ($currentActorId === null) return;
    const guardians = Object.entries(chosen)
      .filter(([, v]) => v)
      .map(([k]) => Number(k));
    const ok = await send(
      tx.registerGuardians({
        member: $currentActorId,
        guardians,
        threshold: Math.max(1, Number(thresholdStr) || 1),
        vetoWindowEpochs: Math.max(1, Number(vetoWindowStr) || 1),
      }),
    );
    if (ok) editing = false;
  }

  async function veto() {
    if ($currentActorId === null) return;
    await send(tx.rotateVeto($currentActorId));
  }

  async function finalize() {
    if ($currentActorId === null) return;
    await send(tx.rotateFinalize($currentActorId));
  }

  async function exitMembership() {
    if ($currentActorId === null) return;
    if (await send(tx.exit($currentActorId))) {
      clearActor();
    }
  }
</script>

<div class="setting-item flex-column">
  <span class="setting-label">{$_('settings.recovery.title', { default: 'Account & recovery' })}</span>
  <div class="setting-description">
      <Explain
        tone="plain"
        summary={$_('settings.recovery.descriptionSummary', {
          default: 'Guardians can jointly move your membership to a new key if this device is lost.',
        })}
      >
        {$_('settings.recovery.description', {
          default:
            'They are trusted members you nominate in advance. A rotation waits out a veto window during which your current key can block a theft.',
        })}
      </Explain>
  </div>

  {#if me?.guardian && !editing}
    <div class="recovery-box">
      <span class="recovery-line">
        {$_('settings.recovery.current', {
          values: { threshold: me.guardian.threshold, window: me.guardian.veto_window_epochs },
          default: `Any ${me.guardian.threshold} of these guardians can request a rotation; the veto window is ${me.guardian.veto_window_epochs} epochs.`,
        })}
      </span>
      <div class="guardian-chips">
        {#each me.guardian.guardians as g}
          <MemberChip memberId={g} />
        {/each}
      </div>
    </div>
  {/if}

  {#if me?.pending_rotation}
    <div class="rotation-warning" role="alert">
      <i class="material-icons" aria-hidden="true">warning</i>
      <div class="flex-column">
        <strong>
          {$_('settings.recovery.pendingTitle', { default: 'A key rotation is pending for your account' })}
        </strong>
        <span>
          {$_('settings.recovery.pendingBody', {
            values: { date: $epochDate(me.pending_rotation.opened_epoch) },
            default: `Opened ${$epochDate(me.pending_rotation.opened_epoch)}. If you did not ask your guardians for this, veto it now.`,
          })}
        </span>
        <!-- A veto deletes the request, so a rotation that is shown is one
             that can still take effect; there is no vetoed state to render. -->
        <div class="backup-actions" style="margin-top: 8px;">
          <Button variant="raised" on:click={veto}>
            <Label>{$_('settings.recovery.veto', { default: 'Veto this rotation' })}</Label>
          </Button>
          {#if rotationReady}
            <Button variant="outlined" on:click={finalize}>
              <Label>{$_('settings.recovery.finalize', { default: 'Finalize rotation' })}</Label>
            </Button>
          {/if}
        </div>
      </div>
    </div>
  {/if}

  {#if editing}
    <div class="recovery-box">
      <span class="recovery-line">{$_('settings.recovery.pick', { default: 'Pick your guardians:' })}</span>
      <div class="guardian-chips">
        {#each candidates as m (m.id)}
          <FormField>
            <Checkbox bind:checked={chosen[m.id]} />
            <span slot="label"><MemberChip memberId={m.id} /></span>
          </FormField>
        {/each}
      </div>
      <div class="guardian-numbers">
        <NumericInput
          integer
          min={1}
          bind:value={thresholdStr}
          label={$_('settings.recovery.threshold', { default: 'Required approvals' })}
        />
        <NumericInput
          integer
          min={1}
          bind:value={vetoWindowStr}
          label={$_('settings.recovery.vetoWindow', { default: 'Veto window (epochs)' })}
        />
      </div>
      <div class="backup-actions">
        <Button variant="raised" on:click={saveGuardians}>
          <Label>{$_('settings.recovery.save', { default: 'Save guardians' })}</Label>
        </Button>
        <Button variant="outlined" on:click={() => (editing = false)}>
          <Label>{$_('common.cancel', { default: 'Cancel' })}</Label>
        </Button>
      </div>
    </div>
  {:else}
    <div class="backup-actions">
      <Button variant="outlined" on:click={beginEdit} disabled={$currentActorId === null}>
        <Label>
          {me?.guardian
            ? $_('settings.recovery.edit', { default: 'Change guardians' })
            : $_('settings.recovery.setup', { default: 'Nominate guardians' })}
        </Label>
      </Button>
    </div>
  {/if}

  <div class="exit-box" role="note">
    <div class="key-disclosure-header">
      <i class="material-icons" aria-hidden="true">logout</i>
      <strong>{$_('settings.recovery.exitTitle', { default: 'Leave the community' })}</strong>
    </div>
    <div class="key-disclosure-item">
        <Explain
          tone="plain"
          summary={$_('settings.recovery.exitSummary', {
            default: 'Voluntary exit is only valid once you owe nothing and hold nothing outstanding.',
          })}
        >
          {$_('settings.recovery.exitBody', {
            default:
              'You must carry no open default, no operation bond of yours still set aside, you must have withdrawn any supply you underwrite, and the community must still have enough validators without you. Your history stays on the ledger. A key that has exited cannot rejoin as itself; an account retired for standing empty can.',
          })}
        </Explain>
    </div>
    <div class="backup-actions">
      <Button variant="outlined" on:click={exitMembership} disabled={$currentActorId === null}>
        <Label>{$_('settings.recovery.exit', { default: 'Exit membership' })}</Label>
      </Button>
    </div>
  </div>
</div>

<style>
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
  .recovery-box {
    border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
    border-radius: 8px;
    padding: 12px 16px;
    margin-bottom: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  :global(.dark-theme) .recovery-box {
    border-color: rgba(255, 255, 255, 0.15);
  }
  .recovery-line {
    font-size: 0.9rem;
    color: var(--mdc-theme-text-secondary-on-surface);
  }
  .guardian-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 16px;
  }
  .guardian-numbers {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  .rotation-warning {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    border: 1px solid rgba(211, 47, 47, 0.35);
    background: rgba(211, 47, 47, 0.05);
    border-radius: 8px;
    padding: 12px 16px;
    margin-bottom: 12px;
  }
  .rotation-warning i {
    color: var(--mdc-theme-error, #d32f2f);
  }
  .backup-actions {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }
  .exit-box {
    margin-top: 16px;
    padding: 12px 16px;
    border-radius: 8px;
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
  }
  :global(.dark-theme) .exit-box {
    border-color: rgba(255, 255, 255, 0.12);
  }
  .key-disclosure-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--mdc-theme-on-surface);
  }
  .key-disclosure-item {
    margin: 10px 0;
    font-size: 0.9rem;
    line-height: 1.5;
    color: var(--mdc-theme-text-secondary-on-surface);
  }
  .flex-column {
    display: flex;
    flex-direction: column;
  }
</style>
