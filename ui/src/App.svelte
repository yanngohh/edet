<script lang="ts">
    import { _, isLoading, locale } from 'svelte-i18n';
    import { onDestroy, onMount, setContext, tick } from 'svelte';
    import CircularProgress from '@smui/circular-progress';
    import TopAppBar, { Row, Section, AutoAdjust } from '@smui/top-app-bar';
    import IconButton from '@smui/icon-button';
    import Drawer, { AppContent, Content, Header, Subtitle, Title } from '@smui/drawer';
    import List, { Item, Graphic } from '@smui/list';
    import 'material-icons/iconfont/material-icons.css';

    import ErrorSnackBar from './common/ErrorSnackBar.svelte';
    import { localizationSettings, hasUserConfiguredLocale } from './common/localizationSettings';
    import { lsGet, lsSet } from './common/safeStorage';
    import {
        beginOnboarding,
        completeOnboarding,
        markTourSeen,
        advance,
        onboardingActive,
        onboardingStep,
        onboardingIsReplay,
        resumeActorChoice,
    } from './common/onboardingStore';

    import { get } from 'svelte/store';
    import { startPolling, stopPolling, nodeUp, firstLoadDone, pendingCount, supporterApprovals, activeBase, nodeUrlLocked } from './lib/node';
    import { startAcceptance, stopAcceptance } from './lib/autosign';
    import { applySystemInsets, startBackground, stopBackground } from './lib/background';
    import { startWaitingNotices, stopWaitingNotices } from './lib/waiting';
    import { ensureSession, keyProof } from './lib/session';
    import { clearActor, currentActorId, heldSeed, initActors, pendingIdentity } from './lib/actors';
    import * as api from './lib/api';
    import { bytesToHex, derivePublicKey } from './lib/crypto';
    import { runTour } from './edet/tour';

    import MyWallet from './edet/wallet/MyWallet.svelte';
    import Requests from './edet/requests/Requests.svelte';
    import Transactions from './edet/transactions/Transactions.svelte';
    import MyContracts from './edet/contracts/MyContracts.svelte';
    import Receivables from './edet/contracts/Receivables.svelte';
    import Community from './edet/community/Community.svelte';
    import Support from './edet/support/Support.svelte';
    import Governance from './edet/governance/Governance.svelte';
    import NetworkStatus from './edet/network/NetworkStatus.svelte';
    import Settings from './edet/Settings.svelte';
    import Onboarding from './edet/onboarding/Onboarding.svelte';
    import UnlockVault from './edet/UnlockVault.svelte';
    import { isLocked, lock, lockState } from './common/vault';

    /**
     * Where the drawer stops overlaying the page and starts sitting beside it.
     *
     * Material's expanded breakpoint, and the stylesheet's `@media` uses the
     * same number. Below it the drawer is out of flow over the content and a
     * section closes it; at or above it the drawer takes its own column and
     * only the menu button opens or closes it — there is room for both, so
     * nothing should move under the member while they navigate.
     */
    const DRAWER_BREAKPOINT = 840;

    /**
     * **MDC caches a slider's track rectangle, and nothing was telling it to
     * look again.** SMUI's `Slider` asks the Svelte context for
     * `SMUI:addLayoutListener` and registers its own `layout` there; the
     * containers that provide it are `Dialog` and `List`, so a slider sitting
     * in an ordinary page had nobody to hear it. After a rotation the thumb
     * therefore sat at the position the OLD width put it, ahead of the value
     * bar that had already been redrawn — visible on the acceptance rule's two
     * sliders and the support circle's row of them.
     *
     * Provided here, at the root, because the width of a page changes for two
     * different reasons and both belong to this component: the screen turns,
     * and the drawer takes or gives back its column. Every SMUI component
     * beneath this one now gets told about both.
     */
    const layoutListeners: Array<() => void> = [];
    setContext('SMUI:addLayoutListener', (listener: () => void) => {
        layoutListeners.push(listener);
        return () => {
            const i = layoutListeners.indexOf(listener);
            if (i >= 0) layoutListeners.splice(i, 1);
        };
    });

    function relayout(): void {
        // A frame later: `resize` fires before the new geometry has settled,
        // and a rect read now is the one being replaced.
        requestAnimationFrame(() => {
            for (const listener of [...layoutListeners]) {
                try {
                    listener();
                } catch {
                    // A component torn down between the event and this frame.
                }
            }
        });
    }

    let viewportWidth = typeof window !== 'undefined' ? window.innerWidth : 0;
    $: narrowNav = viewportWidth > 0 && viewportWidth < DRAWER_BREAKPOINT;

    // Open where there is room for it, shut where there is not. Read once, at
    // the width the app started in: after that it is the member's, and a
    // rotation must not reopen a drawer they closed.
    let openDrawer = typeof window !== 'undefined' && window.innerWidth >= DRAWER_BREAKPOINT;
    let openSection = 0;
    let topAppBar: TopAppBar;

    /**
     * The number a drawer item carries, or 0.
     *
     * Requests counts signatures owed BY you. Support Circle counts approvals
     * owed by you to somebody else — a decision that raises no pending-pool
     * entry (listing you asks nothing of you; only draining does), so it lives
     * on one page and was reachable from nowhere else. A member had no way to
     * learn that somebody was waiting on them short of opening the page on a
     * hunch.
     */
    $: drawerCount = (section: number): number =>
        section === 11 ? $pendingCount : section === 5 ? $supporterApprovals : 0;
    // The identity vault must decrypt before anything can render or sign.
    let booted = false;
    let bootFailed = false;
    // Render order; ids are stable (11 = Requests, listed after Transactions
    // without renumbering the others). 6 is retired: the gap is deliberate,
    // because these ids key the locale strings and the tour, and renumbering
    // would silently re-label every screen below it in six languages.
    const SECTIONS = [0, 1, 11, 2, 3, 4, 5, 8, 9, 10] as const;
    const SECTION_ICONS: Record<number, string> = {
        0: 'account_balance_wallet',
        1: 'swap_horiz',
        11: 'draw',
        2: 'description',
        3: 'request_quote',
        4: 'groups',
        5: 'diversity_3',
        7: 'savings',
        8: 'how_to_vote',
        9: 'hub',
        10: 'settings',
    };
    const SECTION_TOUR: Record<number, string | undefined> = {
        0: 'drawer-wallet',
        1: 'drawer-transactions',
        2: 'drawer-contracts',
        3: 'drawer-receivables',
        4: 'drawer-community',
        5: 'drawer-support',
        10: 'drawer-settings',
    };

    $: if ($localizationSettings.locale) {
        $locale = $localizationSettings.locale;
        // Keep <html lang> in sync so screen readers announce content in the
        // correct language.
        if (typeof document !== 'undefined') {
            document.documentElement.lang = $localizationSettings.locale.split('-')[0];
        }
    }

    // Shepherd tour host. `runTour` calls these two hooks before each step to
    // open the drawer and switch to the target section — otherwise the
    // popovers would anchor to invisible elements. The same host powers the
    // first-run flow and Settings' "Replay tour".
    const tourHost = {
        setDrawerOpen: (o: boolean) => { openDrawer = o; },
        setSection:    (idx: number) => { openSection = idx; },
        hasWallet:     () => get(currentActorId) !== null || get(pendingIdentity) !== null,
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
            if ($onboardingIsReplay) {
                // Settings' "Replay tour" — onboarding already completed
                // once, so `advance()`'s wizard sequencing would misfire.
                // Just return to the app.
                completeOnboarding();
            } else {
                // Advance rather than complete outright: this re-shows the
                // <Onboarding /> overlay on the `economics-intro` step. That
                // step's own "Continue" calls completeOnboarding().
                advance();
            }
        }
    }

    // When the onboarding store lands on `tour`, unmount the overlay (see
    // gate on <Onboarding /> below) and launch Shepherd against the real app
    // DOM. Guard on `$firstLoadDone` so the tour never starts before the
    // node has answered once — the wallet metrics it points at would be
    // empty placeholders.
    $: if ($onboardingStep === 'tour' && !tourRunning && $firstLoadDone) {
        void startTour();
    }

    /**
     * Stale-profile self-heal: an identity restored on this device may no
     * longer match the ledger (a dev cluster that re-ran genesis, or a key
     * rotated away elsewhere). If the held key does not resolve to the
     * stored member id, drop the acting identity and let onboarding run —
     * never destroy the seed. Unreachable node → keep the identity.
     */
    async function validateActor(): Promise<void> {
        const id = get(currentActorId);
        if (id === null) return;
        const seed = heldSeed(id);
        if (!seed) {
            clearActor();
            return;
        }
        const keyHex = bytesToHex(derivePublicKey(seed));
        for (let attempt = 0; attempt < 10; attempt++) {
            try {
                // Runs right after vault unlock, when a session may not be
                // minted yet — so it carries its own key proof rather than
                // relying on a bearer token that might not exist. It holds
                // the seed for this very key, so proving it costs nothing.
                const who = await api.whois(
                    get(activeBase),
                    keyHex,
                    keyProof(derivePublicKey(seed), seed, 'GET', `/whois/${keyHex}`),
                );
                if (who.member !== id) clearActor();
                return;
            } catch {
                await new Promise((r) => setTimeout(r, 500));
            }
        }
    }

    onMount(async () => {
        // **Before anything renders a control.** The room the system bars take
        // is a layout fact, not a wallet fact: it must not wait on the vault,
        // or the unlock screen and the first frame of every page are drawn
        // under the navigation bar and only straighten out later.
        void applySystemInsets();
        // The passphrase gate comes FIRST, because the vault cannot be opened
        // without it: `loadVault` throws `vault-locked` rather than returning
        // an empty ring, so asking afterwards would present a boot failure to
        // a member whose only problem is that they have not typed it yet.
        if (await isLocked()) {
            lock();
            return;
        }
        await boot();
    });

    /** Everything that needs the vault open. Re-entered after an unlock. */
    async function boot(): Promise<void> {
        // Open the encrypted identity vault before anything renders: every
        // write path signs with seeds that live in it.
        try {
            await initActors();
        } catch (e) {
            console.error('vault init failed', e);
            bootFailed = true;
            return;
        }
        await validateActor();
        // Authenticated reads: mint one read-session token now that
        // the vault has actually loaded the acting identity's seed — the
        // read-side analogue of the vault "unlock" this device just did.
        // The `currentActorId` value hasn't necessarily CHANGED across the
        // vault load (it was read from localStorage before the seed was
        // available), so node.ts's actor-change subscription alone would
        // miss this case; this call is what actually covers it.
        void ensureSession(get(activeBase), get(currentActorId));
        booted = true;
        startPolling();
        // The wallet's acceptance rule, applied to the pending pool for as
        // long as this app is running and the vault unlocked.
        startAcceptance();
        // And what keeps it running once the member looks elsewhere: a tray on
        // the desktop, a foreground service and a re-resumed WebView on
        // Android. After `startPolling`, because its visibility handler only
        // ever re-paces a poll that has already started.
        startBackground();
        // And the half the rule cannot decide: a request left in the hold
        // band, or an approval that raises no pool entry at all.
        startWaitingNotices();

        // First-run wizard gate: any outstanding step launches the wizard at
        // the earliest one.
        // First run: options, then what edet is, then a NETWORK, then a key,
        // then the tour. The network comes before the key because a key earns
        // standing on one ledger and cannot carry it to another.
        // A key is "held" whether or not the ledger has a row for it yet —
        // making one is local, and the trade that seats it is waited for in
        // the wallet, not here.
        const needsLocaleSetup = !hasUserConfiguredLocale();
        // Asked before the key, because standing cannot be carried between
        // ledgers — see `NetworkChoiceStep`. A launcher-pinned instance
        // (`EDET_NODE`) has no choice to make.
        // A flag of its own, NOT `lsGet('edet-network') === null`: `node.ts`'s
        // `networkId` store persists on subscription, so its default is
        // already in storage before this line runs and the step would never
        // appear on a genuine first run. Mirrors `edet-economics-seen`.
        const needsNetwork = !nodeUrlLocked && lsGet('edet-network-chosen') !== '1';
        const needsEconomics = lsGet('edet-economics-seen') !== '1';
        const needsKey = $currentActorId === null && get(pendingIdentity) === null;
        const needsTour = lsGet('edet-tour-seen') !== '1';
        if (needsLocaleSetup || needsNetwork || needsEconomics || needsKey || needsTour) {
            beginOnboarding({ needsLocaleSetup, needsNetwork, needsEconomics, needsKey, needsTour });
        } else {
            completeOnboarding();
        }
    }

    onDestroy(() => {
        stopPolling();
        stopAcceptance();
        stopBackground();
        stopWaitingNotices();
    });

    function toggleDrawer() {
        openDrawer = !openDrawer;
        // Taking or giving back the drawer's column resizes every page under
        // it, and no `resize` event says so.
        relayout();
    }

    function toggleSection(section: number) {
        openSection = section;
        // Only where it is covering what the member just chose.
        if (narrowNav) openDrawer = false;
    }

    // Automatically reset scroll positions whenever the active section changes
    $: {
        if (openSection !== undefined && typeof document !== 'undefined') {
            const resetScroll = () => {
                const containers = document.querySelectorAll('.mdc-top-app-bar--fixed-adjust');
                containers.forEach(el => {
                    el.scrollTop = 0;
                });

                const mainContent = document.getElementById('main-content');
                if (mainContent) mainContent.scrollTop = 0;

                const content = document.getElementById('content');
                if (content) content.scrollTop = 0;

                window.scrollTo(0, 0);
            };

            resetScroll();

            // Run again after layout updates to handle async component mounts
            tick().then(() => {
                resetScroll();
                setTimeout(resetScroll, 0);
                setTimeout(resetScroll, 50);
                setTimeout(resetScroll, 150);
            });
        }
    }
