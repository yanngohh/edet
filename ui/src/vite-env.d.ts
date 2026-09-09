/// <reference types="svelte" />
/// <reference types="vite/client" />

/** Injected by vite.config.ts from package.json. */
declare const __APP_VERSION__: string;

/** Per-instance bindings the dev harness passes through vite's envPrefix. */
interface ImportMetaEnv {
    readonly EDET_NODE?: string;
}
