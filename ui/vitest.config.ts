import { defineConfig } from 'vitest/config';

// Stand-alone vitest config: the tests focus on pure-TS modules
// (`common/mnemonic.ts`, `common/backup.ts`, `common/backupScheduler.ts`).
// We don't pull in the Svelte plugin here because the unit tests do not
// import any `.svelte` file; keeping the surface minimal makes CI fast.
export default defineConfig({
    test: {
        environment: 'jsdom',
        include: ['src/**/__tests__/**/*.test.ts'],
        // `crypto.getRandomValues` is available in jsdom without polyfills.
        setupFiles: [],
    },
});
