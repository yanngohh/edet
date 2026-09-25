<script>
  import { onDestroy } from "svelte";
  import { hint } from "./hint.js";
  import { loadLife, money } from "./load.js";
  import { shortName, initials, capacityFlow, personFacts } from "./analytics.js";
  import Icon from "./Icon.svelte";
  export let person;
  export let day;
  export let data;
  export let at;
  $: flow = row ? capacityFlow(day, row.id) : null;
  let life = [],
    loading = false,
    error = "",
    version = 0;
  // **A person is not a member.** Most of a town can be people the ledger has
  // no row for — pilot-3 had 113 of them — so everything the ledger says is
  // read through `row` and guarded, and what the town knows is shown instead.
  $: facts = personFacts(data.run, day, person);
  $: p = facts.p;
  $: name = shortName(data.run, person);
  $: row = facts.row;
  $: purse = facts.purse;
  $: lived = life.filter((x) => x.tick <= day.tick);
  $: connections = facts.connections;
  $: refresh(data.lives, person);
  async function refresh(lives, selectedPerson) {
    const request = ++version;
    life = [];
    loading = true;
    error = "";
    try {
      const result = await loadLife(lives, selectedPerson);
      if (request === version) life = result;
    } catch {
      if (request === version) error = "This person’s conversation could not be read.";
    } finally {
      if (request === version) loading = false;
    }
  }
  onDestroy(() => version++);
</script>

