/**
 * Cross-step onboarding state.
 *
 * The onboarding wizard spans several Svelte components (locale setup,
 * choose-your-path, mnemonic generation, mnemonic confirmation, backup
 * export / restore, guided tour). This module owns the ephemeral shared
 * state so child components can remain dumb and don't have to prop-drill
 * the mnemonic, derived keys, or step cursor between each other.
 *
 * Security note: the mnemonic and derived keys live here in memory only.
 * They are cleared on `completeOnboarding()` and on navigation away from
 * the wizard so they don't linger across page reloads or component
 * remounts.
 */

import { writable, derived, type Readable } from 'svelte/store';

import type { DerivedKeys, Mnemonic } from './mnemonic';
import { zero } from './mnemonic';

/**
 * Discrete onboarding states.
 *
 * The order matters: the wizard walks these left-to-right, skipping any
 * step whose corresponding `needs…` flag was false at `beginOnboarding`
 * time. `chooseNext` computes the next applicable state given the
 * current flags, so jumping steps (e.g. the "Replay tour" button in
 * Settings) just sets the cursor directly with `goToStep`.
 *
 *   locale-setup     Pick language + date / time / number formats.
 *   path-choice      "Create new identity" vs. "Restore from backup".
 *   mnemonic-generate / mnemonic-confirm / backup-export
 *                    The create-new flow: generate, retype, save.
 *   restore-backup   The restore flow: supply phrase + backup file.
 *   tour             Guided tour of the wallet + drawer.
 */
export type OnboardingStep =
    | 'idle'
    | 'locale-setup'
    | 'path-choice'
    | 'mnemonic-generate'
    | 'mnemonic-confirm'
    | 'backup-export'
    | 'restore-backup'
    | 'tour'
    | 'complete';

/** User-chosen onboarding path, decided at the `path-choice` step. */
export type OnboardingPath = 'create' | 'restore';

interface OnboardingState {
    step: OnboardingStep;
    mnemonic: Mnemonic | null;
    derived: DerivedKeys | null;
    /** Set once the user has picked their language + date/time/number format. */
    localeConfigured: boolean;
    /** `null` until the user chooses between Create-new and Restore-from-backup. */
    pathChoice: OnboardingPath | null;
    /** Set once the user has retyped the challenge words successfully. */
    mnemonicConfirmed: boolean;
    backupSaved: boolean;
    tourSeen: boolean;
}

const initial: OnboardingState = {
    step: 'idle',
    mnemonic: null,
    derived: null,
    localeConfigured: false,
    pathChoice: null,
    mnemonicConfirmed: false,
    backupSaved: false,
    tourSeen: false,
};

const store = writable<OnboardingState>(initial);

export const onboardingState: Readable<OnboardingState> = { subscribe: store.subscribe };

/** Convenience selector for components that only care about the current step. */
export const onboardingStep: Readable<OnboardingStep> = derived(store, ($s) => $s.step);

/** Convenience selector: is the wizard active at all? */
export const onboardingActive: Readable<boolean> = derived(
    store,
    ($s) => $s.step !== 'idle' && $s.step !== 'complete',
);

export function beginOnboarding(params: {
    needsLocaleSetup: boolean;
    needsMnemonic: boolean;
    needsTour: boolean;
}): void {
    // Pick the first applicable step. Locale comes before path-choice so
    // the "Create / Restore" prompt is rendered in the user's language.
    const step: OnboardingStep = params.needsLocaleSetup
        ? 'locale-setup'
        : params.needsMnemonic
          ? 'path-choice'
          : params.needsTour
            ? 'tour'
            : 'complete';
    store.set({
        ...initial,
        step,
        localeConfigured: !params.needsLocaleSetup,
        // Skipping the mnemonic flow (returning user with a backup already
        // saved) implies the path is irrelevant; treat it as Create so
        // chooseNext doesn't bounce back to path-choice on reconnect.
        pathChoice: params.needsMnemonic ? null : 'create',
        mnemonicConfirmed: !params.needsMnemonic,
        backupSaved: !params.needsMnemonic,
        tourSeen: !params.needsTour,
    });
}

export function markLocaleConfigured(): void {
    store.update((s) => ({ ...s, localeConfigured: true }));
}

/**
 * Called from PathChoiceStep when the user picks one of the two branches.
 * Subsequent `advance()` calls route through the correct path.
 */
export function choosePath(kind: OnboardingPath): void {
    store.update((s) => ({ ...s, pathChoice: kind }));
}

/** Undo a previous `choosePath` so the wizard falls back to the path-choice
 *  screen. Invoked when the user cancels out of the Restore flow. */
export function clearPathChoice(): void {
    store.update((s) => ({ ...s, pathChoice: null }));
}

export function setMnemonic(mnemonic: Mnemonic, derivedKeys: DerivedKeys): void {
    store.update((s) => {
        // Zero any previous derived keys before replacing them.
        if (s.derived) {
            zero(s.derived.agentSeed);
            zero(s.derived.backupKey);
            zero(s.derived.fingerprint);
        }
        return { ...s, mnemonic, derived: derivedKeys, mnemonicConfirmed: false };
    });
}

/** Called from MnemonicConfirm.svelte after the user correctly retypes the
 *  challenge words. Unblocks the backup-export step. */
export function markMnemonicConfirmed(): void {
    store.update((s) => ({ ...s, mnemonicConfirmed: true }));
}

export function markBackupSaved(): void {
    store.update((s) => ({ ...s, backupSaved: true }));
}

/**
 * Called from the RestoreBackup step after a successful restore. A restored
 * identity already has a persisted backup bundle AND an already-known
 * mnemonic, so we fast-forward both flags; `advance()` then jumps straight
 * to the tour.
 */
export function markRestoreCompleted(): void {
    store.update((s) => ({
        ...s,
        mnemonicConfirmed: true,
        backupSaved: true,
    }));
}

export function markTourSeen(): void {
    store.update((s) => ({ ...s, tourSeen: true }));
}

export function goToStep(step: OnboardingStep): void {
    store.update((s) => ({ ...s, step }));
}

/** Advance to the next logical step given what's been completed so far. */
export function advance(): void {
    store.update((s) => {
        const next = chooseNext(s);
        return { ...s, step: next };
    });
}

function chooseNext(s: OnboardingState): OnboardingStep {
    if (!s.localeConfigured) return 'locale-setup';
    if (s.pathChoice === null) return 'path-choice';
    if (s.pathChoice === 'restore') {
        // Restore path: if the restore hasn't produced a backup yet, keep
        // the user on the RestoreBackup step; once `markRestoreCompleted`
        // has fired we jump to the tour like the create path.
        if (!s.backupSaved) return 'restore-backup';
        if (!s.tourSeen) return 'tour';
        return 'complete';
    }
    // create path
    if (!s.mnemonic) return 'mnemonic-generate';
    if (!s.mnemonicConfirmed) return 'mnemonic-confirm';
    if (!s.backupSaved) return 'backup-export';
    if (!s.tourSeen) return 'tour';
    return 'complete';
}

/**
 * Mark onboarding as complete. Clears mnemonic + derived keys from memory
 * and flips the step to `idle`, dismissing the wizard.
 */
export function completeOnboarding(): void {
    store.update((s) => {
        if (s.derived) {
            zero(s.derived.agentSeed);
            zero(s.derived.backupKey);
            zero(s.derived.fingerprint);
        }
        return { ...initial, step: 'idle' };
    });
}

/** Cancel onboarding without completing it. Same cleanup as complete but the
 *  UI may surface a warning that recovery is not yet configured. */
export function cancelOnboarding(): void {
    completeOnboarding();
}
