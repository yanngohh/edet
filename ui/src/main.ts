import App from './App.svelte';
import { init, register, getLocaleFromNavigator } from 'svelte-i18n'

const defaultLocale = 'en';
register('en', () => import('./locales/en.json'));
register('es', () => import('./locales/es.json'));
register('fr', () => import('./locales/fr.json'));
register('it', () => import('./locales/it.json'));
register('de', () => import('./locales/de.json'));
register('zh', () => import('./locales/zh.json'));

import { get } from 'svelte/store';
import { localizationSettings } from './common/localizationSettings';

init({
    fallbackLocale: defaultLocale,
    initialLocale: get(localizationSettings).locale || getLocaleFromNavigator(),
})

// Mount to the #app div defined in index.html rather than document.body directly,
// keeping the DOM structure clean and consistent with the HTML structure.
const app = new App({
  target: document.getElementById('app') ?? document.body,
});

export default app;
