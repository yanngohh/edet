/**
 * Shepherd-powered walkthrough for first-run users.
 *
 * Every step declares (a) the CSS selector its popup should point at and
 * (b) a `prepare` hook that opens the drawer and navigates to the right
 * section before the popup renders. Without the prepare hook the drawer
 * items would be invisible and Shepherd would center each popover in the
 * middle of the screen.
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

/**
 * A beat with the target on screen and NOTHING covering it, before the popup
 * that talks about it opens.
 *
 * On a phone the popup is most of the width and lands next to — sometimes
 * over — what it points at, so a reader who arrives with both at once has no
 * moment where they can see which component the words are about. Scrolling
 * the target to the middle of the screen and holding it there for a beat
 * gives them that moment. It costs a second per step and buys the only thing
 * a tour is for.
 */
const REVEAL_DELAY_MS = 900;

/** Wait for a CSS selector to appear in the DOM, or for an attribute on an
 *  existing element to satisfy the selector (e.g. [data-loaded="true"]).
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
    walletQr: '[data-tour="wallet-qr"]',
    walletMetrics: '[data-tour="wallet-metrics"]',
    walletMetricsReady: '[data-tour="wallet-metrics"][data-loaded="true"]',
    drawerWallet: '[data-tour="drawer-wallet"]',
    drawerTransactions: '[data-tour="drawer-transactions"]',
    drawerContracts: '[data-tour="drawer-contracts"]',
    drawerReceivables: '[data-tour="drawer-receivables"]',
    drawerCommunity: '[data-tour="drawer-community"]',
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
    /**
     * Is a WALLET on screen — seated member or pending key?
     *
     * Three of the steps below point at wallet elements, and `PendingKeyWallet`
     * carries the same anchors, so a key made moments ago is enough. What is
     * not enough is no key at all (somebody who skipped identity setup): the
     * wallet is then a two-line notice, and Shepherd does not fail on a
     * selector that resolves to nothing — it centres the popup and shows the
     * copy anyway, which is worse, because that copy describes a screen the
     * reader is not looking at. Those three are dropped instead.
     */
    hasWallet(): boolean;
}

