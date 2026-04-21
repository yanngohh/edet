<script lang="ts">
  import { _ } from 'svelte-i18n';
  import List, { Item, Graphic, Text } from '@smui/list';
  import { localizationSettings, updateSetting, TIMEZONE_OPTIONS, LOCALE_OPTIONS } from '../common/localizationSettings';
  import { formatDateTime, formatNumber } from '../common/functions';

  let theme: 'light' | 'dark' | 'system' = (localStorage.getItem('edet-theme') as any) || 'system';

  function updateTheme(newTheme: string) {
    theme = newTheme as any;
    localStorage.setItem('edet-theme', theme);
    const darkState = theme === 'dark' || (theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
    const themeLink = document.getElementById('smui-theme') as HTMLLinkElement;
    if (themeLink) {
      themeLink.href = darkState ? 'smui-dark.css' : 'smui.css';
    }
    if (darkState) {
        document.documentElement.classList.add('dark-theme');
    } else {
        document.documentElement.classList.remove('dark-theme');
    }
  }

  $: theme, updateTheme(theme);

  // Preview values
  const sampleTimestamp = Date.now() * 1000; // Current time in microseconds
  const sampleNumber = 1234.56;

  $: dateTimePreview = $localizationSettings && formatDateTime(sampleTimestamp);
  $: numberPreview = $localizationSettings && formatNumber(sampleNumber, 2);

</script>

<div class="settings-container flex-column">
  <div class="settings-card card">
    <h2 class="section-title">{$_('settings.title')}</h2>
    
    <!-- Theme Setting -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.theme')}</span>
      
      <List class="theme-list">
        <Item on:click={() => (theme = 'light')} selected={theme === 'light'}>
          <Graphic class="material-icons">{theme === 'light' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.themeLight')}</Text>
        </Item>
        <Item on:click={() => (theme = 'dark')} selected={theme === 'dark'}>
          <Graphic class="material-icons">{theme === 'dark' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.themeDark')}</Text>
        </Item>
        <Item on:click={() => (theme = 'system')} selected={theme === 'system'}>
          <Graphic class="material-icons">{theme === 'system' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.themeSystem')}</Text>
        </Item>
      </List>
    </div>

    <!-- Date & Time Format -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.dateTimeFormat')}</span>
      
      <div class="subsetting">
        <span class="subsetting-label">{$_('settings.dateFormat')}</span>
        <List class="format-list">
          <Item on:click={() => updateSetting('dateFormat', 'iso')} selected={$localizationSettings.dateFormat === 'iso'}>
            <Graphic class="material-icons">{$localizationSettings.dateFormat === 'iso' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.dateFormatISO')}</Text>
          </Item>
          <Item on:click={() => updateSetting('dateFormat', 'us')} selected={$localizationSettings.dateFormat === 'us'}>
            <Graphic class="material-icons">{$localizationSettings.dateFormat === 'us' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.dateFormatUS')}</Text>
          </Item>
          <Item on:click={() => updateSetting('dateFormat', 'eu')} selected={$localizationSettings.dateFormat === 'eu'}>
            <Graphic class="material-icons">{$localizationSettings.dateFormat === 'eu' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.dateFormatEU')}</Text>
          </Item>
        </List>
      </div>

      <div class="subsetting">
        <span class="subsetting-label">{$_('settings.timeFormat')}</span>
        <List class="format-list">
          <Item on:click={() => updateSetting('timeFormat', '24h')} selected={$localizationSettings.timeFormat === '24h'}>
            <Graphic class="material-icons">{$localizationSettings.timeFormat === '24h' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.timeFormat24h')}</Text>
          </Item>
          <Item on:click={() => updateSetting('timeFormat', '12h')} selected={$localizationSettings.timeFormat === '12h'}>
            <Graphic class="material-icons">{$localizationSettings.timeFormat === '12h' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
            <Text>{$_('settings.timeFormat12h')}</Text>
          </Item>
        </List>
      </div>

      <div class="preview">
        <span class="preview-label">{$_('settings.preview')}:</span>
        <span class="preview-value">{dateTimePreview}</span>
      </div>
    </div>

    <!-- Timezone -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.timezone')}</span>
      <select bind:value={$localizationSettings.timezone} class="timezone-select">
        {#each TIMEZONE_OPTIONS as tz}
          <option value={tz.value} disabled={tz.disabled}>{tz.label}</option>
        {/each}
      </select>
    </div>

    <!-- Number Format -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.numberFormat')}</span>
      
      <List class="format-list">
        <Item on:click={() => updateSetting('numberFormat', 'dot-comma')} selected={$localizationSettings.numberFormat === 'dot-comma'}>
          <Graphic class="material-icons">{$localizationSettings.numberFormat === 'dot-comma' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.numberFormatDotComma')}</Text>
        </Item>
        <Item on:click={() => updateSetting('numberFormat', 'comma-dot')} selected={$localizationSettings.numberFormat === 'comma-dot'}>
          <Graphic class="material-icons">{$localizationSettings.numberFormat === 'comma-dot' ? 'radio_button_checked' : 'radio_button_unchecked'}</Graphic>
          <Text>{$_('settings.numberFormatCommaDot')}</Text>
        </Item>
      </List>

      <div class="preview">
        <span class="preview-label">{$_('settings.preview')}:</span>
        <span class="preview-value">{numberPreview}</span>
      </div>
    </div>

    <!-- Language -->
    <div class="setting-item flex-column">
      <span class="setting-label">{$_('settings.language')}</span>
      <select bind:value={$localizationSettings.locale} class="timezone-select">
        {#each LOCALE_OPTIONS as lang}
          <option value={lang.value}>{lang.label}</option>
        {/each}
      </select>
    </div>
  </div>
</div>

<style>
  .settings-container {
    width: 100%;
    padding: 16px;
    box-sizing: border-box;
    align-items: center;
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
</style>
