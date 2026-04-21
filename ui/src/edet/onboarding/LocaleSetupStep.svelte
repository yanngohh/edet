<script lang="ts">
  /**
   * Step 1 of onboarding: let the user pick their language + date/time /
   * number formats before anything else happens.
   *
   * Everything rendered after this step (the recovery phrase, the tour
   * popovers, error banners, etc.) is translated on the fly by svelte-i18n
   * as soon as the user changes the language dropdown, so the rest of the
   * wizard reads naturally in their chosen locale.
   *
   * The live preview mirrors the preview in the Settings page, so what the
   * user sees here is exactly what they'll get in the app.
   */
  import { _ } from 'svelte-i18n';
  import List, { Item, Graphic, Text } from '@smui/list';
  import Button, { Label } from '@smui/button';

  import {
    localizationSettings,
    updateSetting,
    markLocaleConfigured,
    TIMEZONE_OPTIONS,
    LOCALE_OPTIONS,
  } from '../../common/localizationSettings';
  import {
    markLocaleConfigured as markStepDone,
    advance,
  } from '../../common/onboardingStore';
  import { formatDateTime, formatNumber } from '../../common/functions';

  const sampleTimestamp = Date.now() * 1000; // microseconds
  const sampleNumber = 1234.56;

  $: dateTimePreview = $localizationSettings && formatDateTime(sampleTimestamp);
  $: numberPreview = $localizationSettings && formatNumber(sampleNumber, 2);

  function onContinue() {
    // Persist the "the user has explicitly acknowledged these settings"
    // flag so future launches skip the step.
    markLocaleConfigured();
    // Advance the onboarding orchestrator.
    markStepDone();
    advance();
  }
</script>

<div class="step">
  <h2 class="step-title">
    {$_('onboarding.localeSetup.title', { default: 'Choose your language and formats' })}
  </h2>
  <p class="step-body">
    {$_('onboarding.localeSetup.body', {
      default:
        'The rest of this wizard — including your recovery phrase and the tour — will be shown in the language you pick here. You can change these settings later under Settings → Localization.',
    })}
  </p>

  <div class="setting-item">
    <span class="setting-label">{$_('settings.language', { default: 'Language' })}</span>
    <select bind:value={$localizationSettings.locale} class="select">
      {#each LOCALE_OPTIONS as lang}
        <option value={lang.value}>{lang.label}</option>
      {/each}
    </select>
  </div>

  <div class="setting-item">
    <span class="setting-label">{$_('settings.dateFormat', { default: 'Date Format' })}</span>
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

  <div class="setting-item">
    <span class="setting-label">{$_('settings.timeFormat', { default: 'Time Format' })}</span>
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

  <div class="setting-item">
    <span class="setting-label">{$_('settings.timezone', { default: 'Timezone' })}</span>
    <select bind:value={$localizationSettings.timezone} class="select">
      {#each TIMEZONE_OPTIONS as tz}
        <option value={tz.value} disabled={tz.disabled}>{tz.label}</option>
      {/each}
    </select>
  </div>

  <div class="setting-item">
    <span class="setting-label">{$_('settings.numberFormat', { default: 'Number Format' })}</span>
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
  </div>

  <div class="preview">
    <span class="preview-label">{$_('settings.preview', { default: 'Preview' })}:</span>
    <span class="preview-value">{dateTimePreview} · {numberPreview}</span>
  </div>

  <div class="actions-row end">
    <Button variant="raised" on:click={onContinue}>
      <Label>{$_('onboarding.localeSetup.continue', { default: 'Continue' })}</Label>
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
  .select {
    width: 100%;
    padding: 12px;
    border: 1px solid var(--mdc-theme-text-hint-on-background, #e0e0e0);
    border-radius: 4px;
    background: var(--mdc-theme-surface, #fff);
    color: var(--mdc-theme-on-surface);
    font-size: 1rem;
    font-family: inherit;
    cursor: pointer;
  }
  .select:focus {
    outline: none;
    border-color: var(--mdc-theme-primary);
    box-shadow: 0 0 0 2px rgba(98, 0, 238, 0.1);
  }
  :global(.dark-theme) .select {
    background: #2a2a2a;
    border-color: rgba(255, 255, 255, 0.2);
    color: #fff;
  }
  :global(.format-list) {
    border: 1px solid var(--mdc-theme-text-hint-on-background, #f0f0f0);
    border-radius: 4px;
  }
  :global(.dark-theme) :global(.format-list) {
    border-color: rgba(255, 255, 255, 0.1);
  }
  .preview {
    padding: 10px 14px;
    border-radius: 4px;
    background: var(--mdc-theme-background, #f5f5f5);
    border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  :global(.dark-theme) .preview {
    background: rgba(255, 255, 255, 0.04);
    border-color: rgba(255, 255, 255, 0.1);
  }
  .preview-label {
    font-size: 0.85rem;
    color: var(--mdc-theme-text-secondary-on-surface);
  }
  .preview-value {
    font-family: monospace;
    font-weight: 600;
    color: var(--mdc-theme-primary);
  }
  .actions-row {
    display: flex;
    gap: 12px;
  }
  .actions-row.end {
    justify-content: flex-end;
  }
</style>
