<script lang="ts">
  /**
   * Step 2 of onboarding: establish who this device acts as. Two paths, both
   * production-shaped:
   *
   *   create   Generate a 12-word recovery phrase (shown once, never
   *            stored) and derive the Ed25519 identity key. The public key
   *            is handed — out of band — to an existing member, who records
   *            a trade with it. This screen then shows that trade for review,
   *            takes this device's signature on it, and becomes the member
   *            the ledger seats.
   *   restore  Re-enter a recovery phrase: the key is re-derived and the
   *            ledger is asked which membership it belongs to.
   *
   * **There is no third path, and there is no admission.**
   * One offered to self-admit into an "open trial tier"; it was gated on a
   * params field the node has never served, so it was unreachable, and the
   * transition behind it is gone — creation was unbillable, so it had to be
   * bounded by a ledger-wide per-epoch counter, which is a censorship lever
   * rather than a quota. An account is seated by the first bonded trade that
   * names its key: nobody approves it, and the counterparty who chose to trade
   * carries the bond for the row, which is the same first risk §Recourse already
   * names.
   *
   * Dev-harness founders onboard the same way: the launcher prints their
   * recovery phrases and each window restores one — no in-app shortcut.
   */
  import { _ } from 'svelte-i18n';
  import { get } from 'svelte/store';
  import Button, { Label } from '@smui/button';
  import Textfield from '@smui/textfield';
  import CircularProgress from '@smui/circular-progress';

  import { firstLoadDone, activeBase, refreshSoon } from '../../lib/node';
  import { chooseActor, rememberSeed, setNickname, setPendingSeed } from '../../lib/actors';
  import { keyProof } from '../../lib/session';
  import * as api from '../../lib/api';
  import {
    bytesToHex,
    derivePublicKey,
    generatePhrase,
    isValidPhrase,
    seedFromPhrase,
    zero,
  } from '../../lib/crypto';
  import { advance, leaveActorChoice, markKeyHeld } from '../../common/onboardingStore';
  import { errorStore } from '../../common/errorStore';

  type Mode = 'menu' | 'create' | 'restore';
  type CreatePhase = 'words' | 'confirm';

  let mode: Mode = 'menu';
  // --- create path -------------------------------------------------------
  let phrase = '';
  let createPhase: CreatePhase = 'words';
  let confirmIndex = 0;
  let confirmInput = '';
  let nickname = '';

  // --- restore path ------------------------------------------------------
  let restoreInput = '';
  let restoring = false;

  function enterCreate() {
    phrase = generatePhrase();
    createPhase = 'words';
    confirmInput = '';
    mode = 'create';
  }

  function toConfirm() {
    confirmIndex = Math.floor(Math.random() * 12);
    confirmInput = '';
    createPhase = 'confirm';
  }

  function checkConfirm() {
    const words = phrase.split(' ');
    if (confirmInput.trim().toLowerCase() === words[confirmIndex]) {
      // Seed enters the vault's pending slot; the phrase leaves memory.
      const seed = seedFromPhrase(phrase);
      setPendingSeed(seed);
      phrase = '';
      zero(seed);
      // **The key exists, so this step is done.** Everything after it —
      // handing the key over, the trade coming back to be signed — needs
      // another member and happens in `PendingKeyWallet`, where the app is
      // usable while it is waited for. Keeping it here was the wall.
      markKeyHeld();
      advance();
      refreshSoon();
    } else {
      errorStore.pushError(
        $_('onboarding.identity.confirmError', { default: "That's not the right word — check your written phrase." }),
      );
    }
  }

  function finish(memberId: number) {
    if (nickname.trim()) setNickname(memberId, nickname);
    chooseActor(memberId);
    markKeyHeld();
    advance();
    refreshSoon();
  }

  /**
   * Back to the app without an identity.
   *
   * The way past identity setup for somebody who would rather see the app
   * first. Making a key is the ordinary path now and it finishes here — what
   * needs another member is the trade, and that is waited for in the wallet.
   */
  function lookAround() {
    leaveActorChoice();
  }

  /** Re-derive the key from the phrase and ask the ledger who it is. */
  async function restore() {
    if (!isValidPhrase(restoreInput)) {
      errorStore.pushError($_('onboarding.identity.restoreInvalid', { default: 'That is not a valid recovery phrase.' }));
      return;
    }
    restoring = true;
    try {
      const seed = seedFromPhrase(restoreInput);
      const pubkey = derivePublicKey(seed);
      const keyHex = bytesToHex(pubkey);
      // Proves possession of the very key being looked up, which is what
      // makes this a self-resolve rather than a scan of someone else's
      // identity — the only lookup the node allows without a session.
      const who = await api.whois(get(activeBase), keyHex, keyProof(pubkey, seed, 'GET', `/whois/${keyHex}`));
      if (who.member === null || who.member === undefined) {
        zero(seed);
        errorStore.pushError(
          $_('onboarding.identity.restoreNotFound', { default: 'No member on this ledger holds this key.' }),
        );
        return;
      }
      rememberSeed(who.member, seed);
      zero(seed);
      restoreInput = '';
      finish(who.member);
    } finally {
      restoring = false;
    }
  }
