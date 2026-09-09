/**
 * Cross-step onboarding state.
 *
 * The first-run wizard spans several components (locale setup, identity
 * choice, guided tour, economics explainer). This module owns the shared
 * step cursor so child components stay dumb and never prop-drill state.
 *
 * Steps, in order (each skipped when its `needs…` flag was false at
 * `beginOnboarding` time):
 *
 *   locale-setup     Pick language + date / time / number formats.
 *   economics-intro  Plain-language explainer of the debt/credit model.
 *   actor-choice     Make a key, or restore one from its recovery phrase.
 *   tour             Shepherd walkthrough of the wallet + drawer. The
 *                    wizard overlay unmounts for this step so the tour can
 *                    target the real app DOM (App.svelte owns the trigger).
 *
 * **The order is load-bearing at every join.** Language is picked first so
 * that everything after it is read in the language the member CHOSE rather
 * than the one the device reported — and the explainer is the longest and
 * most load-bearing prose in the app, so it is the screen that most needs to
 * be in the right one. The explainer then comes before the key, because being
 * asked to write down twelve words is a strange request from an app that has
 * not yet said what it is for. And the key comes before the TOUR, because the
 * tour's first three steps are the wallet, and a wallet is what a key gets
 * you — touring an empty state teaches nothing.
 *
 * **And making a key FINISHES that step.** It is local: twelve words and an
 * Ed25519 derivation, no network and nobody's permission. What needs another
 * member is the first TRADE, which is what seats the account, and that wait
 * belongs in the wallet (`PendingKeyWallet`) rather than across the way in.
 * `leaveActorChoice()` is the way past it for somebody who would rather look
 * around first; `resumeActorChoice()` is the way back to it from the app.
 */

import { writable, derived, type Readable } from 'svelte/store';

export type OnboardingStep =
    | 'idle'
    | 'locale-setup'
    | 'network-choice'
    | 'actor-choice'
    | 'tour'
    | 'economics-intro'
    | 'complete';

interface OnboardingState {
    step: OnboardingStep;
    localeConfigured: boolean;
    /** A network has been chosen for this device. */
    networkChosen: boolean;
    /** A key is held — seated member or pending — so identity is behind us. */
    keyHeld: boolean;
    tourSeen: boolean;
    economicsSeen: boolean;
    /**
     * Set by `replayTour()` (Settings → "Replay tour"). Onboarding has
     * already completed at that point, so `advance()`'s wizard-sequencing
     * logic would misfire (the other flags are back at their reset values)
     * and walk the user back into the wizard. App.svelte checks this flag
     * when the replayed tour ends and returns to the app via
     * `completeOnboarding()` instead of `advance()`.
     */
    isReplay: boolean;
}

const initial: OnboardingState = {
    step: 'idle',
    localeConfigured: false,
    networkChosen: false,
    keyHeld: false,
    tourSeen: false,
    economicsSeen: false,
    isReplay: false,
};

const store = writable<OnboardingState>(initial);

export const onboardingState: Readable<OnboardingState> = { subscribe: store.subscribe };

/** Convenience selector for components that only care about the step. */
export const onboardingStep: Readable<OnboardingStep> = derived(store, ($s) => $s.step);

/** Is the wizard active at all? */
export const onboardingActive: Readable<boolean> = derived(
    store,
    ($s) => $s.step !== 'idle' && $s.step !== 'complete',
);

/** Is the current `tour` step a Settings replay rather than first-run? */
export const onboardingIsReplay: Readable<boolean> = derived(store, ($s) => $s.isReplay);

export function beginOnboarding(params: {
    needsLocaleSetup: boolean;
    needsNetwork: boolean;
    needsEconomics: boolean;
    needsKey: boolean;
    needsTour: boolean;
}): void {
    // The opening step is `chooseNext` of the flags themselves rather than a
    // second ladder written out here: two orderings of the same sequence is
    // one too many, and they drift.
    const flags = {
        ...initial,
        localeConfigured: !params.needsLocaleSetup,
        networkChosen: !params.needsNetwork,
        economicsSeen: !params.needsEconomics,
        keyHeld: !params.needsKey,
        tourSeen: !params.needsTour,
    };
    store.set({ ...flags, step: chooseNext(flags) });
}

export function markLocaleConfigured(): void {
    store.update((s) => ({ ...s, localeConfigured: true }));
}

/** A network has been chosen. The network step calls this to move on. */
export function markNetworkChosen(): void {
    store.update((s) => ({ ...s, networkChosen: true }));
}

/**
 * Open identity setup on demand — the ONLY way this step is ever reached.
 *
 * Every other flag is marked done rather than left at its reset value, for
 * the reason `isReplay` exists: after `completeOnboarding()` the flags are
 * back at `initial`, so an `advance()` from here would read
 * `localeConfigured: false` and march a member who just created an identity
 * back through language setup.
 */
export function resumeActorChoice(): void {
    store.set({
        ...initial,
        step: 'actor-choice',
        localeConfigured: true,
        networkChosen: true,
        economicsSeen: true,
        keyHeld: false,
        tourSeen: true,
    });
}

/** A key now exists on this device. The identity step calls this to move on. */
export function markKeyHeld(): void {
    store.update((s) => ({ ...s, keyHeld: true }));
}

/**
 * Leave identity setup for the app, finished or not.
 *
 * One exit for both outcomes, because opening this step is not entering a
 * sequence: there is nothing to advance THROUGH. Either the visitor holds a
 * key now or they are still looking around, and both belong in the app.
 */
export function leaveActorChoice(): void {
    completeOnboarding();
}

export function markTourSeen(): void {
    store.update((s) => ({ ...s, tourSeen: true }));
}

export function markEconomicsSeen(): void {
    store.update((s) => ({ ...s, economicsSeen: true }));
}

export function goToStep(step: OnboardingStep): void {
    store.update((s) => ({ ...s, step }));
}

/**
 * Enter tour-ONLY replay mode (Settings → "Replay tour"). Jumps straight to
 * `tour` and flags `isReplay` so the tour-completion handler knows to call
 * `completeOnboarding()` rather than `advance()` once Shepherd finishes.
 */
export function replayTour(): void {
    store.update((s) => ({ ...s, step: 'tour', isReplay: true }));
}

/** Advance to the next logical step given what's been completed so far. */
export function advance(): void {
    store.update((s) => ({ ...s, step: chooseNext(s) }));
}

function chooseNext(s: OnboardingState): OnboardingStep {
    if (!s.localeConfigured) return 'locale-setup';
    if (!s.economicsSeen) return 'economics-intro';
    // BEFORE the key, and that ordering is the point. A key earns standing on
    // exactly one ledger; founding is irreversible and there is no merge, so a
    // device that trades first and picks a network afterwards has nothing to
    // carry over. Asked here, the choice is free.
    if (!s.networkChosen) return 'network-choice';
    if (!s.keyHeld) return 'actor-choice';
    if (!s.tourSeen) return 'tour';
    return 'complete';
}

/** Dismiss the wizard. */
export function completeOnboarding(): void {
    store.set({ ...initial, step: 'idle' });
}

/** Cancel without completing — same cleanup as complete. */
export function cancelOnboarding(): void {
    completeOnboarding();
}
