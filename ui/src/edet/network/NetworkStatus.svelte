<script lang="ts">
    import { _ } from 'svelte-i18n';

    import MemberChip from '../../components/MemberChip.svelte';
    import StatCard from '../../components/StatCard.svelte';
    import Explain from '../../components/Explain.svelte';
    import { networkView, clusterNodes } from '../../lib/node';
    import { epochDate } from '../../lib/epoch';
    import { formatNumber } from '../../common/functions';

    $: net = $networkView;
    $: reachable = $clusterNodes.filter((n) => n.up && n.hash);
    $: agree = reachable.length > 0 && reachable.every((n) => n.hash === reachable[0].hash);
</script>

<div class="main-container flex-column">
    <Explain summary={$_('network.summary', { default: 'Your community ledger is replicated by its charter validators.' })}>
        {$_('network.intro', {
            default:
                'Every node commits the same blocks and must land on bit-identical state. The state commitment below is that fingerprint.',
        })}
    </Explain>

    {#if $clusterNodes.length > 1}
        <!-- Read-only federation telemetry: this node's peers and whether
             they agree on the state commitment. No switching — this device
             talks to its own node only. -->
        <div class="cluster-panel">
            <div class="cluster-head">
                <h3 class="section-title" style="border: none; margin: 0; padding: 0;">
                    {$_('network.cluster', { default: 'Federation' })}
                </h3>
                <span>
                    {#if reachable.length === 0}
                        <span class="muted">{$_('network.noNodes', { default: 'no nodes reachable' })}</span>
                    {:else if agree}
                        <span class="agree">✓ {$_('network.agree', { values: { count: reachable.length }, default: `all ${reachable.length} agree` })}</span>
                    {:else}
                        <span class="disagree">✗ {$_('network.diverged', { default: 'diverged' })}</span>
                    {/if}
                </span>
            </div>
            <div class="node-row">
                {#each $clusterNodes as n (n.url)}
                    <span class="node-pill" class:self={n.self} title={n.url}>
                        <span class="node-dot {n.up ? 'up' : 'down'}"></span>
                        {$_('network.node', { values: { index: n.index }, default: `node ${n.index}` })}
                        {#if n.self} · {$_('network.selfNode', { default: 'yours' })}{/if}
                        {#if n.up}<span class="mono muted"> · h{n.height} · {n.hash?.slice(0, 8)}</span>{/if}
                    </span>
                {/each}
            </div>
        </div>
    {:else}
        <Explain
            tone="notice"
            icon="info"
            summary={$_('network.singleSummary', { default: 'This network declares one node, so its answers are checked against nothing.' })}
        >
            {$_('network.single', { default: 'A second node is what makes a disagreement visible.' })}
        </Explain>
    {/if}

    {#if net}
        <div class="stat-grid">
            <StatCard label={$_('network.height', { default: 'Block height' })} value={String(net.height)} icon="layers"
                help={$_('network.heightHelp', { default: 'Committed blocks. Every block is an agreed, ordered batch of transactions.' })} />
            <!-- The number AND the day it is. This is the one screen where the
                 ledger's own unit is the subject, so the epoch stays the value
                 and the date sits beside it; everywhere else a date replaces
                 the number outright (`lib/epoch.ts`). -->
            <StatCard label={$_('network.epoch', { default: 'Epoch' })} value={String(net.epoch)} icon="schedule"
                detail={$epochDate(net.epoch)}
                help={$_('network.epochHelp', { default: 'The ledger clock: maturities, decay, and network recomputations tick per epoch. One epoch is one day.' })} />
            <StatCard label={$_('network.membersCount', { default: 'Members' })} value={`${net.active_members} / ${net.members}`} icon="group"
                help={$_('network.membersHelp', { default: 'Active members / all members ever admitted.' })} />
            <!-- §Adoption's honest signals. What must not sit here are two cards the
                 node has never served: an activity gauge and a "community
                 brake", both naming a governor this model does not run. There is no
                 macroprudential brake in this design and its absence is
                 deliberate (the paper, the paper's stability section) — so
                 the card read `NaN%` under help text promising the community
                 throttles credit centrally. A view that describes a mechanism
                 the chain does not run is read as a promise. -->
            <StatCard label={$_('network.insured', { default: 'Insured credit' })} value={formatNumber(net.insured_credit, 2)} icon="verified_user"
                help={$_('network.insuredHelp', { default: 'What the community is actually standing behind right now: the backing drawn through its underwriters. Declared supply is an upper bound and not this.' })} />
            <StatCard label={$_('network.seed', { default: 'External seed' })} value={formatNumber(net.external_seed, 2)} icon="park"
                help={$_('network.seedHelp', { default: 'Backing brought from outside the community — at founding, or by a later ceremony. It is the only thing that grows what the community can really carry, and it is what governance votes with.' })} />
            <StatCard label={$_('network.utilisation', { default: 'Ceiling in use' })} value={Math.round(net.utilisation * 100) + '%'} icon="donut_large"
                tone={net.utilisation >= 1 ? 'warn' : 'default'}
                help={$_('network.utilisationHelp', { default: 'How much of the community\'s insurable backing is already committed. At 100% new obligations are still allowed, but they are uninsured — nobody throttles them.' })} />
            <StatCard label={$_('network.uninsured', { default: 'Uninsured obligations' })} value={String(net.uninsured_obligations)} icon="report"
                help={$_('network.uninsuredHelp', { default: 'Live obligations the community does not stand behind, borne by their creditors alone. A rising count means the seed is small, where the ceiling only tells you it is full.' })} />
            <StatCard label={$_('network.mempool', { default: 'Pending transactions' })} value={String(net.mempool)} icon="pending"
                help={$_('network.mempoolHelp', { default: 'Queued transactions waiting for the next block.' })} />
        </div>

        <h3 class="section-title">{$_('network.validatorsTitle', { default: 'Charter validators' })}</h3>
        {#if net.validators.length === 0}
            <p class="note">{$_('network.noValidators', { default: 'No validator set is registered on this dev ledger — the dev cluster commits by node quorum.' })}</p>
        {:else}
            <div class="validator-row">
                {#each net.validators as v}
                    <MemberChip memberId={v} />
                {/each}
            </div>
        {/if}

        <h3 class="section-title">{$_('network.commitmentTitle', { default: 'State commitment' })}</h3>
        <code class="hash">{net.state_hash}</code>
        <p class="note">
            {$_('network.devNote', {
                default:
                    'Dev cluster consensus is the crash-fault protocol; Byzantine safety is the production engine\'s job and is verified in the node\'s BFT suite.',
            })}
        </p>
    {/if}
</div>

<style>
    .main-container {
        width: 100%;
        padding: 16px;
        box-sizing: border-box;
        gap: 14px;
        max-width: var(--edet-column);
    }
    .cluster-panel {
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.12));
        border-radius: 8px;
        padding: 14px;
        background: var(--mdc-theme-surface, #fff);
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    :global(.dark-theme) .cluster-panel {
        background: #1e1e1e;
        border-color: rgba(255, 255, 255, 0.1);
    }
    .cluster-head {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 12px;
        flex-wrap: wrap;
    }
    .node-row {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }
    .node-pill {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        padding: 8px 12px;
        border-radius: 6px;
        border: 1px solid var(--mdc-theme-text-hint-on-background, rgba(0, 0, 0, 0.2));
        color: var(--mdc-theme-on-surface);
    }
    .node-pill.self {
        border-color: var(--mdc-theme-primary);
        color: var(--mdc-theme-primary);
        font-weight: 600;
    }
    .node-dot {
        width: 8px;
        height: 8px;
        border-radius: 50%;
        display: inline-block;
    }
    .node-dot.up { background: #43a047; }
    .node-dot.down { background: #d32f2f; }
    .agree { color: #2e7d32; }
    .disagree { color: #d32f2f; font-weight: 600; }
    .muted { color: var(--mdc-theme-text-secondary-on-surface, #888); }
    .mono { font-family: monospace; }
    .stat-grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
        gap: 12px;
    }
    .section-title {
        color: var(--mdc-theme-primary);
        border-bottom: 2px solid var(--mdc-theme-primary);
        padding-bottom: 8px;
        margin: 8px 0 0;
        font-weight: 500;
    }
    .validator-row {
        display: flex;
        gap: 16px;
        flex-wrap: wrap;
    }
    .hash {
        font-family: monospace;
        font-size: 0.8rem;
        word-break: break-all;
        padding: 10px 12px;
        border-radius: 6px;
        background: var(--mdc-theme-background, #f5f5f5);
        border: 1px dashed var(--mdc-theme-text-hint-on-background, #ccc);
    }
    :global(.dark-theme) .hash {
        background: rgba(255, 255, 255, 0.05);
        border-color: rgba(255, 255, 255, 0.15);
    }
    .note {
        margin: 0;
        font-size: 0.82rem;
        color: var(--mdc-theme-text-secondary-on-surface, #888);
        line-height: 1.45;
    }
</style>