function wait(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Put `selector` in the middle of the screen and leave it there, uncovered,
 * for `REVEAL_DELAY_MS`.
 *
 * Centring is what keeps the popup off it: Shepherd places the popup against
 * the target's edge, so a target sitting at the top or bottom of the viewport
 * leaves no room on that side and the popup is pushed over it. From the
 * middle there is room either way. A selector that never resolves times out
 * inside `waitForElement` and the step opens anyway rather than hanging.
 */
async function revealTarget(selector: string): Promise<void> {
    await waitForElement(selector);
    document.querySelector(selector)?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    await wait(REVEAL_DELAY_MS);
}

/**
 * Build and run the tour. Returns a promise that resolves when the user
 * completes or cancels it. Safe to call multiple times.
 */
export async function runTour(host: TourHost): Promise<void> {
    host.setSection(0);
    host.setDrawerOpen(false);
    // Wait until the wallet metrics have loaded before starting Shepherd —
    // the grid sets data-loaded="true" once the member view has arrived, so
    // the tour never opens on a screen of placeholders.
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

        const atWallet = async () => {
            host.setSection(0);
            host.setDrawerOpen(false);
            await wait(NAVIGATION_DELAY_MS);
        };
        const atDrawer = (section: number) => async () => {
            host.setDrawerOpen(true);
            host.setSection(section);
            await wait(NAVIGATION_DELAY_MS);
        };

        const steps: StepSpec[] = [
            {
                id: 'welcome',
                titleKey: 'tour.welcome.title',
                titleDefault: 'Welcome to edet',
                textKey: 'tour.welcome.text',
                textDefault:
                    "Let's take a quick look at your wallet, your community, and where to find settings.",
                prepare: atWallet,
            },
            {
                id: 'wallet-header',
                target: TOUR_TARGETS.walletHeader,
                on: 'bottom',
                titleKey: 'tour.walletHeader.title',
                titleDefault: 'Your identity',
                textKey: 'tour.walletHeader.text',
                textDefault:
                    'This is who you are on the community ledger: your avatar, your local display name, and your membership status.',
                prepare: atWallet,
            },
            {
                id: 'wallet-qr',
                target: TOUR_TARGETS.walletQr,
                on: 'left',
                titleKey: 'tour.walletQr.title',
                titleDefault: 'Get paid',
                textKey: 'tour.walletQr.text',
                textDefault:
                    'Selling something? The buyer scans this QR (or types your address) to record the purchase — the debt they take from you IS the payment.',
                prepare: atWallet,
            },
            {
                id: 'wallet-metrics',
                target: TOUR_TARGETS.walletMetrics,
                on: 'top',
                titleKey: 'tour.walletMetrics.title',
                titleDefault: 'Your standing at a glance',
                textKey: 'tour.walletMetrics.text',
                textDefault:
                    'These cards show what the community backs you for, what you owe, and who is backing you. Each card has a help icon with a plain-English explanation.',
                prepare: atWallet,
            },
            {
                id: 'drawer-transactions',
                target: TOUR_TARGETS.drawerTransactions,
                on: 'right',
                titleKey: 'tour.transactions.title',
                titleDefault: 'Record a transaction',
                textKey: 'tour.transactions.text',
                textDefault:
                    'Record a purchase here. It first clears what the seller already owes — to you, then through their support circle — and only the remainder becomes your new debt.',
                prepare: atDrawer(1),
            },
            {
                id: 'drawer-contracts',
                target: TOUR_TARGETS.drawerContracts,
                on: 'right',
                titleKey: 'tour.contracts.title',
                titleDefault: 'Debts you owe',
                textKey: 'tour.contracts.text',
                textDefault:
                    'Contracts track your open debts and their maturity. They clear as you trade — sell to your creditor and the mutual debt settles by itself. The only thing left to file is a maturity extension.',
                prepare: atDrawer(2),
            },
            {
                id: 'drawer-receivables',
                target: TOUR_TARGETS.drawerReceivables,
                on: 'right',
                titleKey: 'tour.receivables.title',
                titleDefault: 'Owed to you',
                textKey: 'tour.receivables.text',
                textDefault:
                    'The mirror view: contracts where you are the creditor, with the actions a creditor has — including marking an overdue debt as defaulted.',
                prepare: atDrawer(3),
            },
            {
                id: 'drawer-community',
                target: TOUR_TARGETS.drawerCommunity,
                on: 'right',
                titleKey: 'tour.community.title',
                titleDefault: 'Your community',
                textKey: 'tour.community.text',
                textDefault:
                    'Every member of the ledger, and what the community has put behind each of them. Standing is not granted here and cannot be bought — it is the record of debts already carried and paid.',
                prepare: atDrawer(4),
            },
            {
                id: 'drawer-support',
                target: TOUR_TARGETS.drawerSupport,
                on: 'right',
                titleKey: 'tour.support.title',
                titleDefault: 'Support circle',
                textKey: 'tour.support.text',
                textDefault:
                    'Choose whose debts your sales help clear, and approve the supporters who list you. This is the cascade that makes selling clear debt.',
                prepare: atDrawer(5),
            },
            {
                id: 'drawer-settings',
                target: TOUR_TARGETS.drawerSettings,
                on: 'right',
                titleKey: 'tour.settings.title',
                titleDefault: 'Settings and recovery',
                textKey: 'tour.settings.text',
                textDefault:
                    'Language, theme, number formats, node connection, and account recovery (guardians). You can replay this tour from here at any time.',
                prepare: atDrawer(10),
            },
        ];

        // Steps whose target only exists where a wallet renders. `welcome`
        // and every drawer step stand on their own: the drawer is the same
        // drawer, and what it leads to is worth seeing either way.
        const walletOnly = new Set(['wallet-header', 'wallet-qr', 'wallet-metrics']);
        const shown = host.hasWallet() ? steps : steps.filter((s) => !walletOnly.has(s.id));

        shown.forEach((spec, idx) => {
            const isFirst = idx === 0;
            const isLast = idx === shown.length - 1;
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
                // Navigate, then show the target alone for a beat, then the
                // popup. Shepherd awaits this before rendering the step.
                beforeShowPromise: async () => {
                    await spec.prepare?.();
                    if (spec.target) await revealTarget(spec.target);
                },
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
