/**
 * Shepherd-powered walkthrough for first-run users.
 *
 * Every step declares (a) the CSS selector its popup should point at and
 * (b) a `prepare` hook that opens the drawer and navigates to the right
 * section before the popup renders. Without the prepare hook the drawer
 * items would be invisible and Shepherd would center each popover in the
 * middle of the screen, which is what happened before this rewrite.
 *
 * Targets use `data-tour="..."` attributes rather than class names so
 * reskinning the UI can't silently detach steps.
 *
 * The tour is i18n-aware: copy is read from the current `svelte-i18n`
 * store at build time so replays after a locale switch pick up
 * translations.
 */

import Shepherd from 'shepherd.js';
import { get } from 'svelte/store';
import { _ } from 'svelte-i18n';

import 'shepherd.js/dist/css/shepherd.css';

/** How long to wait after navigating to a section before Shepherd shows the
 *  next popup. Lets Svelte finish its render + any in-flight animations. */
const NAVIGATION_DELAY_MS = 400;

/** Wait for a CSS selector to appear in the DOM, or for an attribute on an
 *  existing element to satisfy the selector (e.g. [data-loaded="true"]).
 *  Uses MutationObserver with both childList and attribute observation so it
 *  catches both element insertion *and* attribute changes on existing nodes.
 *  Resolves immediately if the element is already present.
 *  Times out after maxWaitMs and resolves anyway so the tour still starts. */
function waitForElement(selector: string, maxWaitMs = 15_000): Promise<void> {
    return new Promise((resolve) => {
        if (document.querySelector(selector)) {
            resolve();
            return;
        }
        let settled = false;
        const settle = () => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            observer.disconnect();
            resolve();
        };
        const timer = setTimeout(settle, maxWaitMs);
        const observer = new MutationObserver(() => {
            if (document.querySelector(selector)) settle();
        });
        observer.observe(document.body, {
            childList: true,
            subtree: true,
            attributes: true,
            attributeFilter: ['data-loaded'],
        });
    });
}

export const TOUR_TARGETS = {
    walletHeader: '[data-tour="wallet-header"]',
    walletMetrics: '[data-tour="wallet-metrics"]',
    walletMetricsReady: '[data-tour="wallet-metrics"][data-loaded="true"]',
    walletQr: '[data-tour="wallet-qr"]',
    drawerWallet: '[data-tour="drawer-wallet"]',
    drawerTransactions: '[data-tour="drawer-transactions"]',
    drawerContracts: '[data-tour="drawer-contracts"]',
    drawerSupport: '[data-tour="drawer-support"]',
    drawerSettings: '[data-tour="drawer-settings"]',
} as const;

/** Translate a key lazily so the tour uses the current locale on replay. */
function t(key: string, fallback: string): string {
    try {
        const translator = get(_);
        return translator(key, { default: fallback });
    } catch {
        return fallback;
    }
}

/** Hooks the caller provides so the tour can navigate between sections. */
export interface TourHost {
    /** Open or close the drawer. Called with `true` before drawer-targeted steps. */
    setDrawerOpen(open: boolean): void;
    /** Switch the active content section by index (matches App.svelte). */
    setSection(index: number): void;
}