<div class="profile">
  <div class="profile-header"><span class="avatar" class:underwriter={row?.supply > 0}>{initials(name)}</span><div><div class="eyebrow">{row?.supply > 0 ? 'COMMUNITY UNDERWRITER' : row ? 'COMMUNITY MEMBER' : 'IN THE TOWN, NO ACCOUNT'}</div><h2>{name}</h2><span>{row ? `Member ${row.id} · ${row.status}` : `No account on the ledger · waiting since day ${facts.since}`} {#if p?.founder}· Founder{/if}</span></div></div>
  {#if !row}<div class="waiting-note"><Icon name="info" size={14} />The ledger has no row for them on this day. What is shown is the town's: their household, {#if facts.sponsor != null}the offer from {shortName(data.run, facts.sponsor)} that introduced them, {/if}and their own words.</div>{/if}
  {#if row?.open_default > 0}<div class="default-note"><Icon name="info" size={14} />In default · {money(row.open_default)} credit units</div>{/if}
  <p class="card">{p?.card || 'Their card has not been written.'} <span class="g" use:hint={"A generated persona, not a description of a real person"}>ⓖ</span></p>
  <div class="person-stats"><div><span>Credit capacity</span><strong>{row ? money(row.capacity) : '–'}</strong><small>individual ceiling</small></div><div><span>Outstanding debt</span><strong>{row ? money(row.debt) : '–'}</strong><small>credit units</small></div><div><span>Cash on hand</span><strong>{purse ? money(purse.cash) : '–'}</strong><small>separate cash economy</small></div><div><span>Unpaid bills</span><strong class:coral-text={purse?.arrears > 0}>{purse ? money(purse.arrears) : '–'}</strong><small>cash units</small></div></div>
  {#if row?.supply > 0}<div class="profile-fact"><Icon name="shield" size={14} /><span>Declared supply</span><strong>{money(row.supply)}</strong></div>{/if}
  {#if row}<div class="profile-fact"><Icon name="link" size={14} /><span>Connected backings</span><strong>{connections.length}</strong></div>{/if}
  {#if facts.sponsor != null}<div class="profile-fact"><Icon name="people" size={14} /><span>Introduced by</span><strong>{shortName(data.run, facts.sponsor)}</strong></div>{/if}
  {#if flow}
    <div class="flow-block">
      <div class="eyebrow">WHAT REACHES THEM</div>
      <h3>{money(flow.reach)}<small> could carry</small></h3>
      {#if flow.carrying.length}
        <p class="flow-note">The paths the community's backings take to reach them, and the most those paths could carry. It is <em>above</em> the capacity on the left, and often far above it: credit already drawn reserves its flow, and the ledger's figure is what is left. The tape carries the stakes but not the reservations, so this says by what route, never how much is free.</p>
        {#each flow.carrying.slice(0, 7) as arc}
          <div class="flow-arc">
            <span>{arc.from == null ? 'a declared supply' : shortName(data.run, day.members.find((m) => m.id === arc.from)?.person)} → {shortName(data.run, day.members.find((m) => m.id === arc.to)?.person)}</span>
            <strong>{money(arc.used)}<small> of {money(arc.of)}</small></strong>
          </div>
        {/each}
      {:else}
        <p class="flow-note">No stake reaches them yet, so the community underwrites nothing for them. An uninsured trade, repaid, is what writes the first one.</p>
      {/if}
    </div>
  {/if}
  <details class="identity"><summary>Identity & reading context</summary><p>Reads at the <strong>{p?.tier || 'unknown'}</strong> tier.</p>{#if p?.introduced_by != null}<p>Introduced by {shortName(data.run, p.introduced_by)}.</p>{/if}<p class="address">{p?.address || 'No address recorded'}</p>{#if p?.retired}<p>Retired: {p.retired}</p>{/if}</details>
  <div class="life-heading"><div><div class="eyebrow">THE PERSON BEHIND THE NODE</div><h3>Their recorded thinking <span class="g" use:hint={"Generated self-report and tool calls, not a measurement"}>ⓖ</span></h3></div><span>{lived.length}</span></div>
  <p class="life-caption">Conversations recorded through day {data.days[at].epoch}.</p>
  {#if loading}<p class="life-empty" role="status">Reading their days…</p>{:else if error}<p class="life-empty coral-text" role="alert">{error}</p>{:else if !lived.length}<p class="life-empty">No conversations are available through this day. Later snapshots may reveal more.</p>{/if}
  {#each [...lived].reverse() as x, i}
    <details class="exchange" open={i === 0}>
      <summary><span>{x.what === 'card' ? 'Their persona was written' : `Tick ${x.tick}`}</span><span class="g" use:hint={"Generated by the model playing this person: one run's sample, never a measurement of how people behave."}>ⓖ</span></summary>
      {#each x.messages || [] as m}
        {#if !Array.isArray(m.content)}<div class="message"><span class="message-role">{m.role}</span><pre class={m.role}>{typeof m.content === 'string' ? m.content : JSON.stringify(m.content, null, 2)}</pre></div>{/if}
        {#each Array.isArray(m.content) ? m.content : [] as b}
          {#if b.type === 'text'}<div class="message"><span class="message-role">{m.role}</span><pre class={m.role}>{b.text}</pre></div>
          {:else if b.type === 'tool_use'}<div class="message"><span class="message-role" class:diary={b.name === 'diary'}>{b.name === 'diary' ? 'Their diary' : `Tool call · ${b.name}`}</span><pre class="use">{JSON.stringify(b.input, null, 2)}</pre></div>
          {:else if b.type === 'tool_result'}<div class="message"><span class="message-role">Tool result</span><pre class="result" class:err={b.is_error}>{typeof b.content === 'string' ? b.content : JSON.stringify(b.content, null, 2)}</pre></div>{/if}
        {/each}
      {/each}
    </details>
  {/each}
</div>

<style>
  .profile {
    overflow-wrap: anywhere;
  }
  .profile-header {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .profile-header .avatar {
    width: 44px;
    height: 44px;
    font-size: 13px;
  }
  .profile-header .eyebrow {
    font-size: 7px;
    letter-spacing: 1px;
  }
  .profile-header h2 {
    font-size: 18px;
    margin: 6px 0;
  }
  .profile-header > div > span {
    font-size: 9px;
    color: var(--muted);
  }
  .card {
    margin: 18px 0;
    font-size: 11px;
    color: var(--secondary);
    line-height: 1.8;
  }
  .waiting-note {
    display: flex;
    gap: 7px;
    padding: 10px;
    border: 1px dashed var(--line);
    border-radius: 5px;
    color: var(--muted);
    margin-top: 15px;
    font-size: 10px;
    line-height: 1.6;
  }
  .default-note {
    display: flex;
    gap: 7px;
    padding: 10px;
    background: color-mix(in srgb, var(--coral) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--coral) 22%, transparent);
    border-radius: 5px;
    color: var(--coral);
    margin-top: 15px;
    font-size: 10px;
  }
  .person-stats {
    display: grid;
    grid-template-columns: 1fr 1fr;
    border: 1px solid var(--line);
    border-radius: 7px;
    overflow: hidden;
    margin-bottom: 15px;
  }
  .person-stats > div {
    padding: 12px;
  }
  .person-stats > div:nth-child(odd) {
    border-right: 1px solid var(--line);
  }
  .person-stats > div:nth-child(-n + 2) {
    border-bottom: 1px solid var(--line);
  }
  .person-stats span {
    color: var(--muted);
    font-size: 9px;
  }
  .person-stats strong {
    display: block;
    font-family: var(--mono);
    font-weight: 400;
    font-size: 17px;
    margin: 7px 0 5px;
  }
  .person-stats small {
    font-size: 8px;
    color: var(--muted);
  }
  .profile-fact {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 9px 0;
    font-size: 10px;
    color: var(--muted);
  }
  .profile-fact strong {
    color: var(--secondary);
    font-weight: 400;
    margin-left: auto;
    font-family: var(--mono);
  }
  .identity {
    margin-top: 12px;
    font-size: 10px;
    color: var(--muted);
    padding-block: 13px;
    border-block: 1px solid var(--line);
  }
  .identity summary {
    cursor: pointer;
  }
  .identity p {
    margin-top: 10px;
    line-height: 1.7;
  }
  .identity .address {
    font-family: var(--mono);
    font-size: 9px;
  }
  .life-heading {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-top: 24px;
  }
  .life-heading .eyebrow {
    font-size: 7px;
    letter-spacing: 1.2px;
  }
  .life-heading h3 {
    margin-top: 8px;
  }
  .life-heading > span {
    font-family: var(--mono);
    color: var(--muted);
    font-size: 10px;
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 4px 6px;
  }
  .life-caption {
    margin: 8px 0 16px;
    font-size: 9px;
    color: var(--muted);
  }
  .life-empty {
    color: var(--muted);
    font-size: 11px;
    line-height: 1.8;
    margin-top: 15px;
  }
  .exchange {
    background: color-mix(in srgb, var(--ink) 22%, transparent);
    border: 1px solid var(--line);
    border-radius: 6px;
    margin-bottom: 8px;
    overflow: hidden;
  }
  .exchange summary {
    padding: 11px;
    font-size: 10px;
    color: var(--secondary);
    cursor: pointer;
  }
  .exchange summary .g {
    margin-left: 5px;
  }
  .message {
    padding: 4px 11px 10px;
  }
  .message-role {
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: var(--muted);
  }
  .message-role.diary {
    color: var(--gold);
  }
  pre {
    white-space: pre-wrap;
    word-break: break-word;
    font-family: inherit;
    font-size: 10px;
    line-height: 1.8;
    color: var(--secondary);
    margin: 7px 0 0;
  }
  .use,
  .result {
    font-family: var(--mono);
    font-size: 9px;
    max-height: 300px;
    overflow: auto;
  }
  .err {
    color: var(--coral);
  }
  .assistant {
    color: var(--secondary);
  }

  .flow-block { border-top: 1px solid var(--line); margin-top: 0.9rem; padding-top: 0.8rem; }
  .flow-block h3 { margin: 0.15rem 0 0.35rem; font-size: 1.35rem; font-weight: 500; }
  .flow-note { color: var(--muted); font-size: 0.8rem; margin: 0 0 0.5rem; }
  .flow-arc { display: flex; justify-content: space-between; gap: 0.5rem; padding: 0.28rem 0; border-bottom: 1px solid var(--line); font-size: 0.83rem; }
  .flow-arc small { opacity: 0.6; font-weight: 400; }
</style>
