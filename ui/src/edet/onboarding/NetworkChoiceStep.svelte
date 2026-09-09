<script lang="ts">
  /**
   * Which network this device acts in — asked BEFORE the key exists.
   *
   * That ordering is the whole reason this step is here. Standing is earned
   * on exactly one ledger: founding is irreversible, there is no merge, and
   * two orders becoming one is a founding rather than a migration, with every
   * stake earned again from nothing. So a member who trades first and picks a
   * network afterwards has nothing to carry over. Asked here, before a key
   * has been anywhere, the choice costs nothing.
   *
   * It also replaces the alternative, which is to
   * silently found a private single-validator chain on published dev keys and
   * let the member trade on it — a dead end nobody was told about.
   */
  import { _ } from 'svelte-i18n';
  import List, { Item, Graphic, Text } from '@smui/list';
  import Button, { Label } from '@smui/button';
  import Textfield from '@smui/textfield';

  import { advance, markNetworkChosen } from '../../common/onboardingStore';
  import { lsSet } from '../../common/safeStorage';
  import { customChainId, customNodeUrl, networkId, networkNodes } from '../../lib/node';
  import { NETWORKS } from '../../lib/networks';

  // A named network always has its nodes and its chain id; `custom` has them
  // only once the member has typed both. Nothing to continue to until then:
  // a URL without a chain id is a node, not a network, and the chain id is
  // the one thing this app must never take from the node (`lib/networks.ts`).
  $: ready = $networkNodes.length > 0 && ($networkId !== 'custom' || $customChainId.trim() !== '');

  function onContinue() {
    // Persisted so a later launch skips the step. The selection itself is
    // already in storage (`node.ts` writes it on every change); this records
    // that a MEMBER made it, which the selection alone cannot say — the store
    // persists its own default the moment it is subscribed.
    lsSet('edet-network-chosen', '1');
    markNetworkChosen();
    advance();
  }
</script>

<div class="step">
  <h2 class="step-title">
    {$_('onboarding.network.title', { default: 'Choose your network' })}
  </h2>
  <p class="step-body">
    {$_('onboarding.network.body', {
      default:
        'edet is not one ledger — it is a community that keeps one. Whatever you build here, the standing others put behind you and the record of what you settled, lives on the network you pick now and cannot be moved to another. Pick the one your community runs.',
    })}
  </p>

  <div class="setting-item">
    <span class="setting-label">{$_('onboarding.network.label', { default: 'Network' })}</span>
    <List class="network-list">
      {#each NETWORKS as n (n.id)}
        <Item on:click={() => ($networkId = n.id)} selected={$networkId === n.id}>
          <Graphic class="material-icons">
            {$networkId === n.id ? 'radio_button_checked' : 'radio_button_unchecked'}
          </Graphic>
          <Text>{$_(`settings.connection.net.${n.id}`, { default: n.id })}</Text>
        </Item>
      {/each}
    </List>
  </div>

  {#if $networkId === 'custom'}
    <div class="setting-item">
      <Textfield
        label={$_('settings.connection.nodes', { default: 'Node URLs, separated by spaces' })}
        bind:value={$customNodeUrl}
        style="width: 100%;"
      />
      <Textfield
        label={$_('settings.connection.chain', { default: 'Chain id' })}
        bind:value={$customChainId}
        style="width: 100%;"
      />
      <p class="step-note">
        {$_('settings.connection.chainHelp', {
          default:
            'The chain id is what every signature from this device binds to. Take it from whoever runs the network — it is in the genesis file and on the Network status of every node — never from the node you are about to trust: a node that names the chain chooses which ledger you sign for.',
        })}
      </p>
    </div>
  {/if}

  <p class="step-note">
    {#if $networkNodes.length > 1}
      {$_('onboarding.network.crosscheck', {
        default: `This network publishes ${$networkNodes.length} nodes. Your app asks all of them the same questions and shows you whether their answers agree — so no single node has to be taken at its word.`,
      })}
    {:else}
      {$_('onboarding.network.single', {
        default:
          'This network publishes one node, so there is nothing to check its answers against. That is fine for a node you run yourself, and a reason to be careful with one you do not.',
      })}
    {/if}
  </p>

  <div class="actions">
    <Button variant="raised" on:click={onContinue} disabled={!ready}>
      <Label>{$_('onboarding.network.continue', { default: 'Continue' })}</Label>
    </Button>
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
  .step-note {
    margin: 0;
    font-size: 0.9rem;
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.5;
  }
  .setting-item {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .setting-label {
    font-weight: 600;
    font-size: 1rem;
    color: var(--mdc-theme-on-surface);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    padding-top: 8px;
  }
</style>
