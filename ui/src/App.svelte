<script lang="ts">
    import { _ } from 'svelte-i18n';
    import {onMount, setContext} from 'svelte';
    import { writable } from 'svelte/store';
    import type {AppClient} from '@holochain/client';
    import {AppWebsocket} from '@holochain/client';
    import CircularProgress from '@smui/circular-progress';
    import TopAppBar, { Row, Section, AutoAdjust } from '@smui/top-app-bar';
    import IconButton from '@smui/icon-button';
    import Drawer, { AppContent, Content, Header, Subtitle, Title } from '@smui/drawer';
    import List, {Item, Graphic} from '@smui/list';
    import 'material-design-icons/iconfont/material-icons.css';
    import { isLoading } from 'svelte-i18n';


    import MyWallet from './edet/transaction/MyWallet.svelte';

    import {clientContext} from './contexts';
    import { locale } from 'svelte-i18n';
    import { localizationSettings, hasUserConfiguredLocale } from './common/localizationSettings';
    import ErrorSnackBar from './common/ErrorSnackBar.svelte';
    import { errorStore } from './common/errorStore';
    import {
        connectionState,
        markConnected,
        markLost,
        markReconnecting,
        markRestored,
    } from './common/connectionState';

    import SupportBreakdown from "./edet/support/SupportBreakdown.svelte";
    import MySupporters from "./edet/support/MySupporters.svelte";
    import MyVouchesGiven from "./edet/support/MyVouchesGiven.svelte";
    import MyVouchesReceived from "./edet/support/MyVouchesReceived.svelte";
    import MyTransactions from './edet/transaction/MyTransactions.svelte';
    import MyContracts from './edet/transaction/MyContracts.svelte';
    import Settings from './edet/Settings.svelte';

    // Onboarding overlay + scheduler + storage helpers
    import Onboarding from './edet/onboarding/Onboarding.svelte';
    import {
        beginOnboarding,
        onboardingActive,
        onboardingStep,
        completeOnboarding,
        markTourSeen,
    } from './common/onboardingStore';
    import { hasCurrentBackup, readCurrentBackup, writeCurrentBackup } from './common/backupStorage';
    import { startBackupScheduler } from './common/backupScheduler';
    import { encodeBackup, parseEnvelope } from './common/backup';
    import type { BackupPayload } from './common/backup';
    import { tauriBridge } from './common/tauriBridge';
    import { loadBackupKey } from './common/deviceKey';
    import { lsGet, lsSet } from './common/safeStorage';
    import { runTour } from './edet/tour';


    // clientStore holds the AppClient once connected. Exposing it as a Svelte
    // store (rather than a raw variable) ensures that:
    //   1. Child components always read the *current* client via getClient(),
    //      which closes over the store rather than a snapshot.
    //   2. During Vite HMR the store is reset to undefined, which triggers the
    //      `$clientStore` reactive guard in the template and prevents children
    //      from being instantiated before onMount re-connects.
    const clientStore = writable<AppClient | undefined>(undefined);
    $: client = $clientStore;  // local alias for use in non-template code
    let loading = true;
    let loadingKey = '';
    let fatalError = false;
    let connectionLost = false;
    let reconnecting = false;
    let openDrawer = false;
    let openSection = 0;
    let topAppBar: TopAppBar;

    const MAX_CONNECT_ATTEMPTS = 5;
    const BASE_BACKOFF_MS = 1000;

    $: loading, openDrawer, openSection;

    $: if ($localizationSettings.locale) {
        $locale = $localizationSettings.locale;
        // Keep the <html lang> attribute in sync with the active locale so that
        // screen readers announce content in the correct language.
        if (typeof document !== 'undefined') {
            document.documentElement.lang = $localizationSettings.locale.split('-')[0];
        }
    }

    // Shepherd tour host. The `runTour` function in `./edet/tour.ts` calls
    // these two hooks before each step to open the drawer and switch to the
    // target section — otherwise the popovers would anchor to invisible
    // elements and end up centred on the screen. The same host powers the
    // first-run flow and the Settings "Replay tour" button, which just
    // calls `goToStep('tour')` and falls through to the reactive watcher
    // below.
    const tourHost = {
        setDrawerOpen: (o: boolean) => { openDrawer = o; },
        setSection:    (idx: number) => { openSection = idx; },
    };
    let tourRunning = false;

    async function startTour(): Promise<void> {
        if (tourRunning) return;
        tourRunning = true;
        try {
            await runTour(tourHost);
        } finally {
            tourRunning = false;
            markTourSeen();
            lsSet('edet-tour-seen', '1');
            // Flips step → 'idle' and zeroes the derived keys that were
            // kept alive in memory during the backup-export step.
            completeOnboarding();
        }
    }

    // When the onboarding store lands on `tour`, unmount the overlay (see
    // gate on `<Onboarding />` below) and launch Shepherd against the real
    // app DOM. We own the trigger in App.svelte — not inside
    // Onboarding.svelte — so the tour can target the drawer items and
    // wallet metrics that live in this component's template.
    // Guard on `$clientStore` so the tour never starts before the
    // AppWebsocket is live — the restore path advances to `tour` before
    // connectWithRetry() has run, which would show only a spinner.
    $: if ($onboardingStep === 'tour' && !tourRunning && $clientStore) {
        void startTour();
    }

    /**
     * Attempt to connect with exponential backoff. Returns the client or throws.
     *
     * - Tauri path: calls `get_app_websocket_auth` to get the port + token.
     *   The zome-call-signer.js init script injected by lib.rs sets
     *   window.__HC_ZOME_CALL_SIGNER__ so AppWebsocket uses the plugin's
     *   sign_zome_call command via lair — no custom transform needed.
     * - hc-spin / browser path: calls AppWebsocket.connect() with no args.
     */
    async function connectWithRetry(): Promise<AppClient> {
        let lastError: any;
        for (let attempt = 1; attempt <= MAX_CONNECT_ATTEMPTS; attempt++) {
            try {
                if (tauriBridge.isTauri()) {
                    const { port, token } = await tauriBridge.getAppWebsocketAuth();
                    return await AppWebsocket.connect({
                        url: new URL(`ws://localhost:${port}`),
                        token,
                    });
                }
                return await AppWebsocket.connect();
            } catch (e: any) {
                lastError = e;
                if (attempt < MAX_CONNECT_ATTEMPTS) {
                    await new Promise(r => setTimeout(r, BASE_BACKOFF_MS * Math.pow(2, attempt - 1)));
                }
            }
        }
        throw lastError;
    }

    /** Attach a close-event listener to the underlying WebSocket so we detect drops.
     *
     * @holochain/client v0.20 exposes the underlying WebSocket via the public
     * `on('close', callback)` event emitter method on AppWebsocket.  We use that
     * public API instead of accessing internal `(c as any).client?.socket`
     * which is fragile across SDK version upgrades.
     *
     * Fallback: if `on` is not available (unexpected SDK change), we register
     * a 30-second heartbeat poll that detects a dead connection via a caught error.
     */
    function attachCloseListener(c: AppClient) {
        // Prefer the public EventEmitter-style API (available in AppWebsocket v0.20+)
        if (typeof (c as any).on === 'function') {
            (c as any).on('close', async () => {
                if (connectionLost) return;
                connectionLost = true;
                reconnecting = true;
                markLost();
                markReconnecting();
                errorStore.pushError(
                    $_('app.connectionLost', { default: 'Connection to Holochain lost. Reconnecting…' }),
                    'warning'
                );
                await attemptReconnect();
            });
            return;
        }

        // Fallback: access the underlying WebSocket socket if the emitter API is absent.
        // This handles SDK versions that expose .client.socket rather than .on('close').
        const socket: WebSocket | undefined = (c as any).client?.socket ?? (c as any).socket;
        if (socket) {
            socket.addEventListener('close', async () => {
                if (connectionLost) return;
                connectionLost = true;
                reconnecting = true;
                markLost();
                markReconnecting();
                errorStore.pushError(
                    $_('app.connectionLost', { default: 'Connection to Holochain lost. Reconnecting…' }),
                    'warning'
                );
                await attemptReconnect();
            }, { once: true });
            return;
        }

        // Last-resort fallback: heartbeat poll every 30 seconds
        const heartbeatInterval = setInterval(async () => {
            if (connectionLost) { clearInterval(heartbeatInterval); return; }
            try {
                await c.appInfo();
            } catch {
                clearInterval(heartbeatInterval);
                if (connectionLost) return;
                connectionLost = true;
                reconnecting = true;
                markLost();
                markReconnecting();
                errorStore.pushError(
                    $_('app.connectionLost', { default: 'Connection to Holochain lost. Reconnecting…' }),
                    'warning'
                );
                await attemptReconnect();
            }
        }, 30_000);
    }

    async function attemptReconnect() {
        try {
            const newClient = await connectWithRetry();
            clientStore.set(newClient);
            connectionLost = false;
            reconnecting = false;
            markRestored();
            attachCloseListener(newClient);
            errorStore.pushError(
                $_('app.connectionRestored', { default: 'Connection restored' }),
                'warning'
            );
            // Re-evaluate onboarding: if profile creation was deferred
            // while offline, trigger it now that the DHT is reachable.
            await initOnboarding();
        } catch (e: any) {
            reconnecting = false;
            errorStore.pushError(
                $_('app.connectionFailed', { default: 'Unable to reconnect to Holochain. Please restart the application.' }),
                'error'
            );
        }
    }

    onMount(async () => {
        try {
            // Tauri-only: before we even try to connect a client, ask the
            // shell whether the conductor has an edet install. If it's a
            // fresh device we present the Restore-or-Create screen and
            // defer the rest of boot until the user picks a path.
            if (tauriBridge.isTauri()) {
                try {
                    loadingKey = 'app.loading.conductor';
                    const state = await tauriBridge.startupState();
                    if (state.kind === 'fresh') {
                        // No edet identity on this device yet. Start the
                        // onboarding wizard — it offers both "Create new" and
                        // "Restore from backup" paths. The standalone
                        // RestoreScreen is no longer shown as a first step.
                        loading = false;
                        beginOnboarding({
                            needsLocaleSetup: !hasUserConfiguredLocale(),
                            needsMnemonic: true,
                            needsTour: true,
                        });
                        return;
                    }
                    if (state.kind === 'error') {
                        errorStore.pushError(state.message, 'error');
                    }
                } catch (e: any) {
                    console.warn('startup_state failed; proceeding with hc-spin path', e);
                }
            }

            loadingKey = 'app.loading.connecting';
            const c = await connectWithRetry();
            loadingKey = 'app.loading.wallet';
            clientStore.set(c);
            attachCloseListener(c);
            markConnected();
            await initOnboarding();
            startBackupScheduler(produceBackup);
        } catch (e: any) {
            fatalError = true;
            errorStore.pushError(
                $_('app.connectionFailed', { default: 'Unable to connect to Holochain. Please check that the conductor is running.' }),
                'error'
            );
        }
        loading = false;
    });

    /**
     * Decide whether to show the first-run wizard. Gating rule: if the
     * user has never explicitly acknowledged their locale preferences, or
     * no backup has been saved locally, or the tour hasn't been seen,
     * launch onboarding; otherwise mark it complete.
     */
    async function initOnboarding() {
        if (!client) return;
        const needsLocaleSetup = !hasUserConfiguredLocale();
        const needsMnemonic = !(await hasCurrentBackup());
        const needsTour = lsGet('edet-tour-seen') !== '1';
        if (needsLocaleSetup || needsMnemonic || needsTour) {
            beginOnboarding({ needsLocaleSetup, needsMnemonic, needsTour });
        } else {
            completeOnboarding();
        }
    }

    /**
     * Backup handler used by the scheduler. Called whenever the scheduler's
     * debounced window closes with a pending request. Steps:
     *
     *   1. Load the mnemonic-derived backup key that was wrapped with a
     *      device-local key at onboarding. If absent, the scheduler is a
     *      no-op (onboarding hasn't finished or the user cleared their
     *      secrets).
     *   2. Dump the current source chain via the Tauri bridge. In browser
     *      dev mode this returns an empty array; that's acceptable because
     *      the bundle's role in dev is to exercise the encode path.
     *   3. Re-encrypt the whole payload against the existing mnemonic
     *      fingerprint (read back from the stored envelope) and write it to
     *      local persistence.
     *
     * The backup key is zeroed immediately after use so the plaintext lives
     * in memory for no longer than strictly necessary.
     */
    async function produceBackup(reason: string): Promise<void> {
        if (!client) return;
        const backupKey = await loadBackupKey();
        if (!backupKey) {
            console.debug(`[app] backup trigger "${reason}" — no wrapped backup key; onboarding not complete`);
            return;
        }
        // Read the existing envelope to reuse the mnemonic fingerprint; we
        // don't re-compute it because the mnemonic is no longer in memory.
        let fingerprint: Uint8Array | null = null;
        try {
            const existing = await readCurrentBackup();
            if (existing && existing.length > 0) {
                fingerprint = parseEnvelope(existing).mnemonic_fp;
            }
        } catch (e) {
            console.warn('[app] could not read existing backup envelope for fingerprint', e);
        }
        if (!fingerprint) {
            // Without a fingerprint the restore flow cannot cross-check the
            // mnemonic. Refuse to write a bundle that can't be validated.
            console.warn('[app] skipping backup write — no fingerprint available');
            // Zero the key before returning.
            for (let i = 0; i < backupKey.length; i++) backupKey[i] = 0;
            return;
        }

        let sourceChain: unknown[] = [];
        let dnaHash = new Uint8Array(0);
        try {
            const appInfo = await client.appInfo();
            const cellList = (appInfo as any)?.cell_info?.edet ?? [];
            const cellInfo = cellList[0] ?? null;
            const cellIdPair = cellInfo
                ? (cellInfo.provisioned?.cell_id ?? cellInfo.cell_id ?? null)
                : null;
            if (cellIdPair) {
                dnaHash = cellIdPair[0];
                sourceChain = await tauriBridge.dumpSourceChain(cellIdPair);
            }
        } catch (e) {
            console.warn('[app] dumpSourceChain failed during scheduled backup', e);
        }

        const payload: BackupPayload = {
            schema_version: 1,
            created_at_us: Date.now() * 1000,
            app_id: 'edet',
            dna_hash: dnaHash,
            agent_pub_key: client.myPubKey,
            source_chain: sourceChain,
        };
        try {
            const ciphertext = encodeBackup(payload, backupKey, fingerprint);
            await writeCurrentBackup(ciphertext, client.myPubKey);
            console.debug(`[app] backup written (reason=${reason}, size=${ciphertext.length}B, records=${sourceChain.length})`);
        } finally {
            // Best-effort zeroisation of the unwrapped key.
            for (let i = 0; i < backupKey.length; i++) backupKey[i] = 0;
        }
    }

    // Expose a getter that always dereferences the store. Child components call
    // getClient() inside onMount/event handlers (after the client is set), so
    // the store indirection ensures they never capture a stale undefined snapshot.
    //
    // installAndConnect is called from MnemonicConfirm (create-new path) after
    // the user has confirmed their phrase. It imports the agent seed into lair,
    // installs edet, then opens the AppWebsocket and sets the client store so
    // subsequent steps (BackupExport) can use the client immediately.
    setContext(clientContext, {
        getClient: () => $clientStore,
        // Used by MnemonicConfirm (create-new path): imports seed, installs, connects.
        installAndConnect: async (agentSeed: Uint8Array): Promise<void> => {
            const pubKey = await tauriBridge.importLairSeed(agentSeed);
            await tauriBridge.installAppWithAgentKey(pubKey);
            const c = await connectWithRetry();
            clientStore.set(c);
            attachCloseListener(c);
            markConnected();
            startBackupScheduler(produceBackup);
        },
        // Used by RestoreBackupStep (restore path): app is already installed
        // by RestoreScreen; just open the AppWebsocket and set the client.
        connectOnly: async (): Promise<void> => {
            const c = await connectWithRetry();
            clientStore.set(c);
            attachCloseListener(c);
            markConnected();
            startBackupScheduler(produceBackup);
        },
    });

    function toggleDrawer() {
        openDrawer = !openDrawer;
    }

    function toggleSection(section: number) {
        openSection = section;
        openDrawer = false;
    }