function wait(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Build and run the tour. Returns a promise that resolves when the user
 * completes or cancels it. Safe to call multiple times.
 */
export async function runTour(host: TourHost): Promise<void> {
    host.setSection(0);
    host.setDrawerOpen(false);
    // Wait until the wallet metrics have fully loaded (all 10 zome calls in
    // fetchMetrics() complete) before starting Shepherd. The metrics grid
    // sets data-loaded="true" only after fetchMetrics() resolves, so polling
    // for that attribute guarantees the tour doesn't open on a screen of zeros.
    await waitForElement(TOUR_TARGETS.walletMetricsReady);

    return new Promise((resolve) => {
        const tour = new Shepherd.Tour({
            useModalOverlay: true,
            defaultStepOptions: {
                scrollTo: { behavior: 'smooth', block: 'center' },
                cancelIcon: { enabled: true },
                classes: 'edet-tour',
            },
            exitOnEsc: true,
            keyboardNavigation: true,
        });

        const back = { text: () => t('tour.back', 'Back'), action: () => tour.back() };
        const next = { text: () => t('tour.next', 'Next'), action: () => tour.next() };
        const done = { text: () => t('tour.done', 'Finish'), action: () => tour.complete() };

        type StepSpec = {
            id: string;
            titleKey: string;
            titleDefault: string;
            textKey: string;
            textDefault: string;
            target?: string;
            on?: 'top' | 'bottom' | 'left' | 'right';
            /** Prepare the UI so `target` is on-screen, then resolve. */
            prepare?: () => Promise<void>;
        };

        const steps: StepSpec[] = [
            {
                id: 'welcome',
                titleKey: 'tour.welcome.title',
                titleDefault: 'Welcome to edet',
                textKey: 'tour.welcome.text',
                textDefault:
                    "Let's take a quick look at your wallet, your trust network, and where to find settings.",
                prepare: async () => {
                    host.setSection(0);
                    host.setDrawerOpen(false);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
            {
                id: 'wallet-header',
                target: TOUR_TARGETS.walletHeader,
                on: 'bottom',
                titleKey: 'tour.walletHeader.title',
                titleDefault: 'Your wallet',
                textKey: 'tour.walletHeader.text',
                textDefault:
                    'This is your identity on edet. Your avatar and the long string below are how others find and verify you.',
                prepare: async () => {
                    host.setSection(0);
                    host.setDrawerOpen(false);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
            {
                id: 'wallet-metrics',
                target: TOUR_TARGETS.walletMetrics,
                on: 'top',
                titleKey: 'tour.walletMetrics.title',
                titleDefault: 'Your reputation at a glance',
                textKey: 'tour.walletMetrics.text',
                textDefault:
                    'These cards show your current debt, reputation, available capacity, and risk. Each card has a help icon with a plain-English explanation.',
                prepare: async () => {
                    host.setSection(0);
                    host.setDrawerOpen(false);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
            {
                id: 'wallet-qr',
                target: TOUR_TARGETS.walletQr,
                on: 'left',
                titleKey: 'tour.walletQr.title',
                titleDefault: 'Receive payments',
                textKey: 'tour.walletQr.text',
                textDefault:
                    'Share your address as a QR code or copy it to the clipboard. Sellers use this to record a sale against your account.',
                prepare: async () => {
                    host.setSection(0);
                    host.setDrawerOpen(false);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
            {
                id: 'drawer-transactions',
                target: TOUR_TARGETS.drawerTransactions,
                on: 'right',
                titleKey: 'tour.transactions.title',
                titleDefault: 'Your transactions',
                textKey: 'tour.transactions.text',
                textDefault:
                    'Every purchase and repayment shows up here in chronological order.',
                prepare: async () => {
                    host.setDrawerOpen(true);
                    host.setSection(1);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
            {
                id: 'drawer-contracts',
                target: TOUR_TARGETS.drawerContracts,
                on: 'right',
                titleKey: 'tour.contracts.title',
                titleDefault: 'Active contracts',
                textKey: 'tour.contracts.text',
                textDefault:
                    'Contracts track open debts. Trial contracts are your first transactions with strangers and help build reputation safely.',
                prepare: async () => {
                    host.setDrawerOpen(true);
                    host.setSection(2);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
            {
                id: 'drawer-support',
                target: TOUR_TARGETS.drawerSupport,
                on: 'right',
                titleKey: 'tour.support.title',
                titleDefault: 'Support and vouches',
                textKey: 'tour.support.text',
                textDefault:
                    'Distribute your reputation to people you trust, and review who has vouched for you. Vouches increase the credit capacity of your peers.',
                prepare: async () => {
                    host.setDrawerOpen(true);
                    host.setSection(3);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
            {
                id: 'drawer-settings',
                target: TOUR_TARGETS.drawerSettings,
                on: 'right',
                titleKey: 'tour.settings.title',
                titleDefault: 'Settings and recovery',
                textKey: 'tour.settings.text',
                textDefault:
                    'Change language, theme, number formats, and — most importantly — manage your backup file. You can replay this tour from here at any time.',
                prepare: async () => {
                    host.setDrawerOpen(true);
                    host.setSection(7);
                    await wait(NAVIGATION_DELAY_MS);
                },
            },
        ];

        steps.forEach((spec, idx) => {
            const isFirst = idx === 0;
            const isLast = idx === steps.length - 1;
            const buttons = [] as { text: () => string; action: () => void }[];
            if (!isFirst) buttons.push(back);
            buttons.push(isLast ? done : next);

            tour.addStep({
                id: spec.id,
                attachTo: spec.target ? { element: spec.target, on: spec.on ?? 'bottom' } : undefined,
                title: t(spec.titleKey, spec.titleDefault),
                text: t(spec.textKey, spec.textDefault),
                // Shepherd awaits `beforeShowPromise` before rendering the
                // step; use it to open the drawer / switch section so the
                // `attachTo` selector actually resolves.
                beforeShowPromise: spec.prepare,
                buttons: buttons.map((b) => ({ text: b.text(), action: b.action })),
            });
        });

        tour.on('complete', () => {
            host.setDrawerOpen(false);
            host.setSection(0);
            resolve();
        });
        tour.on('cancel', () => {
            host.setDrawerOpen(false);
            host.setSection(0);
            resolve();
        });
        tour.start();
    });
}
