<script lang="ts">
    import { _ } from 'svelte-i18n';
    import {onMount, setContext} from 'svelte';
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
    import { localizationSettings } from './common/localizationSettings';
    import ErrorSnackBar from './common/ErrorSnackBar.svelte';
    import { errorStore } from './common/errorStore';

    import SupportBreakdown from "./edet/support/SupportBreakdown.svelte";
    import MySupporters from "./edet/support/MySupporters.svelte";
    import MyVouchesGiven from "./edet/support/MyVouchesGiven.svelte";
    import MyVouchesReceived from "./edet/support/MyVouchesReceived.svelte";
    import MyTransactions from './edet/transaction/MyTransactions.svelte';
    import MyContracts from './edet/transaction/MyContracts.svelte';
    import Settings from './edet/Settings.svelte';


    let client: AppClient | undefined;
    let loading = true;
    let connectionLost = false;
    let reconnecting = false;
    let openDrawer = false;
    let openSection = 0;
    let topAppBar: TopAppBar;

    const MAX_CONNECT_ATTEMPTS = 5;
    const BASE_BACKOFF_MS = 1000;

    $: client, loading, openDrawer, openSection;

    $: if ($localizationSettings.locale) {
        $locale = $localizationSettings.locale;
        // Keep the <html lang> attribute in sync with the active locale so that
        // screen readers announce content in the correct language.
        if (typeof document !== 'undefined') {
            document.documentElement.lang = $localizationSettings.locale.split('-')[0];
        }
    }

    /** Attempt to connect with exponential backoff. Returns the client or throws. */
    async function connectWithRetry(): Promise<AppClient> {
        let lastError: any;
        for (let attempt = 1; attempt <= MAX_CONNECT_ATTEMPTS; attempt++) {
            try {
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
            client = newClient;
            connectionLost = false;
            reconnecting = false;
            attachCloseListener(newClient);
            errorStore.pushError(
                $_('app.connectionRestored', { default: 'Connection restored' }),
                'warning'
            );
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
            client = await connectWithRetry();
            attachCloseListener(client);
        } catch (e: any) {
            errorStore.pushError(
                $_('app.connectionFailed', { default: 'Unable to connect to Holochain. Please check that the conductor is running.' }),
                'error'
            );
        }
        loading = false;
    });

    setContext(clientContext, {
        getClient: () => client,
    });

    function toggleDrawer() {
        openDrawer = !openDrawer;
    }

    function toggleSection(section: number) {
        openSection = section;
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
    </div>
{:else}
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
                        activated={openSection === section}>
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
        <TopAppBar bind:this={topAppBar} variant="static">
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
                {#if loading}
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