</script>

<ErrorSnackBar />

<!-- Skip-to-content link: only visible on keyboard focus, allowing screen-reader and
     keyboard users to bypass the drawer navigation and jump directly to the main content. -->
<a href="#main-content" class="skip-to-content">{$_('app.skipToContent', {default: 'Skip to content'})}</a>

{#if connectionLost}
    <div class="connection-banner" role="alert" aria-live="assertive">
        <i class="material-icons" aria-hidden="true">{reconnecting ? 'sync' : 'wifi_off'}</i>
        <span>
            {reconnecting
                ? $_('app.connectionLost', { default: 'Connection to Holochain lost. Reconnecting…' })
                : $_('app.connectionFailed', { default: 'Unable to reconnect. Please restart the application.' })}
        </span>
    </div>
{/if}

{#if $isLoading || loading}
    <div class="full-page-center">
        <CircularProgress class="circular-progress" indeterminate/>
        {#if loadingKey}
            <p class="loading-status">{$_(loadingKey)}</p>
        {/if}
    </div>
{:else if fatalError}
    <div class="full-page-center">
        <div class="fatal-error-container">
            <i class="material-icons fatal-error-icon" aria-hidden="true">wifi_off</i>
            <p>{$_('app.connectionFailed', { default: 'Unable to connect to Holochain. Please check that the conductor is running.' })}</p>
        </div>
    </div>
{:else}
    <div class="drawer-frame">
    <Drawer id="app-drawer" open={openDrawer} variant="dismissible" aria-label={$_('app.navigation', {default: 'Navigation'})}>
        <Header>
            <Title>{$_('app.name')}</Title>
            <!-- svelte-ignore missing-declaration -->
            <Subtitle>{__APP_VERSION__}</Subtitle>
        </Header>
        <Content>
            <List>
                {#each [...Array(8).keys()] as section}
                    <Item on:click={() => toggleSection(section)}
                        activated={openSection === section}
                        data-tour={section === 0
                            ? 'drawer-wallet'
                            : section === 1
                                ? 'drawer-transactions'
                                : section === 2
                                    ? 'drawer-contracts'
                                    : section === 3 || section === 4 || section === 5 || section === 6
                                        ? 'drawer-support'
                                        : section === 7
                                            ? 'drawer-settings'
                                            : undefined}>
                        <Graphic class="material-icons">
                            {#if section === 0}account_balance_wallet
                            {:else if section === 1}swap_horiz
                            {:else if section === 2}description
                            {:else if section === 3}favorite
                            {:else if section === 4}group
                            {:else if section === 5}verified_user
                            {:else if section === 6}card_membership
                            {:else if section === 7}settings
                            {/if}
                        </Graphic>
                        <span>{$_('app.sections.' + section)}</span>
                    </Item>
                {/each}
            </List>
        </Content>
    </Drawer>

    <AppContent class="app-content">
        <TopAppBar bind:this={topAppBar} variant="fixed">
            <Row>
                <Section>
                    <IconButton
                        class="material-icons"
                        aria-label={$_('app.toggleNav', {default: 'Toggle navigation'})}
                        aria-expanded={openDrawer}
                        aria-controls="app-drawer"
                        on:click={() => toggleDrawer()}>menu</IconButton>
                    <Title>{$_('app.sections.' + openSection)}</Title>
                </Section>
            </Row>
        </TopAppBar>
        <AutoAdjust {topAppBar}>
            <main id="main-content" tabindex="-1">
                {#if loading || !$clientStore}
                    <div class="center-container">
                        <CircularProgress class="circular-progress" indeterminate/>
                    </div>
                {:else}
                    <div id="content">
                        {#if openSection === 0}
                            <MyWallet></MyWallet>
                        {:else if openSection === 1}
                            <MyTransactions></MyTransactions>
                        {:else if openSection === 2}
                            <MyContracts></MyContracts>
                        {:else if openSection === 3}
                            <SupportBreakdown></SupportBreakdown>
                        {:else if openSection === 4}
                            <MySupporters></MySupporters>
                        {:else if openSection === 5}
                            <MyVouchesGiven></MyVouchesGiven>
                        {:else if openSection === 6}
                            <MyVouchesReceived></MyVouchesReceived>
                        {:else if openSection === 7}
                            <Settings></Settings>
                        {/if}
                    </div>
                {/if}
            </main>
        </AutoAdjust>
    </AppContent>
    </div>

    {#if $onboardingActive && $onboardingStep !== 'tour'}
        <Onboarding />
    {/if}
{/if}


<style>
    /* Skip-to-content: visually hidden until focused by keyboard */
    .skip-to-content {
        position: absolute;
        left: -9999px;
        top: 8px;
        z-index: 10000;
        padding: 8px 16px;
        background: var(--mdc-theme-primary, #6200ee);
        color: #fff;
        border-radius: 4px;
        font-size: 0.9rem;
        font-weight: 600;
        text-decoration: none;
    }
    .skip-to-content:focus {
        left: 8px;
    }

    main {
        padding: 0;
        margin: 0 auto;
        display: flex;
        flex-direction: column;
        width: 100%;
        align-items: center;
        /* Allow programmatic focus for skip-to-content without a visible outline
           (the outline is irrelevant here since main is not interactive itself) */
        outline: none;
    }

    @media (min-width: 640px) {
        main {
            width: 100%;
            max-width: 1600px;
        }
    }

    /*
     * Sticky layout: fixed TopAppBar + sticky dismissible Drawer.
     *
     * SMUI's dismissible drawer renders:
     *   <aside class="mdc-drawer mdc-drawer--dismissible">   ← sibling
     *   <div   class="mdc-drawer-app-content app-content">   ← sibling
     *     <header class="mdc-top-app-bar">                   ← fixed bar
     *     <div class="mdc-top-app-bar--fixed-adjust">        ← scrollable area
     *
     * Strategy:
     *   - The drawer + app-content row gets height:100vh / overflow:hidden
     *     so it never itself scrolls.
     *   - The drawer gets height:100% / overflow-y:auto so its nav list
     *     scrolls independently without affecting the rest.
     *   - The AutoAdjust div (fixed-adjust) is the only scrolling element.
     *   - The fixed TopAppBar sits above everything via its own z-index.
     */

    /* Parent flex row wrapping Drawer + AppContent */
    .drawer-frame {
        display: flex;
        height: 100vh;
        overflow: hidden;
        /* Landscape: keep content clear of the camera cutout on the left */
        padding-left: env(safe-area-inset-left);
        padding-right: env(safe-area-inset-right);
        padding-bottom: env(safe-area-inset-bottom);
    }

    /*
     * SMUI's dismissible drawer uses position:absolute + margin-left on
     * AppContent to simulate the slide-in/out. In a flex layout we need the
     * drawer to participate in normal flow instead.
     */

    /* Drawer: participate in flex flow, full height, own scroll.
     * margin-top pushes it below the fixed TopAppBar (56px mobile / 64px desktop)
     * plus the status-bar safe area we added to the bar itself. */
    :global(.mdc-drawer--dismissible) {
        position: relative !important;
        height: calc(100vh - 56px - env(safe-area-inset-top, 0px));
        overflow-y: auto;
        flex-shrink: 0;
        margin-top: calc(56px + env(safe-area-inset-top, 0px));
    }
    @media (min-width: 600px) {
        :global(.mdc-drawer--dismissible) {
            height: calc(100vh - 64px - env(safe-area-inset-top, 0px));
            margin-top: calc(64px + env(safe-area-inset-top, 0px));
        }
    }

    /* Portrait: push TopAppBar below the status bar */
    :global(.mdc-top-app-bar) {
        padding-top: env(safe-area-inset-top);
    }

    /* When closed, hide it but keep it in flow so layout doesn't jump */
    :global(.mdc-drawer--dismissible:not(.mdc-drawer--open)) {
        display: none !important;
    }

    /* AppContent: remove the margin SMUI adds for the abs-positioned drawer */
    :global(.mdc-drawer-app-content) {
        margin-left: 0 !important;
        margin-right: 0 !important;
    }

    /* AppContent: fill remaining width, column layout, no overflow */
    :global(.mdc-drawer-app-content.app-content) {
        flex: 1 1 0;
        min-width: 0;
        height: 100vh;
        overflow: hidden;
        display: flex;
        flex-direction: column;
    }

    /* AutoAdjust element below the fixed bar: fill remaining height and scroll.
     * Override MDC's default padding-top (56px mobile / 64px desktop) to also
     * account for the safe-area-inset-top we added to the TopAppBar itself. */
    :global(.mdc-drawer-app-content.app-content .mdc-top-app-bar--fixed-adjust) {
        flex: 1 1 0;
        min-height: 0;
        overflow-y: auto;
        padding-top: calc(56px + env(safe-area-inset-top, 0px));
    }
    @media (min-width: 600px) {
        :global(.mdc-drawer-app-content.app-content .mdc-top-app-bar--fixed-adjust) {
            padding-top: calc(64px + env(safe-area-inset-top, 0px));
        }
    }

    .loading-status {
        margin-top: 16px;
        font-size: 0.9rem;
        color: var(--mdc-theme-text-secondary-on-background, rgba(0,0,0,0.6));
        text-align: center;
    }

    .fatal-error-container {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 16px;
        padding: 32px;
        text-align: center;
        color: var(--mdc-theme-error, #b00020);
    }

    .fatal-error-icon {
        font-size: 48px;
    }

    .connection-banner {
        position: fixed;
        top: 0;
        left: 0;
        right: 0;
        z-index: 9999;
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 8px 16px;
        background: var(--mdc-theme-error, #b00020);
        color: #fff;
        font-size: 0.875rem;
        font-weight: 500;
    }
    
</style>
