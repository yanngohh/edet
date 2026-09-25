import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';

import {
    advance,
    beginOnboarding,
    completeOnboarding,
    leaveActorChoice,
    markEconomicsSeen,
    markKeyHeld,
    markLocaleConfigured,
    markNetworkChosen,
    markTourSeen,
    onboardingActive,
    onboardingIsReplay,
    onboardingStep,
    replayTour,
    resumeActorChoice,
} from '../onboardingStore';

const ALL = { needsLocaleSetup: true, needsNetwork: true, needsEconomics: true, needsKey: true, needsTour: true };

beforeEach(() => {
    completeOnboarding();
});

describe('first-run sequencing', () => {
    it('walks options → how it works → network → key → tour → app', () => {
        beginOnboarding(ALL);
        // Language first, so every screen after it — above all the explainer,
        // which is the longest prose in the app — is read in the language the
        // member CHOSE rather than the one the device reported.
        expect(get(onboardingStep)).toBe('locale-setup');
        markLocaleConfigured();
        advance();
        expect(get(onboardingStep)).toBe('economics-intro');
        markEconomicsSeen();
        advance();
        // The network BEFORE the key, and this assertion is the point of the
        // ordering: standing is earned on one ledger and cannot be carried to
        // another, so a member who makes a key first and picks a network
        // afterwards has nothing to bring with them.
        expect(get(onboardingStep)).toBe('network-choice');
        markNetworkChosen();
        advance();
        expect(get(onboardingStep)).toBe('actor-choice');
        // Making a key finishes that step — it is local, and the trade that
        // seats the account is waited for in the wallet.
        markKeyHeld();
        advance();
        // The tour comes AFTER, because its first three steps are the wallet
        // and a key is what puts one on screen.
        expect(get(onboardingStep)).toBe('tour');
        markTourSeen();
        advance();
        expect(get(onboardingStep)).toBe('complete');
    });

    it('skips steps whose needs flag is false', () => {
        beginOnboarding({ ...ALL, needsLocaleSetup: false, needsNetwork: false, needsEconomics: false, needsKey: false });
        expect(get(onboardingStep)).toBe('tour');
        markTourSeen();
        advance();
        expect(get(onboardingStep)).toBe('complete');
    });

    it('opens at the first outstanding step rather than always at the top', () => {
        beginOnboarding({ ...ALL, needsLocaleSetup: false });
        expect(get(onboardingStep)).toBe('economics-intro');
    });
});

describe('replayTour', () => {
    it('jumps straight to tour and flags the replay', () => {
        replayTour();
        expect(get(onboardingStep)).toBe('tour');
        expect(get(onboardingIsReplay)).toBe(true);
        completeOnboarding();
        expect(get(onboardingIsReplay)).toBe(false);
    });
});

describe('identity on demand', () => {
    it('opens the step alone and returns to the app either way', () => {
        resumeActorChoice();
        expect(get(onboardingStep)).toBe('actor-choice');
        expect(get(onboardingActive)).toBe(true);
        leaveActorChoice();
        expect(get(onboardingStep)).toBe('idle');
        expect(get(onboardingActive)).toBe(false);
    });

    it('does not walk a returning member back through the wizard', () => {
        // The bug `isReplay` exists for, one step over: after
        // `completeOnboarding()` every flag is at its reset value, so a naive
        // re-entry would advance into locale setup.
        completeOnboarding();
        resumeActorChoice();
        markKeyHeld();
        advance();
        expect(get(onboardingStep)).toBe('complete');
    });

    it('a device that already holds a key is not asked on first run', () => {
        beginOnboarding({ ...ALL, needsLocaleSetup: false, needsNetwork: false, needsEconomics: false, needsKey: false });
        expect(get(onboardingStep)).not.toBe('actor-choice');
    });
});

describe('a step may not end the wizard', () => {
    /**
     * `completeOnboarding()` dismisses the WHOLE wizard. A step that calls it
     * is fine while it happens to be last and silently truncates the sequence
     * the moment anything is added after it — which is exactly what happened:
     * moving the explainer ahead of identity left `EconomicsIntroStep` ending
     * onboarding on its own Continue, so the key step and the tour never ran
     * and the first run looked like it had no wizard at all.
     *
     * Steps `advance()`. The sequencer decides what is next, and `advance()`
     * lands on `complete`, which unmounts the overlay — so nothing is lost by
     * the rule and the ordering lives in exactly one place.
     */
    it('no onboarding step component calls completeOnboarding', () => {
        const dir = join(__dirname, '../../edet/onboarding');
        const offenders = readdirSync(dir)
            .filter((f) => f.endsWith('Step.svelte'))
            .filter((f) => readFileSync(join(dir, f), 'utf8').includes('completeOnboarding'));
        expect(offenders).toEqual([]);
    });
});
