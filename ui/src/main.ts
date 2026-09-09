import App from "./App.svelte";
import { init, register, getLocaleFromNavigator } from "svelte-i18n";

import { get } from "svelte/store";
import { localizationSettings } from "./common/localizationSettings";

const defaultLocale = "en";
register("en", () => import("./locales/en.json"));
register("es", () => import("./locales/es.json"));
register("fr", () => import("./locales/fr.json"));
register("it", () => import("./locales/it.json"));
register("de", () => import("./locales/de.json"));
register("zh", () => import("./locales/zh.json"));

init({
  fallbackLocale: defaultLocale,
  initialLocale: get(localizationSettings).locale || getLocaleFromNavigator(),
});

const app = new App({
  target: document.getElementById("app") ?? document.body,
});

export default app;
