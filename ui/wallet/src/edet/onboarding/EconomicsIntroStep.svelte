<script lang="ts">
  /**
   * Final onboarding step: a plain-language explainer of the debt/credit
   * model, shown once after the guided tour and before the wizard closes.
   *
   * The four load-bearing facts, explicit up front:
   *   1. Buying = taking on debt to the seller, not spending a token.
   *   2. You clear debt by selling onward.
   *   3. Unrepaid debt defaults after the maturity window.
   *   4. Capacity is earned standing — the backing others have staked on you,
   *      not a limit anybody grants and not one you can raise alone.
   */
  import { _ } from 'svelte-i18n';
  import Button, { Label } from '@smui/button';

  import { networkView } from '../../lib/node';
  import { advance, markEconomicsSeen } from '../../common/onboardingStore';
  import { lsSet } from '../../common/safeStorage';

  $: minMaturity = $networkView?.min_maturity_epochs ?? 30;

  function onContinue() {
    // Persist "seen" so a returning user on this device isn't shown the
    // explainer again, mirroring the tour's `edet-tour-seen` flag.
    lsSet('edet-economics-seen', '1');
    markEconomicsSeen();
    // **A step advances; only the sequencer decides what is next.** This one
    // ended the wizard outright while it happened to be last, and truncated
    // the whole sequence the moment it stopped being — no key step, no tour.
    // `advance()` reaches `complete` when nothing is outstanding, and that
    // unmounts the overlay just the same.
    advance();
  }
</script>

<div class="step">
  <h2 class="step-title">
    {$_('onboarding.economicsIntro.title', { default: 'How buying and selling works' })}
  </h2>
  <p class="step-body">
    {$_('onboarding.economicsIntro.intro', {
      default:
        'edet has no coins to spend — every transaction is a promise between two members, recorded on your community ledger. Here is what that means for you.',
    })}
  </p>

  <ul class="points">
    <li class="point">
      <span class="point-icon material-icons" aria-hidden="true">shopping_cart</span>
      <div class="point-text">
        <span class="point-title">
          {$_('onboarding.economicsIntro.point1Title', { default: 'Buying creates debt' })}
        </span>
        <span class="point-body">
          {$_('onboarding.economicsIntro.point1Body', {
            default:
              "When you buy from someone, you don't hand over money — you take on a debt to them for the amount.",
          })}
        </span>
      </div>
    </li>
    <li class="point">
      <span class="point-icon material-icons" aria-hidden="true">storefront</span>
      <div class="point-text">
        <span class="point-title">
          {$_('onboarding.economicsIntro.point2Title', { default: 'Selling clears debt' })}
        </span>
        <span class="point-body">
          {$_('onboarding.economicsIntro.point2Body', {
            default:
              'You clear your debt by selling onward to others. A sale first discharges what you owe — through your support circle too — and only the remainder becomes new debt for the buyer.',
          })}
        </span>
      </div>
    </li>
    <li class="point">
      <span class="point-icon material-icons" aria-hidden="true">schedule</span>
      <div class="point-text">
        <span class="point-title">
          {$_('onboarding.economicsIntro.point3Title', { default: 'Unpaid debt defaults' })}
        </span>
        <span class="point-body">
          {$_('onboarding.economicsIntro.point3Body', {
            values: { epochs: minMaturity },
            default: `Every contract has a maturity (at least ${minMaturity} epochs). If a debt isn't cleared by then, it can be marked as a default: your capacity drops sharply and anyone who staked on you is exposed. A late payment ("cure") repairs it.`,
          })}
        </span>
      </div>
    </li>
    <li class="point">
      <span class="point-icon material-icons" aria-hidden="true">balance</span>
      <div class="point-text">
        <span class="point-title">
          {$_('onboarding.economicsIntro.point4Title', { default: 'Your capacity is earned standing' })}
        </span>
        <span class="point-body">
          {$_('onboarding.economicsIntro.point4Body', {
            default:
              'How much you can owe is not a bank limit and not a score: it is the backing the community has actually staked on you, and it grows as you settle what you owe. Nobody grants it, and you cannot raise it alone. A brand-new community starts with everyone at zero — the first trades are simply not covered by anybody, and settling them is what creates the backing for the next ones.',
          })}
        </span>
      </div>
    </li>
  </ul>

  <div class="guardian-cta" role="note">
    <span class="cta-icon material-icons" aria-hidden="true">shield</span>
    <div class="cta-text">
      <span class="cta-title">
        {$_('onboarding.economicsIntro.guardianCtaTitle', {
          default: 'Set up account recovery before you need it',
        })}
      </span>
      <span class="cta-body">
        {$_('onboarding.economicsIntro.guardianCtaBody', {
          default:
            'Nominate guardians early (Settings → Account & recovery): trusted members who can jointly rotate your membership to a new key if this device is ever lost. A rotation waits out a veto window during which your current key can block a theft. Guardians can only be nominated while you still control your key.',
        })}
      </span>
    </div>
  </div>

  <div class="actions-row end">
    <Button variant="raised" on:click={onContinue}>
      <Label>{$_('onboarding.economicsIntro.continue', { default: 'Got it, continue' })}</Label>
    </Button>
  </div>
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
  .points {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .point {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    padding: 14px 16px;
    border-radius: 8px;
    border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
    background: var(--mdc-theme-surface, #fff);
  }
  :global(.dark-theme) .point {
    background: #1e1e1e;
    border-color: rgba(255, 255, 255, 0.1);
  }
  .point-icon {
    font-size: 24px !important;
    color: var(--mdc-theme-primary);
    flex-shrink: 0;
  }
  .point-text {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .point-title {
    font-weight: 600;
    font-size: 1rem;
    color: var(--mdc-theme-on-surface);
  }
  .point-body {
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.45;
    font-size: 0.95rem;
  }
  .guardian-cta {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    padding: 14px 16px;
    border-radius: 8px;
    border: 1px solid rgba(46, 125, 50, 0.4);
    background: rgba(46, 125, 50, 0.06);
  }
  :global(.dark-theme) .guardian-cta {
    background: rgba(46, 125, 50, 0.12);
    border-color: rgba(46, 125, 50, 0.3);
  }
  .cta-icon {
    font-size: 24px !important;
    color: #2e7d32;
    flex-shrink: 0;
  }
  :global(.dark-theme) .cta-icon {
    color: #a5d6a7;
  }
  .cta-text {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .cta-title {
    font-weight: 600;
    font-size: 1rem;
    color: var(--mdc-theme-on-surface);
  }
  .cta-body {
    color: var(--mdc-theme-text-secondary-on-surface);
    line-height: 1.45;
    font-size: 0.95rem;
  }
  .actions-row {
    display: flex;
    gap: 12px;
  }
  .actions-row.end {
    justify-content: flex-end;
  }
</style>
