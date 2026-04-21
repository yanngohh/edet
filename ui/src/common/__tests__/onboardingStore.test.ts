import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';

import {
    advance,
    beginOnboarding,
    cancelOnboarding,
    choosePath,
    clearPathChoice,
    completeOnboarding,
    goToStep,
    markBackupSaved,
    markLocaleConfigured,
    markMnemonicConfirmed,
    markRestoreCompleted,
    markTourSeen,
    onboardingActive,
    onboardingState,
    onboardingStep,
    setMnemonic,
} from '../onboardingStore';

function makeMockDerived() {
    return {
        agentSeed: new Uint8Array(32).fill(1),
        backupKey: new Uint8Array(32).fill(2),
        fingerprint: new Uint8Array(4).fill(3),
    };
}

describe('onboardingStore', () => {
    beforeEach(() => {
        completeOnboarding();
    });

    it('begins on `locale-setup` when the user needs everything', () => {
        beginOnboarding({ needsLocaleSetup: true, needsMnemonic: true, needsTour: true });
        expect(get(onboardingStep)).toBe('locale-setup');
        expect(get(onboardingActive)).toBe(true);
    });

    it('jumps to path-choice when locale is already configured', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: true, needsTour: true });
        expect(get(onboardingStep)).toBe('path-choice');
    });

    it('skips straight to the tour when only the tour is missing', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: false, needsTour: true });
        expect(get(onboardingStep)).toBe('tour');
    });

    it('shortcuts to `complete` when nothing is needed', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: false, needsTour: false });
        expect(get(onboardingStep)).toBe('complete');
    });

    it('full create-path: locale → path-choice → mnemonic flow → backup → tour', () => {
        beginOnboarding({ needsLocaleSetup: true, needsMnemonic: true, needsTour: true });
        expect(get(onboardingStep)).toBe('locale-setup');

        markLocaleConfigured();
        advance();
        expect(get(onboardingStep)).toBe('path-choice');

        choosePath('create');
        advance();
        expect(get(onboardingStep)).toBe('mnemonic-generate');

        setMnemonic('abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about', makeMockDerived());
        advance();
        expect(get(onboardingStep)).toBe('mnemonic-confirm');

        markMnemonicConfirmed();
        advance();
        expect(get(onboardingStep)).toBe('backup-export');

        markBackupSaved();
        advance();
        expect(get(onboardingStep)).toBe('tour');

        markTourSeen();
        advance();
        expect(get(onboardingStep)).toBe('complete');
    });

    it('restore-path: locale → path-choice → restore-backup → tour', () => {
        beginOnboarding({ needsLocaleSetup: true, needsMnemonic: true, needsTour: true });

        markLocaleConfigured();
        advance();
        expect(get(onboardingStep)).toBe('path-choice');

        choosePath('restore');
        advance();
        expect(get(onboardingStep)).toBe('restore-backup');

        // Successful restore fast-forwards the mnemonic + backup flags.
        markRestoreCompleted();
        advance();
        expect(get(onboardingStep)).toBe('tour');

        markTourSeen();
        advance();
        expect(get(onboardingStep)).toBe('complete');
    });

    it('cancelling out of restore returns to path-choice', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: true, needsTour: true });
        choosePath('restore');
        advance();
        expect(get(onboardingStep)).toBe('restore-backup');

        // User hits "Cancel" — the RestoreBackupStep wrapper clears the
        // path choice and advances; the wizard falls back to path-choice.
        clearPathChoice();
        advance();
        expect(get(onboardingStep)).toBe('path-choice');
    });

    it('regenerating the mnemonic resets the confirm gate', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: true, needsTour: false });
        choosePath('create');
        setMnemonic('abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about', makeMockDerived());
        markMnemonicConfirmed();
        expect(get(onboardingState).mnemonicConfirmed).toBe(true);
        setMnemonic('abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art', makeMockDerived());
        expect(get(onboardingState).mnemonicConfirmed).toBe(false);
    });

    it('goToStep can force a direct transition (used by Settings "Replay tour")', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: false, needsTour: false });
        goToStep('tour');
        expect(get(onboardingStep)).toBe('tour');
    });

    it('cancelOnboarding zeroes the derived keys', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: true, needsTour: false });
        choosePath('create');
        const derived = makeMockDerived();
        setMnemonic('some mnemonic', derived);
        cancelOnboarding();
        expect(get(onboardingStep)).toBe('idle');
        expect([...derived.agentSeed].every((b) => b === 0)).toBe(true);
        expect([...derived.backupKey].every((b) => b === 0)).toBe(true);
        expect([...derived.fingerprint].every((b) => b === 0)).toBe(true);
    });

    it('stores the mnemonic only in memory (state.mnemonic, not localStorage)', () => {
        beginOnboarding({ needsLocaleSetup: false, needsMnemonic: true, needsTour: false });
        choosePath('create');
        const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
        setMnemonic(mnemonic, makeMockDerived());
        const st = get(onboardingState);
        expect(st.mnemonic).toBe(mnemonic);
        if (typeof localStorage !== 'undefined') {
            for (let i = 0; i < localStorage.length; i++) {
                const key = localStorage.key(i);
                if (!key) continue;
                const val = localStorage.getItem(key) ?? '';
                expect(val).not.toContain(mnemonic);
            }
        }
    });

    it('onboardingActive is false when idle and true mid-flow', () => {
        expect(get(onboardingActive)).toBe(false);
        beginOnboarding({ needsLocaleSetup: true, needsMnemonic: false, needsTour: false });
        expect(get(onboardingActive)).toBe(true);
        completeOnboarding();
        expect(get(onboardingActive)).toBe(false);
    });
});