</script>

<svelte:window bind:innerWidth={viewportWidth} on:resize={relayout} on:orientationchange={relayout} />

<ErrorSnackBar />

<!-- Skip-to-content link: only visible on keyboard focus. -->
<a href="#main-content" class="skip-to-content">{$_('app.skipToContent', {default: 'Skip to content'})}</a>

{#if !$nodeUp}
    <div class="connection-banner" role="alert" aria-live="assertive">
        <i class="material-icons" aria-hidden="true">wifi_off</i>
        <span>{$_('app.connectionLost', { default: 'Cannot reach the node. Retrying…' })}</span>
    </div>
{/if}

{#if booted && !bootFailed && $currentActorId === null && $pendingIdentity === null && !$onboardingActive}
    <div class="browsing-banner">
        <i class="material-icons" aria-hidden="true">visibility</i>
        <span>{$_('app.browsing.note', { default: 'You are looking around. Trading needs an identity on this device.' })}</span>
        <button type="button" class="browsing-cta" on:click={() => resumeActorChoice()}>
            {$_('app.browsing.cta', { default: 'Set one up' })}
        </button>
    </div>
{/if}

{#if $lockState === 'locked'}
    <UnlockVault onUnlocked={boot} />
{:else if bootFailed}
    <div class="full-page-center">
        <div class="flex-column" style="align-items: center; gap: 12px; max-width: 480px; text-align: center;">
            <i class="material-icons" style="font-size: 40px; color: var(--mdc-theme-error, #d32f2f);" aria-hidden="true">lock</i>
            <p>
                {$_('app.vaultError', {
                    default:
                        'The identity vault on this device could not be opened. Your seeds are intact but unreadable (corrupt browser storage?). Restore from your recovery phrase or an exported backup on a fresh profile.',
                })}
            </p>
        </div>
    </div>
{:else if $isLoading || !booted}
    <div class="full-page-center">
        <CircularProgress class="circular-progress" indeterminate/>
    </div>
{:else}
    <div class="drawer-frame">
    <Drawer id="app-drawer" open={openDrawer} variant="dismissible" aria-label={$_('app.navigation', {default: 'Navigation'})}>
        <Header class="drawer-header">
            <div class="drawer-brand-container">
                <img src="/icon.svg" alt="edet logo" class="drawer-logo" />
                <div class="drawer-brand-text">
                    <Title class="drawer-title">{$_('app.name', { default: 'edet' })}</Title>
                    <!-- svelte-ignore missing-declaration -->
                    <Subtitle class="drawer-subtitle">{__APP_VERSION__}</Subtitle>
                </div>
            </div>
        </Header>
        <Content>
            <List>
                {#each SECTIONS as section}
                    <Item on:click={() => toggleSection(section)}
                        activated={openSection === section}
                        data-tour={SECTION_TOUR[section]}>
                        <Graphic class="material-icons">{SECTION_ICONS[section]}</Graphic>
                        <span>
                            {$_('app.sections.' + section)}{drawerCount(section) > 0 ? ` · ${drawerCount(section)}` : ''}
                        </span>
                    </Item>
                {/each}
            </List>
        </Content>
    </Drawer>

    <!-- Only where the drawer covers the page: a scrim over content that is
         still there, so a tap anywhere outside the list dismisses it. A
         button, not a div, so it is reachable without a pointer. -->
    {#if openDrawer && narrowNav}
        <button
            type="button"
            class="drawer-scrim"
            aria-label={$_('app.closeNav', { default: 'Close navigation' })}
            on:click={() => (openDrawer = false)}
        ></button>
    {/if}

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
                <div id="content">
                    {#if openSection === 0}
                        <MyWallet></MyWallet>
                    {:else if openSection === 1}
                        <Transactions></Transactions>
                    {:else if openSection === 11}
                        <Requests></Requests>
                    {:else if openSection === 2}
                        <MyContracts></MyContracts>
                    {:else if openSection === 3}
                        <Receivables></Receivables>
                    {:else if openSection === 4}
                        <Community></Community>
                    {:else if openSection === 5}
                        <Support></Support>
                    {:else if openSection === 8}
                        <Governance></Governance>
                    {:else if openSection === 9}
                        <NetworkStatus></NetworkStatus>
                    {:else if openSection === 10}
                        <Settings></Settings>
                    {/if}
                </div>
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

    .browsing-banner {
        position: fixed;
        bottom: 0;
        left: 0;
        right: 0;
        z-index: 9000;
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 8px;
        padding: 8px 12px;
        background: var(--mdc-theme-secondary, #018786);
        color: #fff;
        font-size: 0.85rem;
    }
    .browsing-banner .material-icons {
        font-size: 18px;
    }
    .browsing-cta {
        border: 1px solid rgba(255, 255, 255, 0.7);
        background: transparent;
        color: inherit;
        font: inherit;
        font-weight: 600;
        padding: 2px 10px;
        border-radius: 4px;
        cursor: pointer;
    }

    .connection-banner {
        position: fixed;
        top: 0;
        left: 0;
        right: 0;
        z-index: 9000;
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 8px;
        padding: 6px 12px;
        background: #d32f2f;
        color: #fff;
        font-size: 0.85rem;
    }

    main {
        padding: 0;
        margin: 0 auto;
        display: flex;
        flex-direction: column;
        width: 100%;
        max-width: 100vw;
        overflow-x: hidden;
        box-sizing: border-box;
        align-items: center;
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
     *   - The drawer + app-content row gets height:100dvh / overflow:hidden
     *     so it never itself scrolls.
     *   - The drawer gets height:100% / overflow-y:auto so its nav list
     *     scrolls independently without affecting the rest.
     *   - The AutoAdjust div (fixed-adjust) is the only scrolling element.
     *   - The fixed TopAppBar is raised ABOVE the drawer, which it is not by
     *     default — see the z-index rule below.
     *
     * **`dvh`, not `vh`.** On a phone `100vh` is the viewport with the URL
     * bar and system insets ignored, so it is TALLER than what you can see:
     * the frame overflows, the page itself scrolls, and the drawer slides up
     * into the band the fixed bar occupies. `dvh` is the visible viewport, so
     * the frame ends where the screen does and nothing behind it scrolls at
     * all. `vh` stays as the first declaration for engines without `dvh`.
     */

    /*
     * **`env()` is not enough on Android, and it fails SILENTLY.** Measured in
     * the live WebView on a handset: `safe-area-inset-top` reads 37 px and
     * `safe-area-inset-bottom` reads 0, because Android maps the safe area
     * from the display cutout and never from the navigation bar — so an
     * edge-to-edge page puts its own footer under the nav buttons and the
     * stylesheet has no way to know. `MainActivity` publishes what
     * `WindowInsetsCompat` reports as `--edet-inset-*`, and every inset below
     * is the `max()` of the two: a platform where `env()` is right is not
     * double-counted, and one where it is silent is still correct.
     *
     * `--edet-bar` is the top app bar's height plus the top inset, in one
     * place. It was four copies of the same `calc`, and the bar's height
     * appears in each of them.
     */
    .drawer-frame {
        --edet-inset-t: max(env(safe-area-inset-top, 0px), var(--edet-inset-top, 0px));
        --edet-bar: calc(56px + var(--edet-inset-t));
        position: relative;
        display: flex;
        /* border-box, or the paddings below ADD to the height and push the
           bottom of the app back under the bar this is reserving room for. */
        box-sizing: border-box;
        height: 100vh;
        height: 100dvh;
        overflow: hidden;
        padding-left: max(env(safe-area-inset-left, 0px), var(--edet-inset-left, 0px));
        padding-right: max(env(safe-area-inset-right, 0px), var(--edet-inset-right, 0px));
        padding-bottom: max(env(safe-area-inset-bottom, 0px), var(--edet-inset-bottom, 0px));
    }
    @media (min-width: 600px) {
        .drawer-frame {
            --edet-bar: calc(64px + var(--edet-inset-t));
        }
    }

    /* Drawer: participate in flex flow, full height, own scroll.
     *
     * `100%` of the frame's CONTENT box, not `100dvh`: the frame reserves the
     * bottom inset as padding, and a child sized to the whole viewport would
     * spend it again. */
    :global(.mdc-drawer--dismissible) {
        position: relative !important;
        height: calc(100% - var(--edet-bar));
        overflow-y: auto;
        flex-shrink: 0;
        margin-top: var(--edet-bar);
    }

    /*
     * **Narrow: the drawer covers the page instead of squeezing it.** Taking
     * 256 px out of a 408 px screen leaves a column nothing reads well in —
     * the wallet's own address wrapped into five lines behind it. Out of flow,
     * over the content, with a scrim that dismisses it; the app bar stays
     * above both, so the same button that opened it closes it. Above the
     * breakpoint there is room for both and it steals space as before, and
     * nothing but that button opens or closes it.
     *
     * 840 px is Material's expanded breakpoint, and the JS reads the same
     * number (`DRAWER_BREAKPOINT`) to decide whether picking a section closes
     * the drawer.
     */
    @media (max-width: 839px) {
        :global(.mdc-drawer--dismissible) {
            position: absolute !important;
            top: 0;
            bottom: 0;
            left: 0;
            height: auto;
            z-index: 6;
            box-shadow: 0 8px 24px rgba(0, 0, 0, 0.28);
        }
    }

    .drawer-scrim {
        position: absolute;
        top: var(--edet-bar);
        right: 0;
        bottom: 0;
        left: 0;
        z-index: 5;
        margin: 0;
        padding: 0;
        border: none;
        background: rgba(0, 0, 0, 0.32);
        cursor: pointer;
    }

    /* Portrait: push TopAppBar below the status bar.
     *
     * The fixed bar is positioned against the drawer-app-content box, so
     * MDC's `width: 100%` (of the viewport) overflows to the right by the
     * drawer's width once the drawer opens — which would push the inbox
     * button off-screen. Let the left/right edges size it instead. */
    :global(.mdc-top-app-bar) {
        padding-top: var(--edet-inset-t);
        left: 0;
        right: 0;
        width: auto;
        /*
         * **Above the drawer, which is not where MDC puts it.** The shipped
         * values are `$mdc-drawer-z-index: 6` against the top app bar's `4`,
         * so a dismissible drawer — `position: relative`, its own stacking
         * context — paints OVER the fixed bar wherever the two boxes meet.
         * In portrait that is every scroll: the nav list slid across the
         * title. 7 clears the drawer and stays under the tour's overlay and
         * the two banners, which sit in the thousands.
         */
        z-index: 7;
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

    :global(.mdc-drawer-app-content.app-content) {
        flex: 1 1 0;
        min-width: 0;
        /* The frame's content box, which already has the bottom inset taken
           off it — see `.drawer-frame`. */
        height: 100%;
        overflow: hidden;
        display: flex;
        flex-direction: column;
    }

    :global(.mdc-drawer-app-content.app-content .mdc-top-app-bar--fixed-adjust) {
        flex: 1 1 0;
        min-height: 0;
        overflow-y: auto;
        padding-top: var(--edet-bar);
    }

    .drawer-brand-container {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 16px 16px 8px;
    }

    .drawer-logo {
        width: 40px;
        height: 40px;
    }

    .drawer-brand-text {
        display: flex;
        flex-direction: column;
        min-width: 0;
    }

    :global(.drawer-title) {
        margin: 0 !important;
        line-height: 1.2 !important;
    }

    :global(.drawer-subtitle) {
        margin: 0 !important;
        line-height: 1.2 !important;
    }
</style>
