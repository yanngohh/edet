/**
 * Background mode, at the three places it can lie.
 *
 * The mode's whole claim is "your rule goes on deciding while you are not
 * looking", and three things have to be true for it to hold: it is armed
 * exactly when this device could actually sign, the poll it leaves running
 * behind the member is the background one, and a payment it signs reaches
 * them. Each of those is a place where a plausible implementation says
 * something it cannot do — a rule armed with the vault shut, a hidden page
 * still polling every 1.5 s, a signature nobody is told about — so each has a
 * probe here.
 *
 * What none of this can test is Android's `onPause`: whether the WebView
 * actually goes on running behind a foreground service is a claim about a
 * handset, and it is measured there (`just android-run`), not asserted here.
 */
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('../actors', async () => {
    const { writable } = await import('svelte/store');
    return {
        currentActorId: writable<number | null>(1),
        nicknames: writable<Record<number, string>>({}),
        holdsSeed: (id: number) => id === 1,
        heldSeed: () => new Uint8Array(32),
        setNickname: () => {},
    };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

vi.mock('@tauri-apps/plugin-notification', () => ({
    isPermissionGranted: vi.fn(async () => true),
    requestPermission: vi.fn(async () => 'granted'),
    sendNotification: vi.fn(),
    createChannel: vi.fn(async () => {}),
    Importance: { None: 0, Min: 1, Low: 2, Default: 3, High: 4 },
    Visibility: { Secret: -1, Private: 0, Public: 1 },
}));

// Partial: everything else in `node.ts` is the real module (the stores this
// file's dependencies read), and only the one call whose ARGUMENT is the
// claim under test is replaced.
vi.mock('../node', async (importOriginal) => {
    const actual = await importOriginal<typeof import('../node')>();
    return { ...actual, setPollInterval: vi.fn() };
});

import { addMessages, init } from 'svelte-i18n';

import { invoke } from '@tauri-apps/api/core';
import { sendNotification } from '@tauri-apps/plugin-notification';
import { CHANNEL_PAYMENTS, CHANNEL_WAITING } from '../background';
import { currentActorId } from '../actors';
import { setPollInterval } from '../node';
import { lockState } from '../../common/vault';
import { DEFAULT_POLICY, normalizePolicy, setPolicy } from '../policy';
import {
    BACKGROUND_INTERVAL_MS,
    FOREGROUND_INTERVAL_MS,
    armState,
    backgroundArmed,
    needsBatteryExemption,
    notifyIfAway,
    notifySigned,
    resetBackgroundForTests,
    startBackground,
    stopBackground,
} from '../background';

// The app initialises svelte-i18n before it mounts (`main.ts`), and every
// string this module puts in front of the member goes through it — the tray
// menu, the service notification, the payment summary. A suite that skipped
// this would be testing a state the client never has.
import en from '../../locales/en.json';
addMessages('en', en as any);
init({ fallbackLocale: 'en', initialLocale: 'en' });

const ON = { ...DEFAULT_POLICY, auto: true, background: true, maxAmount: 1_000 };

/**
 * Let the applier's promise chain run to its END.
 *
 * `vi.waitFor(() => expect(store).toBe(false))` on a store that STARTS false
 * is a probe that passes before the code under test has done anything —
 * exactly the vacuous assertion a mutation walks straight through. These
 * cases wait for the last thing the chain does instead.
 */
const settled = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

/** jsdom reports `visible`; the property is read-only, so it is redefined. */
function setVisibility(state: 'visible' | 'hidden'): void {
    Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
    document.dispatchEvent(new Event('visibilitychange'));
}

beforeEach(() => {
    vi.clearAllMocks();
    // `clearAllMocks` forgets the CALLS and keeps the implementations, so a
    // per-case `mockImplementation` would leak into every later case.
    vi.mocked(invoke).mockImplementation(async () => null);
    (window as any).__TAURI_INTERNALS__ = {};
    currentActorId.set(1);
    lockState.set('none');
    setPolicy(DEFAULT_POLICY);
    resetBackgroundForTests();
    setVisibility('visible');
});

afterEach(() => {
    stopBackground();
    resetBackgroundForTests();
    delete (window as any).__TAURI_INTERNALS__;
});

/**
 * **A stored policy that predates this field must not inherit "yes" from its
 * silence.** The same reading `auto` and `maxAmount` already get: absent or
 * malformed is the safe direction, which for a mode that spends the device's
 * battery and holds a permanent notification is OFF.
 */
describe('the flag', () => {
    it('is off unless it is literally true', () => {
        expect(normalizePolicy({}).background).toBe(false);
        expect(normalizePolicy(null).background).toBe(false);
        expect(normalizePolicy({ background: 'yes' } as any).background).toBe(false);
        expect(normalizePolicy({ background: 1 } as any).background).toBe(false);
        expect(normalizePolicy({ background: true }).background).toBe(true);
    });

    it('is off in the defaults', () => {
        expect(DEFAULT_POLICY.background).toBe(false);
    });
});

/**
 * **Armed exactly where a signature is possible.** Each of these is a state
 * in which the mode would otherwise hold a service, a notification and a
 * resumed WebView on behalf of a rule that cannot decide anything: the rule
 * off, no seed on this device, the vault shut. The last one is the one worth
 * naming — the seed is HERE and still unreachable, so the member is told.
 */
describe('what arms it', () => {
    it('arms only with the rule on, the seed here and the vault open', () => {
        expect(armState(ON, 1, 'none')).toBe('armed');
        expect(armState(ON, 1, 'unlocked')).toBe('armed');
        expect(armState(ON, 1, 'locked')).toBe('locked');
        expect(armState(ON, 9, 'none')).toBe('nokey');
        expect(armState(ON, null, 'none')).toBe('nokey');
        expect(armState({ ...ON, background: false }, 1, 'none')).toBe('off');
        // The mode has no meaning without the rule it keeps running.
        expect(armState({ ...ON, auto: false }, 1, 'none')).toBe('off');
    });

    it('starts and stops the platform side as the rule and the vault move', async () => {
        startBackground();
        setPolicy(ON);
        await vi.waitFor(() => expect(get(backgroundArmed)).toBe(true));
        expect(invoke).toHaveBeenCalledWith('background_arm', {
            labels: expect.objectContaining({ title: expect.any(String), quit: expect.any(String) }),
        });

        vi.mocked(invoke).mockClear();
        lockState.set('locked');
        await vi.waitFor(() => expect(get(backgroundArmed)).toBe(false));
        expect(invoke).toHaveBeenCalledWith('background_disarm');
    });

    /**
     * A refused service and a desktop with no system tray are the same fact:
     * this device will stop deciding when it is put down. The settings screen
     * reads this store, so it must be the answer the platform gave and not
     * the question the member asked.
     */
    it('reports not-armed when the platform refuses', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        // By COMMAND, not "the next call": the first thing this device does at
        // boot is assert the platform state it believes in, which is a disarm.
        vi.mocked(invoke).mockImplementation(async (cmd: string) => {
            if (cmd === 'background_arm') throw new Error('no system tray');
            return null;
        });
        startBackground();
        setPolicy(ON);
        await vi.waitFor(() => expect(warn).toHaveBeenCalled());
        expect(invoke).toHaveBeenCalledWith('background_arm', expect.anything());
        expect(get(backgroundArmed)).toBe(false);
        warn.mockRestore();
    });

    /** A browser tab has no service and no tray: nothing to arm, nothing claimed. */
    it('claims nothing outside the app', async () => {
        delete (window as any).__TAURI_INTERNALS__;
        startBackground();
        setPolicy(ON);
        await settled();
        expect(invoke).not.toHaveBeenCalled();
        expect(get(backgroundArmed)).toBe(false);
    });

    /**
     * The rule stopping because the vault shut is the one failure this mode
     * could hide: the member is not looking at the app, which is the whole
     * premise. Turning the mode OFF is not that — they did it themselves.
     */
    it('says so when the vault shuts under an armed rule, and not when the member turns it off', async () => {
        startBackground();
        setPolicy(ON);
        await vi.waitFor(() => expect(get(backgroundArmed)).toBe(true));

        vi.mocked(sendNotification).mockClear();
        lockState.set('locked');
        await vi.waitFor(() => expect(sendNotification).toHaveBeenCalledOnce());

        lockState.set('unlocked');
        await vi.waitFor(() => expect(get(backgroundArmed)).toBe(true));
        vi.mocked(sendNotification).mockClear();
        setPolicy({ ...ON, background: false });
        await vi.waitFor(() => expect(get(backgroundArmed)).toBe(false));
        expect(sendNotification).not.toHaveBeenCalled();
    });
});

/**
 * **The cadence follows who is watching, not what is armed.** A hidden page
 * has nobody reading it, so the only thing 1.5 s buys there is battery spent
 * on a screen that is off; coming back to the front polls at once, because
 * the first thing a member does on returning is look.
 */
describe('the poll behind the member', () => {
    it('slows while hidden and speeds up on return', () => {
        startBackground();
        vi.mocked(setPollInterval).mockClear();

        setVisibility('hidden');
        expect(setPollInterval).toHaveBeenLastCalledWith(BACKGROUND_INTERVAL_MS);

        setVisibility('visible');
        expect(setPollInterval).toHaveBeenLastCalledWith(FOREGROUND_INTERVAL_MS);
    });

    it('leaves a poll that has not started alone', () => {
        // `startBackground` is called after `startPolling` in `App.svelte`,
        // and `setPollInterval` is a no-op with no timer — so a visibility
        // event before boot cannot start one. Stated here because the guard
        // lives in `node.ts` and this is the caller that depends on it.
        stopBackground();
        vi.mocked(setPollInterval).mockClear();
        setVisibility('hidden');
        expect(setPollInterval).not.toHaveBeenCalled();
    });
});

/**
 * **A notification is for a member who is not looking.** With the app in
 * front the Requests page is already showing the same line; behind it, this
 * is the only thing that says a purchase was signed in their name.
 */
describe('what it tells the member', () => {
    const SALE = { Sale: { seller: { Member: 1 }, buyer: { Member: 2 }, amount: 30, maturity_epochs: 30 } };

    it('notifies while hidden, with a body the member can open', async () => {
        setVisibility('hidden');
        await notifySigned(SALE);
        expect(sendNotification).toHaveBeenCalledOnce();
        const [sent] = vi.mocked(sendNotification).mock.calls[0] as [{ body: string; largeBody?: string }];
        // The summary the Requests page would show, not a bare "something
        // happened": who bought, and for how much.
        expect(sent.body).toContain('30');
        // **And `largeBody`, or Android truncates it to one line with nothing
        // to expand.** The plugin sets `BigTextStyle` only where that field is
        // present, and what gets cut off is the whole content of the message.
        expect(sent.largeBody).toBe(sent.body);
    });

    /**
     * **Two channels, or a member can only mute both.** A receipt for a
     * purchase the rule signed is frequent and never urgent; something waiting
     * on a person is neither. Android hands the importance of a channel to the
     * member, so the two have to BE two — and the one they would silence is
     * the frequent one, which on a single channel takes the other with it.
     */
    it('puts a receipt and a thing waiting on different channels', async () => {
        setVisibility('hidden');
        await notifySigned(SALE);
        const [signed] = vi.mocked(sendNotification).mock.calls[0] as [{ channelId?: string }];
        expect(signed.channelId).toBe(CHANNEL_PAYMENTS);

        vi.mocked(sendNotification).mockClear();
        await notifyIfAway('Waiting for you', 'somebody needs a signature');
        const [waiting] = vi.mocked(sendNotification).mock.calls[0] as [{ channelId?: string }];
        expect(waiting.channelId).toBe(CHANNEL_WAITING);
        expect(CHANNEL_PAYMENTS).not.toBe(CHANNEL_WAITING);
    });

    it('says nothing while the member is looking at it', async () => {
        setVisibility('visible');
        await notifySigned(SALE);
        expect(sendNotification).not.toHaveBeenCalled();
    });

    it('says nothing outside the app, where there is nowhere to post it', async () => {
        delete (window as any).__TAURI_INTERNALS__;
        setVisibility('hidden');
        await notifySigned(SALE);
        expect(sendNotification).not.toHaveBeenCalled();
    });

    it('says nothing when the member has refused the permission', async () => {
        vi.mocked(await import('@tauri-apps/plugin-notification')).isPermissionGranted.mockResolvedValueOnce(false);
        setVisibility('hidden');
        await notifySigned(SALE);
        expect(sendNotification).not.toHaveBeenCalled();
    });
});

/**
 * **The Doze briefing fires only where there is something left to grant.**
 *
 * It is shown once, on the transition from off to on, and then it sends the
 * member to an OS screen — so asking a member who has already exempted edet to
 * go and exempt it is not a small annoyance: it is the prompt teaching them
 * that this app's prompts are noise, on the one screen the mode genuinely
 * needs them to read.
 *
 * The unreadable case leans the other way on purpose. A device that will not
 * say is not a device that has granted it, and the two costs are not
 * comparable: one redundant screen against a wallet that stops deciding when
 * it is put down, with nothing said.
 */
describe('the Doze briefing', () => {
    /** Android is read off the user agent, exactly as custody reads it. */
    function pretendAndroid(on: boolean): void {
        Object.defineProperty(navigator, 'userAgent', {
            value: on ? 'Mozilla/5.0 (Linux; Android 15; K) AppleWebKit/537.36' : 'Mozilla/5.0 (X11; Linux x86_64)',
            configurable: true,
        });
    }

    it('is needed while the app is still optimised, and not once it is exempt', async () => {
        pretendAndroid(true);
        vi.mocked(invoke).mockImplementation(async (cmd: string) =>
            cmd === 'background_battery_optimised' ? true : null,
        );
        expect(await needsBatteryExemption()).toBe(true);

        vi.mocked(invoke).mockImplementation(async (cmd: string) =>
            cmd === 'background_battery_optimised' ? false : null,
        );
        expect(await needsBatteryExemption()).toBe(false);
    });

    it('is not needed off Android, where there is no such setting', async () => {
        pretendAndroid(false);
        expect(await needsBatteryExemption()).toBe(false);
        expect(invoke).not.toHaveBeenCalled();
    });

    it('is needed when the device will not say', async () => {
        const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
        pretendAndroid(true);
        vi.mocked(invoke).mockRejectedValue(new Error('no such command'));
        expect(await needsBatteryExemption()).toBe(true);
        warn.mockRestore();
    });
});
