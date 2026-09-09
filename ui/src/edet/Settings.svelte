<script lang="ts">
  import { _ } from 'svelte-i18n';
  import Explain from '../components/Explain.svelte';
  import List, { Item, Graphic, Text } from '@smui/list';
  import Button, { Label } from '@smui/button';
  import Textfield from '@smui/textfield';

  import { localizationSettings, updateSetting, TIMEZONE_OPTIONS, LOCALE_OPTIONS } from '../common/localizationSettings';
  import { formatDateTime, formatNumber } from '../common/functions';
  import { lsGet, lsSet, lsRemove } from '../common/safeStorage';
  import { replayTour } from '../common/onboardingStore';
  import { currentActorId, nicknames, setNickname } from '../lib/actors';
  import { addressOf, copyText } from '../lib/display';
  import { activeBase, customChainId, customNodeUrl, networkId, networkNodes, nodeUrlLocked } from '../lib/node';
  import { NETWORKS } from '../lib/networks';
  import { errorStore } from '../common/errorStore';
  import MemberChip from '../components/MemberChip.svelte';
  import AccountRecovery from './AccountRecovery.svelte';
  import BackupSecurity from './BackupSecurity.svelte';

  let theme: 'light' | 'dark' | 'system' = (lsGet('edet-theme') as any) || 'system';

  function updateTheme(newTheme: string) {
    theme = newTheme as any;
    lsSet('edet-theme', theme);
    const darkState = theme === 'dark' || (theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
    const themeLink = document.getElementById('smui-theme') as HTMLLinkElement;
    if (themeLink) {
      themeLink.href = darkState ? '/smui-dark.css' : '/smui.css';
    }
    if (darkState) {
        document.documentElement.classList.add('dark-theme');
    } else {
        document.documentElement.classList.remove('dark-theme');
    }
  }

  $: theme, updateTheme(theme);

  // Preview values
  const sampleTimestamp = Date.now();
  const sampleNumber = 1234.56;

  $: dateTimePreview = $localizationSettings && formatDateTime(sampleTimestamp);
  $: numberPreview = $localizationSettings && formatNumber(sampleNumber, 2);

  // Identity
  let nicknameDraft = '';
  $: if ($currentActorId !== null && nicknameDraft === '') {
    nicknameDraft = $nicknames[$currentActorId] ?? '';
  }

  function saveNickname() {
    if ($currentActorId === null) return;
    setNickname($currentActorId, nicknameDraft);
  }

  async function copyMyAddress() {
    const addr = $currentActorId === null ? null : $addressOf($currentActorId);
    if (addr && (await copyText(addr))) {
      errorStore.pushError($_('common.copied', { default: 'Copied to clipboard' }), 'warning');
    }
  }

  function onReplayTour() {
    // Clear the persisted "seen" flag and flip the shared onboarding store
    // to the tour step via the dedicated replay entry point. App.svelte has
    // a reactive `$: if (step === 'tour')` watcher that starts Shepherd with
    // its drawer / section host — the same code path as first-run, so the
    // replay behaves identically. `replayTour()` marks this a replay so
    // that when the tour ends, App.svelte returns straight to the app
    // instead of re-entering the wizard.
    lsRemove('edet-tour-seen');
    replayTour();
  }
</script>

<div class="settings-container flex-column">
  <div class="settings-card card">
    <h2 class="section-title">{$_('settings.title', { default: 'Settings' })}</h2>

    <!-- Identity -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.identity.title', { default: 'Identity' })}</span>
      {#if $currentActorId !== null}
        <div class="identity-row">
          <MemberChip memberId={$currentActorId} size={36} detail={$_('settings.identity.actingAs', { default: 'acting on this device' })} />
        </div>
        {#if $addressOf($currentActorId)}
          <div class="identity-address">
            <code>{$addressOf($currentActorId)}</code>
            <button
              type="button"
              class="copy-btn"
              aria-label={$_('common.copy', { default: 'Copy' })}
              title={$_('common.copy', { default: 'Copy' })}
              on:click={copyMyAddress}
            >
              <i class="material-icons" aria-hidden="true">content_copy</i>
            </button>
          </div>
        {/if}
        <div class="identity-controls">
          <Textfield
            label={$_('settings.identity.nickname', { default: 'Display name (local only)' })}
            bind:value={nicknameDraft}
          />
          <Button variant="outlined" on:click={saveNickname}>
            <Label>{$_('common.save', { default: 'Save' })}</Label>
          </Button>
        </div>
        <p class="setting-description">
          {$_('settings.identity.nicknameNote', {
            default: 'Names never leave this device — on the ledger you are your membership, not a username.',
          })}
        </p>
        <div class="setting-description">
            <Explain
              tone="plain"
              summary={$_('settings.identity.phraseSummary', {
                default: 'Your identity key derives from your recovery phrase.',
              })}
            >
              {$_('settings.identity.phraseNote', {
                default:
                  'The phrase is never stored anywhere — whoever holds it can become you, and without it a lost device cannot be recovered (unless your guardians rotate you to a new key). Keep it written down, offline.',
              })}
            </Explain>
        </div>
      {/if}
    </div>

    <!-- Backup & security -->
    <BackupSecurity />

    <!-- Account & recovery -->
    <AccountRecovery />

    <!-- Connection -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.connection.title', { default: 'Node connection' })}</span>
      {#if nodeUrlLocked}
        <p class="setting-description">
          {$_('settings.connection.locked', {
            default: 'This instance is bound to its node by the launcher — one device, one node, one identity.',
          })}
        </p>
        <code class="node-url">{$activeBase}</code>
      {:else}
        <div class="setting-description">
          <Explain
            tone="plain"
            summary={$_('settings.connection.networkSummary', { default: 'The network this device acts in.' })}
          >
            {$_('settings.connection.network', {
              default:
                'Your standing lives on one ledger, and there is no way to carry it to another — so change this only deliberately.',
            })}
          </Explain>
        </div>
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
        {#if $networkId === 'custom'}
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
          <div class="setting-description">
              <Explain
                tone="plain"
                summary={$_('settings.connection.chainSummary', {
                  default: 'The chain id is what every signature from this device binds to.',
                })}
              >
                <!-- Its own key: `chainHelp` is also the onboarding step's
                     copy, where the whole sentence belongs — a first-run
                     screen is the one place not to fold an explanation. -->
                {$_('settings.connection.chainDetail', {
                  default:
                    'Take it from whoever runs the network — it is in the genesis file and on the Network status of every node — never from the node you are about to trust: a node that names the chain chooses which ledger you sign for.',
                })}
              </Explain>
          </div>
        {/if}
        <p class="setting-description">
          {#if $networkNodes.length > 1}
            {$_('settings.connection.crosscheck', {
              values: { n: $networkNodes.length },
              default: `Reads are checked against ${$networkNodes.length} nodes of this network — Network status shows whether they agree.`,
            })}
          {:else}
            {$_('settings.connection.single', {
              default: 'One node, so there is nothing to check its answers against. Network status will say so.',
            })}
          {/if}
        </p>
      {/if}
    </div>

    <!-- Theme Setting -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.theme', { default: 'Theme' })}</span>

      <List class="theme-list">
        <Item on:click={() => (theme = 'light')} selected={theme === 'light'}>
          <Graphic class="material-icons">{theme === 'light' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.themeLight', { default: 'Light' })}</Text>
        </Item>
        <Item on:click={() => (theme = 'dark')} selected={theme === 'dark'}>
          <Graphic class="material-icons">{theme === 'dark' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.themeDark', { default: 'Dark' })}</Text>
        </Item>
        <Item on:click={() => (theme = 'system')} selected={theme === 'system'}>
          <Graphic class="material-icons">{theme === 'system' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.themeSystem', { default: 'System' })}</Text>
        </Item>
      </List>
    </div>

    <!-- Date & Time Format -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.dateTimeFormat', { default: 'Date & time format' })}</span>

      <div class="subsetting">
        <span class="subsetting-label">{$_('settings.dateFormat', { default: 'Date format' })}</span>
        <List class="format-list">
          <Item on:click={() => updateSetting('dateFormat', 'iso')} selected={$localizationSettings.dateFormat === 'iso'}>
            <Graphic class="material-icons">{$localizationSettings.dateFormat === 'iso' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.dateFormatISO', { default: 'ISO 8601 (YYYY-MM-DD)' })}</Text>
          </Item>
          <Item on:click={() => updateSetting('dateFormat', 'us')} selected={$localizationSettings.dateFormat === 'us'}>
            <Graphic class="material-icons">{$localizationSettings.dateFormat === 'us' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.dateFormatUS', { default: 'US Format (MM/DD/YYYY)' })}</Text>
          </Item>
          <Item on:click={() => updateSetting('dateFormat', 'eu')} selected={$localizationSettings.dateFormat === 'eu'}>
            <Graphic class="material-icons">{$localizationSettings.dateFormat === 'eu' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.dateFormatEU', { default: 'European Format (DD/MM/YYYY)' })}</Text>
          </Item>
        </List>
      </div>

      <div class="subsetting">
        <span class="subsetting-label">{$_('settings.timeFormat', { default: 'Time format' })}</span>
        <List class="format-list">
          <Item on:click={() => updateSetting('timeFormat', '24h')} selected={$localizationSettings.timeFormat === '24h'}>
            <Graphic class="material-icons">{$localizationSettings.timeFormat === '24h' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.timeFormat24h', { default: '24-hour' })}</Text>
          </Item>
          <Item on:click={() => updateSetting('timeFormat', '12h')} selected={$localizationSettings.timeFormat === '12h'}>
            <Graphic class="material-icons">{$localizationSettings.timeFormat === '12h' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.timeFormat12h', { default: '12-hour' })}</Text>
          </Item>
        </List>
      </div>

      <div class="preview">
        <span class="preview-label">{$_('settings.preview', { default: 'Preview' })}:</span>
        <span class="preview-value">{dateTimePreview}</span>
      </div>
    </div>

    <!-- Timezone -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.timezone', { default: 'Timezone' })}</span>
      <select bind:value={$localizationSettings.timezone} class="timezone-select">
        {#each TIMEZONE_OPTIONS as tz}
          <option value={tz.value} disabled={tz.disabled}>{tz.label}</option>
        {/each}
      </select>
    </div>

    <!-- Number Format -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.numberFormat', { default: 'Number format' })}</span>

      <List class="format-list">
        <Item on:click={() => updateSetting('numberFormat', 'dot-comma')} selected={$localizationSettings.numberFormat === 'dot-comma'}>
          <Graphic class="material-icons">{$localizationSettings.numberFormat === 'dot-comma' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.numberFormatDotComma', { default: 'Dot decimal, comma thousands' })}</Text>
        </Item>
        <Item on:click={() => updateSetting('numberFormat', 'comma-dot')} selected={$localizationSettings.numberFormat === 'comma-dot'}>
          <Graphic class="material-icons">{$localizationSettings.numberFormat === 'comma-dot' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.numberFormatCommaDot', { default: 'Comma decimal, dot thousands' })}</Text>
        </Item>
      </List>

      <div class="preview">
        <span class="preview-label">{$_('settings.preview', { default: 'Preview' })}:</span>
        <span class="preview-value">{numberPreview}</span>
      </div>
    </div>

    <!-- Language -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.language', { default: 'Language' })}</span>
      <select bind:value={$localizationSettings.locale} class="timezone-select">
        {#each LOCALE_OPTIONS as lang}
          <option value={lang.value}>{lang.label}</option>
        {/each}
      </select>
    </div>

    <!-- Tour -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.tour.title', { default: 'Tour' })}</span>
      <p class="setting-description">
        {$_('settings.tour.description', {
          default: "Replay the introduction tour if you'd like a refresher.",
        })}
      </p>
      <div class="backup-actions">
        <Button variant="outlined" on:click={onReplayTour}>
          <Label>{$_('settings.tour.replay', { default: 'Replay tour' })}</Label>
        </Button>
      </div>
    </div>

    <!-- About -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.about.title', { default: 'About' })}</span>
      <!-- svelte-ignore missing-declaration -->
      <p class="setting-description">
        edet {__APP_VERSION__} — {$_('settings.about.body', {
          default: 'a feeless mutual-credit ledger run by your community.',
        })}
      </p>
    </div>
  </div>
</div>

<style>
  .settings-container {
    width: 100%;
    padding: 16px;
    box-sizing: border-box;
    align-items: center;
      max-width: var(--edet-column);
  }

  .settings-card {
    width: 100%;
    max-width: 600px;
    background: var(--mdc-theme-surface, #fff);
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    border-radius: 8px;
    padding: 24px;
    box-shadow: 0 2px 4px rgba(0,0,0,0.05);
    text-align: left;
  }

  :global(.dark-theme) .settings-card {
    background: #1e1e1e;
    border-color: rgba(255, 255, 255, 0.1);
  }

  .section-title {
    margin: 0 0 24px 0;
    font-size: 1.5rem;
    color: var(--mdc-theme-primary);
  }

  .setting-item {
    margin-bottom: 32px;
  }

  .setting-item:last-child {
    margin-bottom: 0;
  }

  .setting-label {
    font-weight: 600;
    margin-bottom: 8px;
    color: var(--mdc-theme-on-surface);
    font-size: 1.1rem;
  }

  .subsetting {
    margin-top: 16px;
  }

  .subsetting-label {
    font-weight: 500;
    margin-bottom: 4px;
    color: var(--mdc-theme-text-secondary-on-surface);
    font-size: 0.9rem;
    display: block;
  }

  :global(.theme-list),
  :global(.format-list) {
      margin-top: 8px;
      border: 1px solid var(--mdc-theme-text-hint-on-background, #f0f0f0);
      border-radius: 4px;
  }

  :global(.dark-theme) :global(.theme-list),
  :global(.dark-theme) :global(.format-list) {
      border-color: rgba(255, 255, 255, 0.1);
  }

  .timezone-select {
    width: 100%;
    margin-top: 8px;
    padding: 12px;
    border: 1px solid var(--mdc-theme-text-hint-on-background, #e0e0e0);
    border-radius: 4px;
    background: var(--mdc-theme-surface, #fff);
    color: var(--mdc-theme-on-surface);
    font-size: 1rem;
    font-family: inherit;
    cursor: pointer;
  }

  .timezone-select:focus {
    outline: none;
    border-color: var(--mdc-theme-primary);
    box-shadow: 0 0 0 2px rgba(98, 0, 238, 0.1);
  }

  :global(.dark-theme) .timezone-select {
    background: #2a2a2a;
    border-color: rgba(255, 255, 255, 0.2);
    color: #fff;
  }

  .timezone-select option {
    background: var(--mdc-theme-surface, #fff);
    color: var(--mdc-theme-on-surface);
  }

  :global(.dark-theme) .timezone-select option {
    background: #2a2a2a;
    color: #fff;
  }

  .preview {
    margin-top: 12px;
    padding: 8px 12px;
    background: var(--mdc-theme-background, #f5f5f5);
    border-radius: 4px;
    border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
  }

  :global(.dark-theme) .preview {
    background: rgba(255, 255, 255, 0.05);
    border-color: rgba(255, 255, 255, 0.1);
  }

  .preview-label {
    font-size: 0.85rem;
    color: var(--mdc-theme-text-secondary-on-surface);
    margin-right: 8px;
  }

  .preview-value {
    font-family: monospace;
    font-weight: 600;
    color: var(--mdc-theme-primary);
  }

  .setting-description {
    margin: 4px 0 12px 0;
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.5;
    font-size: 0.95rem;


  }

  .backup-actions {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }

  .identity-row {
    margin-bottom: 12px;
  }

  .identity-address {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-bottom: 12px;
  }

  .identity-address code {
    font-family: monospace;
    font-size: 0.8rem;
    color: var(--mdc-theme-text-secondary-on-surface, #666);
    overflow-wrap: anywhere;
  }

  .copy-btn {
    background: none;
    border: none;
    padding: 2px;
    cursor: pointer;
    color: var(--mdc-theme-text-secondary-on-surface, #999);
    display: inline-flex;
    flex-shrink: 0;
  }

  .copy-btn:hover {
    color: var(--mdc-theme-primary);
  }

  .copy-btn i {
    font-size: 16px;
  }

  .identity-controls {
    display: flex;
    gap: 12px;
    align-items: flex-end;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }

  .node-url {
    font-family: monospace;
    font-size: 0.85rem;
    color: var(--mdc-theme-text-secondary-on-surface, #666);
    overflow-wrap: anywhere;
  }
</style>