</script>

<div class="step">
  <h2 class="step-title">
    {$_('onboarding.actorChoice.title', { default: 'Who are you in this community?' })}
  </h2>

  {#if !$firstLoadDone}
    <div class="center-container" style="min-height: 20vh;">
      <CircularProgress class="circular-progress" indeterminate />
      <p class="step-body">{$_('onboarding.actorChoice.connecting', { default: 'Connecting to your community…' })}</p>
    </div>
  {:else if mode === 'menu'}
    <p class="step-body">
      {$_('onboarding.actorChoice.body', {
        default:
          'edet is a community ledger: everyone holds their own key, and nobody is admitted by anybody. Create an identity and trade with a member — that trade is what puts you on the ledger — or restore an identity from its recovery phrase.',
      })}
    </p>
    <div class="path-list">
      <button type="button" class="path-card" on:click={enterCreate}>
        <i class="material-icons" aria-hidden="true">fiber_new</i>
        <span class="path-text">
          <span class="path-title">{$_('onboarding.identity.createTitle', { default: 'Create a new identity' })}</span>
          <span class="path-desc">
            {$_('onboarding.identity.createDesc', {
              default:
                'Generates a 12-word recovery phrase; your first purchase from a member is what creates your account.',
            })}
          </span>
        </span>
      </button>
      <button type="button" class="path-card" on:click={() => (mode = 'restore')}>
        <i class="material-icons" aria-hidden="true">restore</i>
        <span class="path-text">
          <span class="path-title">{$_('onboarding.identity.restoreTitle', { default: 'I have a recovery phrase' })}</span>
          <span class="path-desc">
            {$_('onboarding.identity.restoreDesc', {
              default: 'Re-derives your key and finds your membership on the ledger.',
            })}
          </span>
        </span>
      </button>
    </div>
    <div class="actions-row end">
      <Button variant="outlined" on:click={lookAround}>
        <Label>{$_('onboarding.actorChoice.lookAround', { default: 'Look around first' })}</Label>
      </Button>
    </div>
  {:else if mode === 'create'}
    {#if createPhase === 'words'}
      <p class="step-body">
        {$_('onboarding.identity.phraseBody', {
          default:
            'This is your recovery phrase. Write it down and keep it offline — it is shown only once and never stored. Whoever holds it holds this identity; without it, a lost device means a lost key.',
        })}
      </p>
      <ol class="phrase-grid">
        {#each phrase.split(' ') as word, i}
          <li class="phrase-word"><span class="phrase-n">{i + 1}</span>{word}</li>
        {/each}
      </ol>
      <div class="actions-row end">
        <Button variant="outlined" on:click={() => { phrase = ''; mode = 'menu'; }}>
          <Label>{$_('common.back', { default: 'Back' })}</Label>
        </Button>
        <Button variant="outlined" on:click={enterCreate}>
          <Label>{$_('onboarding.identity.regenerate', { default: 'Generate a different phrase' })}</Label>
        </Button>
        <Button variant="raised" on:click={toConfirm}>
          <Label>{$_('onboarding.identity.wroteIt', { default: 'I wrote it down' })}</Label>
        </Button>
      </div>
    {:else if createPhase === 'confirm'}
      <p class="step-body">
        {$_('onboarding.identity.confirmBody', {
          values: { n: confirmIndex + 1 },
          default: `To make sure it is written down: what is word number ${confirmIndex + 1} of your phrase?`,
        })}
      </p>
      <div class="actions-row">
        <Textfield
          label={$_('onboarding.identity.confirmLabel', { values: { n: confirmIndex + 1 }, default: `Word ${confirmIndex + 1}` })}
          bind:value={confirmInput}
        />
        <Button variant="raised" on:click={checkConfirm}>
          <Label>{$_('common.confirm', { default: 'Confirm' })}</Label>
        </Button>
        <Button variant="outlined" on:click={() => (createPhase = 'words')}>
          <Label>{$_('common.back', { default: 'Back' })}</Label>
        </Button>
      </div>
    {/if}
  {:else if mode === 'restore'}
    <p class="step-body">
      {$_('onboarding.identity.restoreBody', {
        default:
          'Enter your 12-word recovery phrase. The key is re-derived locally — the phrase never leaves this device — and the ledger is asked which membership it belongs to.',
      })}
    </p>
    <Textfield
      textarea
      label={$_('onboarding.identity.restoreLabel', { default: 'Recovery phrase (12 words)' })}
      bind:value={restoreInput}
      style="width: 100%;"
    />
    <Textfield
      label={$_('onboarding.actorChoice.nickname', { default: 'Display name on this device (optional)' })}
      bind:value={nickname}
      style="width: 100%;"
    />
    <div class="actions-row end">
      <Button variant="outlined" on:click={() => (mode = 'menu')}>
        <Label>{$_('common.back', { default: 'Back' })}</Label>
      </Button>
      <Button variant="raised" disabled={restoring} on:click={restore}>
        <Label>{$_('onboarding.identity.restore', { default: 'Restore' })}</Label>
      </Button>
    </div>
  {/if}
</div>

<style>
  .step {
    display: flex;
    flex-direction: column;
    gap: 18px;
    max-width: 640px;
    margin: 0 auto;
  }
  .step-title {
    margin: 0;
    color: var(--mdc-theme-primary);
  }
  .step-body {
    margin: 0;
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.5;
  }
  .path-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .path-card {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    padding: 16px;
    border-radius: 10px;
    border: 2px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    background: var(--mdc-theme-surface, #fff);
    cursor: pointer;
    font: inherit;
    text-align: left;
    color: var(--mdc-theme-on-surface);
  }
  .path-card:hover {
    border-color: var(--mdc-theme-primary);
  }
  .path-card i {
    color: var(--mdc-theme-primary);
    font-size: 26px;
  }
  :global(.dark-theme) .path-card {
    background: #1e1e1e;
    border-color: rgba(255, 255, 255, 0.15);
  }
  .path-text {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .path-title {
    font-weight: 600;
  }
  .path-desc {
    font-size: 0.88rem;
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.4;
  }
  .phrase-grid {
    list-style: none;
    margin: 0;
    padding: 16px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 8px;
    border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
    border-radius: 10px;
    background: var(--mdc-theme-background, #fafafa);
  }
  :global(.dark-theme) .phrase-grid {
    background: rgba(255, 255, 255, 0.04);
    border-color: rgba(255, 255, 255, 0.15);
  }
  .phrase-word {
    font-family: monospace;
    font-size: 0.95rem;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--mdc-theme-surface, #fff);
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.08));
  }
  :global(.dark-theme) .phrase-word {
    background: #1e1e1e;
    border-color: rgba(255, 255, 255, 0.1);
  }
  .phrase-n {
    color: var(--mdc-theme-text-secondary-on-surface, #999);
    margin-right: 8px;
    font-size: 0.8rem;
  }
  .actions-row {
    display: flex;
    gap: 12px;
    align-items: flex-end;
    flex-wrap: wrap;
  }
  .actions-row.end {
    justify-content: flex-end;
  }
</style>
