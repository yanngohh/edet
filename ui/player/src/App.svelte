<script>
  import { onMount, onDestroy } from "svelte";
  import { loadScenario, loadScenarios, money, personName } from "./load.js";
  import { refusalsByCode, capacityFlow, contractRegister, registerMatches } from "./analytics.js";
  import { anomalies } from "./anomalies.js";
  import { meaning, segments } from "./codes.js";
  import { hint, tip } from "./hint.js";
  import { createDemo } from "./demo.js";
  import {
    snapshotStats,
    economySeries,
    dayHighlights,
    milestones,
    sparkline,
    compactMoney,
    shortName,
    initials,
    naming,
    outcomeText,
    peopleOn,
  } from "./analytics.js";
  import { layoutNetwork, networkScales } from "./network.js";
  import Graph from "./Graph.svelte";
  import Tracks from "./Tracks.svelte";
  import DayEvents from "./DayEvents.svelte";
  import Person from "./Person.svelte";
  import Icon from "./Icon.svelte";
  import "./app.css";

  let data = createDemo();
  // The ledger's own words name a number; this names the person.
  $: named = naming(data.run);
  const outcome = (row, person) => outcomeText(data.run, row, person, named);
  let at = 23,
    playing = false,
    timer,
    speed = 1,
    error = "",
    loading = false;
  let page = "community",
    inspector = "activity",
    selected = null,
    search = "",
    contractFilter = "all";
  // Light or dark, remembered per browser. The wallet's own two themes.
  let theme = (typeof localStorage !== 'undefined' && localStorage.getItem('edet-theme')) || 'dark';
  $: if (typeof document !== 'undefined') {
    document.documentElement.dataset.theme = theme;
    try { localStorage.setItem('edet-theme', theme); } catch {}
  }
  let help = false,
    reducedMotion = false;
  let loadVersion = 0;
  $: day = data.days[at];
  $: stats = snapshotStats(day, data.run);
  $: series = economySeries(data.days);
  $: positions = layoutNetwork(data.days);
  $: scales = networkScales(data.days);
  $: points = milestones(data.days);
  $: highlights = dayHighlights(day, data.days[at - 1]);
  $: violations = data.days
    .slice(0, at + 1)
    .flatMap((d, i) =>
      d.events.filter((e) => e.event === "violation").map((e) => ({ ...e, at: i, epoch: d.epoch })),
    );
  $: cards = [
    {
      label: "Community members",
      value: stats.members,
      unit: "people",
      field: "members",
      icon: "people",
      color: "mint",
      // The ledger's count is the value; the town is bigger, and says so.
      note: `${day.members.filter((m) => m.supply > 0).length} underwriters · ${stats.waiting} in town without an account`,
    },
    {
      label: "Declared backing",
      value: compactMoney(stats.seed),
      unit: "credit units",
      field: "seed",
      icon: "shield",
      color: "gold",
      note: "Supply committed by underwriters",
    },
    {
      label: "Credit outstanding",
      value: compactMoney(stats.outstanding),
      unit: "credit units",
      field: "outstanding",
      icon: "activity",
      color: "mint",
      note: `${stats.insured} of ${stats.live} open contracts insured`,
    },
    {
      label: "Active contracts",
      value: stats.active,
      unit: "agreements",
      field: "active",
      icon: "link",
      color: "lavender",
      note: `${stats.closed} closed${stats.transferred ? ` · ${stats.transferred} handed on` : ""} · ${stats.defaults} in default`,
    },
  ];
  // **Everybody in the town, not only the ledger's rows.** A person exists
  // from the day somebody offers them a first trade, and most of pilot-3's
  // never got past that: 113 of 128 had a life on the tape and no account.
  $: town = peopleOn(data.run, day);
  $: people = town.filter((x) =>
    `${shortName(data.run, x.person)} ${x.row ? x.row.id : "no account"}`.toLowerCase().includes(search.toLowerCase()),
  );

  // **The life of one contract**, read out of the days themselves: when it was
  // booked and by whose signature, every payment that changed what is
  // outstanding, and every change of status. Nothing here is new data — the
  // tape carries the acts and the snapshots carry the rows; this only walks
  // them.
  const EMPTY_STUDIES = [];
  let openContract = null;
  function contractLife(id) {
    const key = `${id}@${at}`;
    if (lifeCache.has(key)) return lifeCache.get(key);
    const life = contractLifeOf(id);
    // Every day of a scrub leaves an entry, and each is a prefix of the next:
    // ten contracts over two hundred days was 16 MB of prefixes. Keep the
    // recent ones; the rest cost a walk to rebuild and nothing to forget.
    if (lifeCache.size > 60) lifeCache.delete(lifeCache.keys().next().value);
    lifeCache.set(key, life);
    return life;
  }
  function contractLifeOf(id) {
    const out = [];
    let before = null;
    for (let i = 0; i < data.days.length && i <= at; i++) {
      const d = data.days[i];
      const row = d.contracts.find((c) => c.id === id);
      const acts = [];
      for (const e of d.events || []) {
        if (e.event !== "day" && e.event !== "instructions") continue;
        for (const a of e.acts || e.fired || [])
          if (a.result && a.result.contract === id) acts.push({ ...a, person: e.person ?? a.person });
      }
      if (row && !before) out.push({ epoch: d.epoch, at: i, what: "booked", snapshot: row, acts });
      else if (row && before) {
        if (row.outstanding !== before.outstanding)
          out.push({ epoch: d.epoch, at: i, what: "paid", paid: before.outstanding - row.outstanding, snapshot: row, acts });
        else if (row.status !== before.status) out.push({ epoch: d.epoch, at: i, what: row.status, snapshot: row, acts });
        else if (row.insured !== before.insured)
          out.push({ epoch: d.epoch, at: i, what: row.insured ? "became insured" : "lost its insurance", snapshot: row, acts });
        else if (acts.length) out.push({ epoch: d.epoch, at: i, what: "touched", snapshot: row, acts });
      }
      if (row) before = row;
    }
    return out;
  }
  // The offer a signature closed, so a contract can name who proposed it.
  // Called from the markup, once per signature of the open contract, on every
  // re-render — so the answer is kept.
  let offerCache = new Map();
  function offerFor(digest) {
    const key = `${digest}@${at}`;
    if (!offerCache.has(key)) offerCache.set(key, offerForIn(digest));
    return offerCache.get(key);
  }
  function offerForIn(digest) {
    for (let i = 0; i <= at; i++)
      for (const e of data.days[i].events || [])
        if (e.event === "day")
          for (const a of e.acts || [])
            if (a.act && a.act.act === "offer" && a.result && a.result.digest === digest)
              return { person: e.person, epoch: data.days[i].epoch, at: i, act: a };
    return null;
  }
  // **Everything here walks the tape from day 0 on every step of the cursor,
  // so everything here is cached by day and cleared with the run.** Measured
  // on a synthesised 200-day, 50-person tape: the studies alone were 131 ms of
  // a 138 ms day step, uncached, on every page — a held arrow key gave seven
  // updates a second. A cache and a page test are the whole fix.
  let refusalCache = new Map(), lifeCache = new Map(), studyCache = new Map();
  $: me = selected == null ? null : day.members.find((m) => m.person === selected);
  $: if (data) { refusalCache = new Map(); lifeCache = new Map(); studyCache = new Map(); offerCache = new Map(); }
  $: studies = data && page === "anomalies" ? studiesFor(at) : EMPTY_STUDIES;
  function studiesFor(n) {
    if (!studyCache.has(n)) studyCache.set(n, anomalies(data.days, n));
    return studyCache.get(n);
  }
  let anomalyTab = 0, tabPicked = false;
  // Open on the first topic that found something — but only until the reader
  // picks one, and never again on a later day: a tab that moves under somebody
  // reading it takes away what they were reading.
  $: if (page === "anomalies" && studies.length && !tabPicked) {
    const first = studies.findIndex((g) => g.studies.some((s) => s.hits.length));
    anomalyTab = first < 0 ? 0 : first;
    tabPicked = true;
  }
  $: refused = (() => {
    if (!refusalCache.has(at)) refusalCache.set(at, refusalsByCode(data.days, at));
    return refusalCache.get(at);
  })();
  // Run-wide, so a default cured the same day is still findable: the
  // register does not move with the cursor, and the "ever in default" filter
  // reads it through the current day.
  $: register = contractRegister(data.days);
  $: everDefaulted = new Set(
    register.filter((r) => r.steps.some((s) => s.at <= at && (s.what === "default marked" || s.what === "expired"))).map((r) => r.id),
  );
  let registerFilter = "all";
  $: shownRegister = register.filter((r) => registerMatches(r, registerFilter));
  $: shownContracts = day.contracts.filter(
    (c) =>
      contractFilter === "all" ||
      (contractFilter === "closed"
        ? ["settled", "cured"].includes(c.status)
        : contractFilter === "ever"
          ? everDefaulted.has(c.id)
          : c.status === contractFilter),
  );
  $: activity = data.days.map((d) =>
    (d.events || []).reduce(
      (n, e) =>
        n + (e.event === "day" ? (e.acts || []).length : e.event === "instructions" ? (e.fired || []).length : 0),
      0,
    ),
  );
  $: maxActivity = Math.max(1, ...activity);
  $: progress = (at / Math.max(data.days.length - 1, 1)) * 100;
  $: cut = day.economy.published.households_with_income_cut;

  function pause() {
    playing = false;
    clearInterval(timer);
  }
  function startTimer() {
    clearInterval(timer);
    timer = setInterval(() => {
      if (at >= data.days.length - 1) pause();
      else at += 1;
    }, 1500 / speed);
  }
  function play() {
    if (playing) return pause();
    if (at === data.days.length - 1) at = 0;
    playing = true;
    startTimer();
  }
  function changeSpeed() {
    speed = speed === 1 ? 2 : speed === 2 ? 4 : 1;
    if (playing) startTimer();
  }
  function seek(index) {
    at = Math.max(0, Math.min(data.days.length - 1, Number(index)));
  }
  function step(delta) {
    pause();
    seek(at + delta);
  }
  function choose(person) {
    selected = person;
    page = "community";
  }
  function demo() {
    ++loadVersion;
    pause();
    data = createDemo();
    forget();
    at = 23;
    loading = false;
    error = "";
  }
  // **Everything the reader had open belonged to the run they had open.** A
  // contract id is a small integer: left open across a load, `#14` silently
  // became a different contract's life under the same heading. The anomaly tab
  // was the same failure one screen over — a picked tab kept its index and the
  // new run opened on a topic that had found nothing.
  function forget() {
    at = 0;
    selected = null;
    search = "";
    contractFilter = "all";
    openContract = null;
    anomalyTab = 0;
    tabPicked = false;
  }
  function keyboard(e) {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key === "Escape") {
      if (e.target.closest("input, select, textarea, [contenteditable]")) return;
      help = false;
      selected = null;
      return;
    }
    // A field or the canvas wants its own arrows; a BUTTON does not, and
    // refusing them there was why the timeline stopped answering as soon as
    // anybody clicked anything. Space is the exception: a focused button is
    // pressed with it, so it keeps space and the timeline does not take it.
    const target = e.target.closest("input, select, textarea, [contenteditable], [role=application]");
    if (target) return;
    if (e.code === "Space") {
      if (e.target.closest("button, summary, a")) return;
      e.preventDefault();
      play();
    }
    if (e.key === "ArrowRight") {
      e.preventDefault();
      step(1);
    }
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      step(-1);
    }
  }
  // **The runs beside the tree open without being asked for.** The demo is
  // what there is when nothing has been indexed yet, not what a reader should
  // meet when a real tape is sitting there: the newest one opens, and the rest
  // are a click away. Failing to reach them is not an error the reader needs —
  // it only means there is no server offering any, so the demo stands.
  let scenarios = [];
  let current = "";
  async function openScenario(id) {
    const version = ++loadVersion;
    pause();
    loading = true;
    error = "";
    try {
      const loaded = await loadScenario(id);
      if (version === loadVersion) {
        data = loaded;
        current = id;
        at = 0;
        forget();
      }
    } catch (err) {
      if (version === loadVersion) error = err.message || String(err);
    } finally {
      if (version === loadVersion) loading = false;
    }
  }
  onMount(async () => {
    try {
      scenarios = await loadScenarios();
      if (scenarios.length && data.demo) await openScenario(scenarios[0].id);
    } catch {
      scenarios = [];
    }
  });
  onMount(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const changed = () => (reducedMotion = query.matches);
    changed();
    query.addEventListener("change", changed);
    return () => query.removeEventListener("change", changed);
  });
  onDestroy(pause);
</script>

<svelte:window on:keydown={keyboard} />

<div class="app-shell">
  <header class="topbar">
    <a class="brand" href="./" aria-label="edet community view" on:click|preventDefault={() => page = 'community'}><img class="brand-symbol" src="./edet.svg" alt="" width="26" height="26" /><strong>edet</strong><span class="brand-divider"></span><span class="brand-edition">{data.demo ? 'AUTHORED DEMO' : `${data.run.name || 'RUN'} · ${data.run.not_a_run ? 'SCRIPTED TAPE' : data.run.model}`}</span></a>
    <nav class="main-nav" aria-label="Views">
      <button class:current={page === 'community'} on:click={() => page = 'community'}><Icon name="network" size={15} />Community</button>
      <button class:current={page === 'economy'} on:click={() => page = 'economy'}><Icon name="chart" size={15} />Credit and cash</button>
      <button class:current={page === 'anomalies'} on:click={() => page = 'anomalies'}><Icon name="activity" size={15} />Anomalies</button><button class:current={page === 'ledger'} on:click={() => page = 'ledger'}><Icon name="shield" size={15} />Ledger</button>
    </nav>
    <div class="topbar-actions">
      <span class="source-badge" class:demo={data.demo}><span></span>{data.demo ? 'Illustrative demo' : data.run.not_a_run ? 'Scripted tape' : 'Local run'}</span>
      {#if scenarios.length}
        <label class="run-pick" class:busy={loading} title="Which run to read">
          <span class="sr-only">Run</span>
          <Icon name="folder" size={14} />
          <select id="scenario" disabled={loading} value={current} on:change={(e) => openScenario(e.currentTarget.value)}>
            {#each scenarios as s}
              <option value={s.id}>{s.id} — {s.people} people, {s.days} days{s.not_a_run ? ', scripted' : ''}</option>
            {/each}
          </select>
          <span class="run-pick-chevron" aria-hidden="true"><Icon name="chevron" size={12} /></span>
        </label>
      {/if}
      <button class="icon-button" aria-label={theme === 'dark' ? 'Switch to the light theme' : 'Switch to the dark theme'} title={theme === 'dark' ? 'Light theme' : 'Dark theme'} on:click={() => theme = theme === 'dark' ? 'light' : 'dark'}>{theme === 'dark' ? '☀' : '☾'}</button><button class="icon-button help-button" aria-label="About this visualization" use:hint={"About this visualization"} on:click={() => help = !help}><Icon name="info" size={17} /></button>
    </div>
  </header>

  <main>
    {#if error}<div class="notice error" role="alert"><Icon name="info" size={17} /><span>{error}</span><button class="icon-button" aria-label="Dismiss error" on:click={() => error = ''}><Icon name="close" size={16} /></button></div>{/if}
    {#if data.run.not_a_run && !data.demo}<div class="notice">Scripted tape · This exercises the apparatus. No model generated the people’s decisions.</div>{/if}
    {#if data.run.partial}<div class="notice">The last snapshot in this tape is incomplete and was left out — a run still being written, or one stopped mid-day.</div>{/if}
    {#if day.unclosed}<div class="notice">Incomplete day · This snapshot includes events before the day could close.</div>{/if}
    {#if help}
      <section class="about panel">
        <div><div class="eyebrow">WHAT THIS IS</div><h2>A run of the edet ledger, day by day</h2><p>A community of model-driven people trading over the real ledger. Select anybody to read what they did and what they were thinking when they did it.</p></div>
        <div><strong>Read the network</strong><p>Node size shows individual residual capacity. Gold nodes declare backing; coral rings show default. Capacity is a credit ceiling, and individual capacities cannot be added into a community budget.</p></div>
        <div><strong>Bring your own run</strong><p>Run <code>edet-civitas index &lt;run&gt;</code> and it joins the list above; point the player at another directory with <code>CIVITAS_RUNS=…</code>. Files stay in your browser. <span class="g" use:hint={"Generated by the model playing this person: one run's sample, never a measurement of how people behave."}>ⓖ</span> marks generated choices; they are samples, not behavioral measurements.</p><button class="text-button" on:click={demo}>Load the illustrative demo <Icon name="arrow" size={14} /></button></div>
        <button class="icon-button about-close" aria-label="Close information" on:click={() => help = false}><Icon name="close" size={16} /></button>
      </section>
    {/if}

    <section class="intro">
      <div><h1>{page === 'community' ? 'The community' : page === 'economy' ? 'Credit and cash' : page === 'anomalies' ? 'Anomalies' : 'The ledger'}</h1><p>{page === 'community' ? 'Who is here, what they owe one another, and who backs whom.' : page === 'economy' ? 'What moves on the ledger, beside what moves in money.' : page === 'anomalies' ? 'Shapes in the tape worth a second look, each with the rule that found it.' : 'Every contract the tape records, as the state holds it.'}</p></div>
      <div class="intro-actions"><div class="snapshot-label"><span class="status-dot"></span> SNAPSHOT <strong>DAY {String(day.epoch).padStart(2, '0')}</strong></div><div class="intro-buttons"><button class="button primary play-story" aria-label={playing ? "Pause this run" : "Play this run"} on:click={play}><Icon name={playing ? "pause" : "play"} size={14} />{playing ? "Pause" : "Play"}</button></div></div>
    </section>

    <section class="metrics" aria-label="Ledger snapshot">
      {#each cards as card}
        <div class="metric panel"><div class="metric-top"><span>{card.label}</span><span class="metric-icon {card.color}"><Icon name={card.icon} size={15} /></span></div><div class="metric-value">{card.value}<span>{card.unit}</span><svg viewBox="0 0 88 30" aria-hidden="true" class={card.color}><path d={sparkline(series.slice(0, at + 1).map((s) => s[card.field]), 88, 30)} fill="none" stroke="currentColor" stroke-width="1.5" /></svg></div><div class="metric-note">{card.note}</div></div>
      {/each}
    </section>

    {#if page === 'community'}
      <div class="community-layout">
        <Graph {day} run={data.run} {positions} {scales} {selected} {reducedMotion} {theme} on:choose={(e) => choose(e.detail)} />
        <aside class="inspector panel">
          {#if selected !== null}
            <div class="inspector-top"><button class="text-button back-button" on:click={() => selected = null}><Icon name="previous" size={13} />The community</button><button class="icon-button" aria-label="Close person details" on:click={() => selected = null}><Icon name="close" size={15} /></button></div>
            <div class="inspector-scroll person-scroll"><Person person={selected} {day} {data} {at} /></div>
          {:else}
            <div class="inspector-heading"><div class="eyebrow">THE DAY</div><h2>This day</h2></div>
            <div class="day-summary"><div class="summary-title"><span class="status-dot"></span>On day {day.epoch}<span class="regime" class:pressure={day.economy.regime === 'crisis'}>{day.economy.regime === 'crisis' ? 'Economic pressure' : day.economy.regime === 'boom' ? 'Expansion' : 'Calm conditions'}</span></div><p>{highlights[0].text}</p>{#if highlights[0].people?.length}<p class="day-summary-people">{#each highlights[0].people as who, i}<button class="text-button" on:click={() => choose(who)}>{shortName(data.run, who)}</button>{#if i < highlights[0].people.length - 1}<span>, </span>{/if}{/each}</p>{/if}</div>
            <div class="inspector-tabs"><button class:active={inspector === 'activity'} on:click={() => inspector = 'activity'}>Day’s activity <span class="g" use:hint={"Generated choices, not behavioral measurements"}>ⓖ</span></button><button class:active={inspector === 'people'} on:click={() => inspector = 'people'}>People <span>{stats.people}</span></button></div>
            <div class="inspector-scroll">
              {#if inspector === 'activity'}<DayEvents {day} run={data.run} on:choose={(e) => choose(e.detail)} />
              {:else}
                <label class="people-search"><Icon name="search" size={14} /><input type="search" placeholder="Find a person…" aria-label="Find a person" bind:value={search} /></label>
                <p class="people-count">{stats.people} people in town · {stats.members} with an account{#if stats.waiting} · <span use:hint={"Introduced by an offer that was never seated, or never offered a trade at all. They have a household and a life on the tape, and the ledger has no row for them."}>{stats.waiting} waiting</span>{/if}</p>
                <div class="people-list">{#each people as x}{#if x.row}{@const member = x.row}<button class="person-row" disabled={member.person == null} on:click={() => choose(member.person)}><span class="avatar" class:underwriter={member.supply > 0}>{initials(shortName(data.run, member.person))}</span><span class="person-row-name"><strong>{shortName(data.run, member.person)}</strong><span>{member.supply > 0 ? 'Underwriter' : `Member ${member.id}`}{#if member.open_default > 0} · <span class="coral-text">In default</span>{/if}</span></span><span class="person-capacity">{compactMoney(member.capacity)}<small>capacity</small></span><Icon name="chevron" size={12} /></button>{:else}<button class="person-row waiting" on:click={() => choose(x.person)}><span class="avatar waiting">{initials(shortName(data.run, x.person))}</span><span class="person-row-name"><strong>{shortName(data.run, x.person)}</strong><span>No account yet · waiting since day {x.since}</span></span><span class="person-capacity">{x.purse ? compactMoney(x.purse.cash) : '–'}<small>cash</small></span><Icon name="chevron" size={12} /></button>{/if}{/each}</div>
                {#if !people.length}<p class="empty-message">No people match “{search}”.</p>{/if}
              {/if}
            </div>
            <div class="inspector-footer"><Icon name="spark" size={16} /><p>Select a node to see the person<br />behind the promise.</p><Icon name="arrow" size={15} /></div>
          {/if}
        </aside>
      </div>
      <div class="context-strip"><div><Icon name="link" size={16} /><span><strong>{stats.stakes}</strong> recorded backings</span></div><span class="context-divider"></span><div><Icon name="globe" size={16} /><span>Price index <strong>{Number(day.economy.published.price_index).toFixed(3)}</strong></span></div><span class="context-divider"></span><div><span class="pressure-dot" class:affected={cut > 0}></span><span><strong>{cut}/{day.economy.published.households}</strong> households with income cuts</span></div></div>
    {:else if page === 'economy'}
      <Tracks days={data.days} {at} on:seek={(e) => { pause(); seek(e.detail); }} />
    {:else if page === 'anomalies'}
      <section class="anomalies-layout">
        <div class="panel anomaly-panel">
          <div class="section-heading"><div><div class="eyebrow">THROUGH DAY {day.epoch}</div><h2>What looks odd <span class="muted">/ {studies.reduce((n, g) => n + g.studies.filter((s) => s.hits.length).length, 0)} of {studies.reduce((n, g) => n + g.studies.length, 0)}</span></h2></div></div>
          <p class="anomaly-preamble">Each of these is a claim about a <em>shape</em>, never about intent: a refusal storm is not an attack, a pair trading both ways is not a wash trade, and a community settling in cash is not a verdict on the ledger. The rule is written out so you can disagree with it. Every act on this tape was generated by a model playing somebody <span class="g" use:hint={"Generated by the model playing this person: one run's sample, never a measurement of how people behave."}>ⓖ</span>, so a pattern here is a fact about this run and nothing wider.</p>
          <div class="anomaly-tabs" role="tablist">
            {#each studies as group, i}
              {@const found = group.studies.filter((s) => s.hits.length).length}
              <button role="tab" aria-selected={anomalyTab === i} class:active={anomalyTab === i} class:has={found > 0} on:click={() => { anomalyTab = i; tabPicked = true; }}>
                {group.topic}<span class="anomaly-tab-count">{found ? found : '—'}</span>
              </button>
            {/each}
          </div>
          {#each [studies[anomalyTab]] as group}
            <p class="anomaly-note">{group.note}</p>
            {#each group.studies as study}
              <div class="anomaly" class:found={study.hits.length} class:grave={study.weight === 'grave' && study.hits.length}>
                <div class="anomaly-head">
                  <strong>{study.title}</strong>
                  <span class="anomaly-count">{study.hits.length ? `${study.hits.length} found` : 'none'}</span>
                </div>
                <p class="anomaly-rule">{study.rule}</p>
                {#each study.hits.slice(0, 6) as h}
                  <div class="anomaly-hit">
                    <button class="text-button" on:click={() => { pause(); seek(h.at); }}>Day {h.epoch} <Icon name="arrow" size={12} /></button>
                    <span class="anomaly-detail">{#each segments(h.detail) as part}{#if part.code}<code class="code-hint" use:hint={meaning(part.code)}>{part.text}</code>{:else}{part.text}{/if}{/each}</span>
                    {#if h.people.length}
                      <span class="anomaly-people">{#each h.people.slice(0, 4) as who, i}<button class="text-button" on:click={() => { page = 'community'; choose(who); }}>{shortName(data.run, who)}</button>{#if i < Math.min(h.people.length, 4) - 1}<span>, </span>{/if}{/each}{#if h.people.length > 4}<span> and {h.people.length - 4} more</span>{/if}</span>
                    {/if}
                  </div>
                {/each}
                {#if study.hits.length > 6}<p class="anomaly-more">and {study.hits.length - 6} more.</p>{/if}
              </div>
            {/each}
          {/each}
        </div>
      </section>
    {:else}
      {#if register.length}
        <section class="panel register-panel">
          <div class="section-heading"><div><div class="eyebrow">EVERY CONTRACT OVER THE RUN</div><h2>Contract lives <span class="muted">/ {shownRegister.length} of {register.length}</span></h2></div><label><span class="sr-only">Filter the register</span><select bind:value={registerFilter}><option value="all">All contracts</option><option value="active">Still active</option><option value="ever">Ever in default</option><option value="expired">In default now</option><option value="cured">Cured</option><option value="settled">Settled</option><option value="closed">Settled or cured</option><option value="transferred">Transferred</option><option value="underwriter">Underwriter stepped in</option><option value="insured">Insured</option></select></label></div>
          <p class="muted register-note">Every contract the tape records, from the day it was first seen, with each change of status or creditor on the day it happened — including a default the morning sweep marked and a cure paid off before the close, which no single day's state shows. This list does not move with the day; a day jumps there.</p>
          <div class="table-scroll register-scroll"><table><thead><tr><th>Opened</th><th>Contract</th><th>Debtor → creditor</th><th>Booked</th><th>Insurance</th><th>Life</th><th>Now</th></tr></thead><tbody>{#each shownRegister as r}<tr class:later={r.at > at}><td><button class="text-button" on:click={() => { pause(); seek(r.at); }}>Day {r.epoch} <Icon name="arrow" size={12} /></button></td><td class="contract-id"><button class="text-button" on:click={() => { openContract = openContract === r.id ? null : r.id; pause(); seek(r.at); }}>#{r.id}</button></td><td>{shortName(data.run, data.days[r.at].members.find((m) => m.id === r.debtor)?.person)}<span class="contract-arrow">→</span><span>{shortName(data.run, data.days[r.at].members.find((m) => m.id === r.creditor)?.person)}</span></td><td class="numeric">{money(r.original)}</td><td class="muted">{r.insured ? 'Insured' : 'Uninsured'}</td><td class="life-cell">{#each r.steps.slice(1) as s, k}{#if k}<span class="muted"> · </span>{/if}<button class="text-button life-step" class:coral-text={s.what === 'default marked' || s.what === 'expired'} on:click={() => { pause(); seek(s.at); }}>{s.what === 'creditor changed' ? `claim to ${shortName(data.run, data.days[s.at].members.find((m) => m.id === s.creditor)?.person)}` : s.what} <small>d{s.epoch}</small></button>{:else}<span class="muted">no change since it opened</span>{/each}</td><td><span class="contract-status" class:expired={r.status === 'expired'}>{r.status === 'expired' ? 'In default' : r.status}</span></td></tr>{/each}</tbody></table>{#if !shownRegister.length}<p class="empty-message">No contracts match.</p>{/if}</div>
        </section>
      {/if}
      <section class="ledger-layout">
        <div class="panel contracts-panel"><div class="section-heading"><div><div class="eyebrow">THE RECORDED OBLIGATIONS</div><h2>Contracts <span class="muted">/ {day.contracts.length}</span></h2></div><label><span class="sr-only">Filter contracts</span><select bind:value={contractFilter}><option value="all">All contracts</option><option value="active">Active</option><option value="expired">In default today</option><option value="ever">Ever in default</option><option value="closed">Closed</option><option value="transferred">Transferred</option></select></label></div><div class="table-scroll"><table><thead><tr><th>Contract</th><th>Debtor → creditor</th><th>Outstanding</th><th>Status</th><th>Insurance</th></tr></thead><tbody>{#each shownContracts as c}<tr class:open={openContract === c.id}><td class="contract-id"><button class="text-button" on:click={() => openContract = openContract === c.id ? null : c.id}>#{c.id}</button></td><td><button class="text-button" disabled={day.members.find((m) => m.id === c.debtor)?.person == null} on:click={() => choose(day.members.find((m) => m.id === c.debtor).person)}>{shortName(data.run, day.members.find((m) => m.id === c.debtor)?.person)}</button><span class="contract-arrow">→</span><span>{shortName(data.run, day.members.find((m) => m.id === c.creditor)?.person)}</span></td><td class="numeric">{money(c.outstanding)}</td><td><span class="contract-status" class:expired={c.status === 'expired'}>{c.status === 'expired' ? 'In default' : c.status}</span></td><td class="muted">{c.insured ? 'Insured' : 'Uninsured'}</td></tr>{/each}</tbody></table>{#if !shownContracts.length}<p class="empty-message">No {contractFilter === 'all' ? '' : contractFilter} contracts on this day.</p>{/if}</div></div>
        {#if openContract !== null}
        <div class="panel audit-panel contract-panel">
          <div class="inspector-top"><button class="text-button back-button" on:click={() => openContract = null}><Icon name="previous" size={13} />All contracts</button></div>
          {#each [day.contracts.find((c) => c.id === openContract)] as c}
            {#if c}
              <div class="eyebrow">CONTRACT #{c.id}</div>
              <h2>{money(c.outstanding)} <small>of {money(c.original)} outstanding</small></h2>
              <p>
                <button class="text-button" disabled={day.members.find((m) => m.id === c.debtor)?.person == null} on:click={() => choose(day.members.find((m) => m.id === c.debtor).person)}>{shortName(data.run, day.members.find((m) => m.id === c.debtor)?.person)}</button>
                owes
                <button class="text-button" disabled={day.members.find((m) => m.id === c.creditor)?.person == null} on:click={() => choose(day.members.find((m) => m.id === c.creditor).person)}>{shortName(data.run, day.members.find((m) => m.id === c.creditor)?.person)}</button>
                · due day {c.maturity_epoch} · {c.insured ? 'insured by the community' : 'uninsured: the creditor carries it alone'}
              </p>
              <div class="contract-life">
                {#each contractLife(c.id) as step}
                  <div class="life-step">
                    <button class="text-button" on:click={() => { pause(); seek(step.at); }}>Day {step.epoch} <Icon name="arrow" size={12} /></button>
                    <strong>{step.what === 'paid' ? `paid ${money(step.paid)}, ${money(step.snapshot.outstanding)} left` : step.what}</strong>
                    {#each step.acts as a}
                      <span class="life-act">
                        <button class="text-button" disabled={a.person == null} on:click={() => choose(a.person)}>{shortName(data.run, a.person)}</button>
                        {named(a.act.describe) || a.act.act} — {outcome(a, a.person)}
                      </span>
                      {#if a.act.act === 'sign'}
                        {#each [offerFor(a.act.digest)] as o}
                          {#if o}<span class="life-act muted">offered on day {o.epoch} by <button class="text-button" on:click={() => choose(o.person)}>{shortName(data.run, o.person)}</button> — {named(o.act.act.describe) || named(o.act.says)}</span>{/if}
                        {/each}
                      {/if}
                    {/each}
                  </div>
                {/each}
              </div>
            {/if}
          {/each}
        </div>
      {:else}
        <div class="panel audit-panel"><span class="audit-icon" class:failed={violations.length}><Icon name="shield" size={26} /></span><div class="eyebrow">TAPE INTEGRITY</div><h2>{violations.length ? `${violations.length} recorded violation${violations.length === 1 ? '' : 's'}` : 'No recorded violations'}</h2><p>Invariant failures recorded through day {day.epoch}. This view reports the tape; it does not independently re-audit the ledger.</p>{#each violations as v}<div class="violation"><button class="text-button" on:click={() => { pause(); seek(v.at); }}>Day {v.epoch} <Icon name="arrow" size={12} /></button><strong>{v.invariant}</strong><span>{v.person == null ? 'Epoch sweep' : personName(data.run, v.person)} · {v.act}</span><details><summary>Transaction</summary><pre>{typeof v.tx === 'string' ? v.tx : JSON.stringify(v.tx, null, 2)}</pre></details></div>{/each}<div class="audit-stat"><span>Underwriter seed</span><strong>{money(day.seed)}</strong></div><div class="audit-stat"><span>Seat commitments</span><strong>{money(day.seat_committed)}</strong></div><div class="audit-stat"><span>Pending offers</span><strong>{day.pending_offers ?? '–'}</strong></div><div class="refusal-block"><div class="eyebrow">WHAT WAS TRIED AND NOT TAKEN</div>{#if refused.length}<p class="muted">Through day {day.epoch}. Nothing was applied and nothing was charged. Hover a code for what the wallet tells a member who meets it.</p>{#each refused.filter((r) => r.fromLedger).slice(0, 5) as r}<div class="refusal"><strong>{r.n}×</strong> <code use:hint={meaning(r.code)}>{r.code}</code><span class="muted">{[...r.acts].join(', ')}</span>{#if r.last}<button class="text-button" on:click={() => { pause(); seek(r.last.at); }}>last on day {r.last.epoch} <Icon name="arrow" size={12} /></button>{/if}</div>{/each}{#if refused.some((r) => !r.fromLedger)}<p class="muted refusal-aside">The wallet stopped these before the ledger saw them. They carry a sentence rather than a code, and cost nothing either.</p>{#each refused.filter((r) => !r.fromLedger).slice(0, 3) as r}<div class="refusal"><strong>{r.n}×</strong> <span class="refusal-says">{r.code}</span>{#if r.last}<button class="text-button" on:click={() => { pause(); seek(r.last.at); }}>last on day {r.last.epoch} <Icon name="arrow" size={12} /></button>{/if}</div>{/each}{/if}{:else}<p class="muted">Nothing was refused through day {day.epoch}.</p>{/if}</div></div>
      {/if}
      </section>
    {/if}

    <section class="timeline panel" aria-label="Playback controls">
      <div class="timeline-main"><div class="playback-buttons"><button class="icon-button" aria-label="Previous day" disabled={at === 0} on:click={() => step(-1)}><Icon name="previous" size={17} /></button><button class="play-button" aria-label={playing ? 'Pause playback' : 'Play timeline'} on:click={play}><Icon name={playing ? 'pause' : 'play'} size={20} /></button><button class="icon-button" aria-label="Next day" disabled={at === data.days.length - 1} on:click={() => step(1)}><Icon name="next" size={17} /></button></div><div class="timeline-day"><span class="eyebrow">DAY</span><strong>{String(day.epoch).padStart(2, '0')} <small>/ {data.days.at(-1).epoch}</small></strong></div><div class="timeline-scrubber"><div class="activity-bars">{#each activity as count, i}<button class:past={i <= at} class:here={i === at} style:height="{4 + count / maxActivity * 17}px" title={`Day ${data.days[i].epoch} · ${count} recorded action${count === 1 ? "" : "s"}`} aria-label={`Go to day ${data.days[i].epoch}`} on:click={() => { pause(); seek(i); }}></button>{/each}</div><input type="range" min="0" max={data.days.length - 1} value={at} aria-label="Timeline day" aria-valuetext={`Day ${day.epoch}`} style="--progress: {progress}%" on:input={(e) => { pause(); seek(e.target.value); }} /><div class="timeline-endpoints"><span>DAY {data.days[0].epoch}</span><span class="activity-caption">Recorded actions <span class="g" use:hint={"Generated by the model playing this person: one run's sample, never a measurement of how people behave."}>ⓖ</span></span><span>DAY {data.days.at(-1).epoch}</span></div></div><button class="speed-button" aria-label={`Playback speed ${speed} times. Change speed`} on:click={changeSpeed}>{speed}×<span>speed</span></button></div>
      <div class="timeline-bottom"><div class="timeline-label" use:hint={"Days the tape itself marks: the first snapshot, the first backing written, the first default, the first crisis in the economy, and the last snapshot. Click one to go there."}><Icon name="activity" size={13} />DAYS WORTH SEEING</div><div class="milestones">{#each points as point}<button class:reached={at >= point.at} title={`Jump to day ${data.days[point.at].epoch}`} on:click={() => { pause(); seek(point.at); }}><span class:coral={point.kind === 'default' || point.kind === 'crisis'}></span>{point.label}<small>{String(data.days[point.at].epoch).padStart(2, '0')}</small></button>{/each}</div><span class="keyboard-hint"><kbd>←</kbd><kbd>→</kbd> step <kbd>space</kbd> play</span></div>
    </section>
    <footer><span>edet {__EDET_VERSION__} <span class="footer-dot">/</span> {data.demo ? 'authored, not run' : `${data.run.model} · world seed ${data.run.world_seed}${data.run.control ? ' · honest-card control' : ''}`}</span><span>{data.demo ? '' : `${data.run.turns_used} / ${data.run.turn_budget} agent turns${data.run.ended ? ` · ${data.run.ended}` : ''}`}</footer>
  </main>
</div>
{#if $tip}
  <div class="tip" style="left: {$tip.x}px; top: {$tip.y}px">{$tip.text}</div>
{/if}

<style>
  .app-shell {
    max-width: 1800px;
    margin: 0 auto;
  }
  .topbar {
    min-height: 76px;
    padding: 0 38px;
    border-bottom: 1px solid var(--line);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 22px;
  }
  .brand {
    color: var(--text);
    text-decoration: none;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .brand-symbol {
    color: var(--mint);
    display: flex;
  }
  .brand strong {
    font-size: 21px;
    font-weight: 620;
    letter-spacing: -0.9px;
  }
  .brand-divider {
    width: 1px;
    height: 18px;
    background: var(--line);
    margin: 0 4px;
  }
  .brand-edition {
    font-family: var(--mono);
    font-size: 9px;
    letter-spacing: 1.7px;
    color: var(--muted);
  }
  .main-nav {
    align-self: stretch;
    display: flex;
    gap: 25px;
  }
  .main-nav button {
    display: flex;
    align-items: center;
    gap: 7px;
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    padding: 0 0 1px;
    font-size: 11px;
    color: var(--muted);
  }
  .main-nav button.current {
    color: var(--mint);
    border-bottom-color: var(--mint);
  }
  .topbar-actions {
    display: flex;
    align-items: center;
    gap: 13px;
  }
  .source-badge {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--mint);
    font-size: 10px;
    white-space: nowrap;
  }
  .source-badge > span {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: currentColor;
  }
  .source-badge.demo {
    color: var(--gold);
  }
  /* The run picker is a button that happens to hold a menu: the same border,
     radius and type as `.button`, the native select made invisible inside it
     so the menu itself stays the platform's, and a chevron of our own. */
  .run-pick {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 0 9px 0 11px;
    height: 32px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: color-mix(in srgb, var(--text) 3%, transparent);
    color: var(--secondary);
    cursor: pointer;
    transition: border-color 120ms ease, background 120ms ease;
  }
  .run-pick:hover {
    background: color-mix(in srgb, var(--text) 8%, transparent);
    border-color: color-mix(in srgb, var(--text) 30%, transparent);
    color: var(--text);
  }
  .run-pick:focus-within {
    border-color: var(--mint);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mint) 25%, transparent);
    color: var(--text);
  }
  .run-pick.busy {
    opacity: 0.55;
    cursor: progress;
  }
  .run-pick > svg {
    color: var(--mint);
  }
  .run-pick select {
    appearance: none;
    -webkit-appearance: none;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    font-size: 11px;
    font-weight: 500;
    letter-spacing: 0.1px;
    padding: 0 18px 0 0;
    margin: 0;
    max-width: 28ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: inherit;
    outline: none;
  }
  .run-pick select:disabled {
    cursor: inherit;
  }
  /* The menu is the platform's; only its ground follows the theme. */
  .run-pick option {
    background: var(--panel);
    color: var(--text);
    font-size: 12px;
  }
  .run-pick-chevron {
    position: absolute;
    right: 8px;
    top: 50%;
    display: inline-flex;
    transform: translateY(-50%) rotate(90deg);
    color: var(--muted);
    pointer-events: none;
    transition: color 120ms ease;
  }
  .run-pick:hover .run-pick-chevron,
  .run-pick:focus-within .run-pick-chevron {
    color: var(--text);
  }
  main {
    padding: 34px 38px 0;
  }
  .intro {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    margin-bottom: 28px;
  }
  .intro-eyebrow {
    display: flex;
    gap: 8px;
    align-items: center;
    color: var(--mint);
    font-size: 9px;
    letter-spacing: 2px;
    font-family: var(--mono);
  }
  h1 {
    font-size: clamp(28px, 2.55vw, 41px);
    font-weight: 450;
    letter-spacing: -1.35px;
    margin: 11px 0 10px;
  }
  .intro p {
    color: var(--secondary);
    font-size: 12px;
  }
  .intro-actions {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 16px;
  }
  .snapshot-label {
    font-family: var(--mono);
    font-size: 9px;
    letter-spacing: 1px;
    color: var(--secondary);
    display: flex;
    gap: 7px;
    align-items: center;
  }
  .snapshot-label strong {
    color: var(--secondary);
    margin-left: 5px;
    font-size: 10px;
    font-weight: 500;
  }
  .present-button {
    padding: 8px 11px;
  }
  kbd {
    font-family: var(--mono);
    border: 1px solid var(--line);
    color: var(--muted);
    padding: 1px 4px;
    border-radius: 3px;
    font-size: 9px;
  }
  .metrics {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 14px;
    margin-bottom: 22px;
  }
  .metric {
    padding: 17px 19px 16px;
    border-radius: 9px;
  }
  .metric-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    color: var(--secondary);
    font-size: 11px;
  }
  .metric-icon {
    display: flex;
  }
  .mint {
    color: var(--mint);
  }
  .gold {
    color: var(--gold);
  }
  .lavender {
    color: var(--lavender);
  }
  .metric-value {
    display: flex;
    align-items: baseline;
    font-size: 29px;
    font-weight: 450;
    letter-spacing: -0.8px;
    margin: 12px 0 9px;
    gap: 6px;
    white-space: nowrap;
  }
  .metric-value > span {
    font-size: 10px;
    color: var(--muted);
    letter-spacing: 0;
  }
  .metric-value svg {
    margin-left: auto;
    width: 69px;
    height: 28px;
    align-self: center;
    opacity: 0.85;
    min-width: 28px;
  }
  .metric-note {
    color: var(--muted);
    font-size: 10px;
  }
  .community-layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 325px;
    gap: 18px;
  }
  .inspector {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 620px;
    overflow: hidden;
  }
  .inspector-heading {
    padding: 23px 23px 19px;
  }
  .inspector-heading h2 {
    margin: 8px 0;
  }
  .day-summary {
    margin: 0 18px 17px;
    padding: 13px;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: color-mix(in srgb, var(--text) 4%, transparent);
  }
  .summary-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 10px;
    color: var(--text);
  }
  .regime {
    font-size: 8px;
    margin-left: auto;
    color: var(--muted);
  }
  .regime.pressure {
    color: var(--coral);
  }
  .day-summary > p {
    color: var(--secondary);
    line-height: 1.7;
    font-size: 11px;
    margin-top: 9px;
  }
  .inspector-tabs {
    display: flex;
    padding: 0 20px;
    border-bottom: 1px solid var(--line);
    gap: 22px;
  }
  .inspector-tabs button {
    padding: 0 0 12px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--muted);
    font-size: 11px;
    display: flex;
    gap: 7px;
    align-items: center;
  }
  .inspector-tabs button.active {
    border-bottom-color: var(--mint);
    color: var(--text);
  }
  .inspector-tabs button > span:not(.g) {
    font-family: var(--mono);
    font-size: 9px;
  }
  .inspector-scroll {
    overflow: auto;
    flex: 1;
    min-height: 0;
  }
  .inspector-footer {
    margin: 0 20px;
    padding: 16px 0;
    border-top: 1px solid var(--line);
    display: flex;
    align-items: center;
    gap: 11px;
    color: var(--muted);
  }
  .inspector-footer p {
    color: var(--muted);
    font-size: 10px;
    line-height: 1.65;
    flex: 1;
  }
  .inspector-top {
    padding: 16px 17px;
    display: flex;
    justify-content: space-between;
    border-bottom: 1px solid var(--line);
  }
  .back-button {
    color: var(--secondary);
  }
  .person-scroll {
    padding: 20px;
  }
  .people-search {
    margin: 14px 18px 4px;
    display: flex;
    gap: 8px;
    align-items: center;
    background: color-mix(in srgb, var(--ink) 40%, transparent);
    border: 1px solid var(--line);
    padding: 8px 10px;
    border-radius: 5px;
    color: var(--muted);
  }
  .people-search input {
    width: 100%;
    min-width: 0;
    border: none;
    background: transparent;
    font-size: 11px;
    color: var(--text);
    outline: none;
  }
  .people-search:focus-within {
    border-color: var(--mint);
  }
  .people-list {
    padding: 0 13px 10px;
  }
  .person-row {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 12px 5px;
    text-align: left;
    border: 0;
    border-bottom: 1px solid color-mix(in srgb, var(--line) 4%, transparent);
    background: transparent;
  }
  .person-row:hover {
    background: color-mix(in srgb, var(--mint) 6%, transparent);
  }
  .person-row-name {
    flex: 1;
    min-width: 0;
  }
  .person-row-name > strong {
    display: block;
    font-weight: 500;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .person-row-name > span {
    display: block;
    color: var(--muted);
    font-size: 9px;
    margin-top: 5px;
  }
  .person-capacity {
    font-family: var(--mono);
    font-size: 10px;
    color: var(--secondary);
    text-align: right;
  }
  .person-capacity small {
    display: block;
    font-family: inherit;
    font-size: 8px;
    margin-top: 5px;
    color: var(--muted);
  }
  .people-count {
    padding: 4px 18px 8px;
    font-size: 9px;
    color: var(--muted);
  }
  .people-count span {
    text-decoration: underline dotted;
    cursor: help;
  }
  .person-row.waiting .person-row-name > strong {
    color: var(--secondary);
    font-weight: 400;
  }
  .person-row.waiting:hover {
    background: color-mix(in srgb, var(--muted) 8%, transparent);
  }
  .avatar.waiting {
    background: transparent;
    color: var(--muted);
    border-style: dashed;
  }
  .context-strip {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 16px;
    padding: 17px 2px 20px;
    color: var(--muted);
    font-size: 10px;
  }
  .context-strip > div {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .context-strip strong {
    color: var(--secondary);
    font-weight: 500;
  }
  .context-divider {
    background: var(--line);
    width: 1px;
    height: 12px;
  }
  .pressure-dot {
    width: 5px;
    height: 5px;
    background: var(--mint);
    border-radius: 50%;
  }
  .pressure-dot.affected {
    background: var(--gold);
  }
  .timeline {
    overflow: hidden;
  }
  .timeline-main {
    display: flex;
    align-items: center;
    gap: 22px;
    padding: 20px 22px 15px;
  }
  .playback-buttons {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .play-button {
    border: none;
    width: 38px;
    height: 38px;
    background: var(--mint);
    color: var(--on-accent);
    border-radius: 50%;
    display: flex;
    justify-content: center;
    align-items: center;
  }
  .timeline-day {
    border-left: 1px solid var(--line);
    padding-left: 20px;
    min-width: 105px;
  }
  .timeline-day > span {
    font-size: 8px;
  }
  .timeline-day strong {
    display: block;
    font-family: var(--mono);
    font-size: 22px;
    font-weight: 400;
    margin-top: 5px;
  }
  .timeline-day small {
    font-size: 10px;
    color: var(--muted);
  }
  .timeline-scrubber {
    flex: 1;
    min-width: 0;
    padding-top: 0;
  }
  .activity-bars {
    height: 21px;
    display: flex;
    gap: 3px;
    align-items: flex-end;
    padding: 0 6px;
  }
  .activity-bars > button {
    flex: 1;
    min-width: 1px;
    background: var(--line);
    border-radius: 1px 1px 0 0;
  }
  .activity-bars > button.past {
    background: var(--muted);
  }
  .timeline-scrubber input {
    appearance: none;
    display: block;
    height: 3px;
    width: 100%;
    margin: 5px 0 9px;
    background: linear-gradient(to right, var(--mint) var(--progress), var(--line) var(--progress));
    border-radius: 3px;
    cursor: pointer;
  }
  .timeline-scrubber input::-webkit-slider-thumb {
    appearance: none;
    height: 10px;
    width: 10px;
    background: var(--gold);
    border-radius: 50%;
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--mint) 11%, transparent);
  }
  .timeline-scrubber input::-moz-range-thumb {
    height: 10px;
    width: 10px;
    border: 0;
    background: var(--gold);
    border-radius: 50%;
  }
  .timeline-endpoints {
    display: flex;
    justify-content: space-between;
    font-family: var(--mono);
    font-size: 8px;
    color: var(--secondary);
  }
  .activity-caption {
    font-family: inherit;
    font-size: 8px;
  }
  .activity-caption .g {
    font-size: 9px;
  }
  .speed-button {
    border: 1px solid var(--line);
    background: color-mix(in srgb, var(--text) 5%, transparent);
    border-radius: 5px;
    padding: 6px 13px;
    font-family: var(--mono);
    color: var(--text);
    font-size: 12px;
  }
  .speed-button span {
    display: block;
    font-size: 8px;
    color: var(--secondary);
    margin-top: 3px;
  }
  .timeline-bottom {
    display: flex;
    align-items: center;
    gap: 20px;
    padding: 13px 22px;
    border-top: 1px solid var(--line);
    background: color-mix(in srgb, var(--ink) 14%, transparent);
  }
  .timeline-label {
    display: flex;
    gap: 7px;
    align-items: center;
    font-size: 8px;
    font-family: var(--mono);
    letter-spacing: 0.6px;
    color: var(--muted);
    white-space: nowrap;
  }
  .milestones {
    display: flex;
    gap: 20px;
    flex-wrap: wrap;
  }
  .milestones button {
    padding: 0;
    background: transparent;
    border: none;
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--muted);
    font-size: 9px;
  }
  .milestones button.reached {
    color: var(--secondary);
  }
  .milestones button > span {
    border-radius: 50%;
    background: var(--muted);
    width: 4px;
    height: 4px;
  }
  .milestones button > span.coral {
    background: var(--gold);
  }
  .milestones button small {
    font-family: var(--mono);
    color: var(--muted);
    margin-left: 2px;
    font-size: 8px;
  }
  .keyboard-hint {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-left: auto;
    font-size: 9px;
    color: var(--muted);
    white-space: nowrap;
  }
  .keyboard-hint kbd:last-child {
    margin-left: 5px;
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 15px;
    padding: 19px 0 22px;
    font-size: 9px;
    color: var(--muted);
  }
  footer > span {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 5px;
  }
  .footer-dot {
    padding: 0 5px;
    color: var(--muted);
  }
  .notice {
    display: flex;
    gap: 10px;
    align-items: center;
    background: color-mix(in srgb, var(--gold) 6%, transparent);
    color: var(--gold);
    border: 1px solid color-mix(in srgb, var(--gold) 15%, transparent);
    border-radius: 6px;
    padding: 10px 14px;
    margin-bottom: 20px;
    font-size: 12px;
  }
  .notice.error {
    color: var(--coral);
  }
  .notice > span {
    flex: 1;
  }
  .about {
    position: relative;
    display: grid;
    grid-template-columns: 1.1fr 1fr 1fr;
    padding: 25px;
    gap: 30px;
    margin-bottom: 28px;
  }
  .about h2 {
    margin-top: 8px;
  }
  .about p {
    color: var(--secondary);
    line-height: 1.7;
    font-size: 11px;
    margin: 10px 0;
  }
  .about strong {
    font-weight: 500;
    font-size: 12px;
  }
  .about-close {
    position: absolute;
    top: 5px;
    right: 5px;
  }
  .ledger-layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 300px;
    gap: 18px;
    margin-bottom: 22px;
  }
  .contracts-panel {
    overflow: hidden;
    min-width: 0;
  }
  .register-panel {
    overflow: hidden;
    min-width: 0;
    margin-bottom: 18px;
  }
  /* The heading above it is inset 24px; the note keeps the same left edge
     and closes the gap the heading's own padding leaves under it. */
  .register-note {
    font-size: 11px;
    line-height: 1.6;
    max-width: 80ch;
    margin: -10px 0 0;
    padding: 0 24px 16px;
  }
  .register-scroll {
    max-height: 420px;
    overflow: auto;
  }
  .life-cell {
    font-size: 11px;
    white-space: normal;
    min-width: 26ch;
  }
  .life-step small {
    color: var(--muted);
    margin-left: 2px;
  }
  .register-panel tr.later {
    opacity: 0.45;
  }
  .section-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 24px;
  }
  .section-heading h2 {
    margin-top: 8px;
  }
  .section-heading select {
    /* A native select paints its own list, and a transparent background with
       no colour of its own leaves white text on white in the dark theme.
       Both need saying, and the options need it said again. */
    background: var(--panel);
    color: var(--text);
    color-scheme: inherit;
    border: 1px solid var(--line);
    padding: 8px 9px;
    border-radius: 5px;
    font-size: 11px;
  }
  .section-heading select option {
    background: var(--panel);
    color: var(--text);
  }
  .table-scroll {
    overflow: auto;
    max-height: 485px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 11px;
    text-align: left;
    white-space: nowrap;
  }
  th {
    padding: 13px 20px;
    background: color-mix(in srgb, var(--text) 4%, transparent);
    font-size: 9px;
    font-weight: 500;
    color: var(--muted);
    border-block: 1px solid var(--line);
    position: sticky;
    top: 0;
  }
  td {
    padding: 15px 20px;
    border-bottom: 1px solid color-mix(in srgb, var(--line) 4%, transparent);
    color: var(--secondary);
  }
  td:nth-child(2) > span:last-child {
    font-size: 10px;
  }
  .numeric,
  .contract-id {
    font-family: var(--mono);
  }
  .contract-arrow {
    margin: 0 8px;
    color: var(--muted);
  }
  .contract-status {
    text-transform: capitalize;
    color: var(--secondary);
    background: color-mix(in srgb, var(--mint) 4%, transparent);
    border: 1px solid color-mix(in srgb, var(--mint) 10%, transparent);
    padding: 4px 7px;
    border-radius: 4px;
    font-size: 9px;
  }
  .contract-status.expired {
    color: var(--coral);
    background: color-mix(in srgb, var(--coral) 6%, transparent);
    border-color: color-mix(in srgb, var(--coral) 13%, transparent);
  }
  .audit-panel {
    padding: 25px;
  }
  .audit-icon {
    width: 50px;
    height: 50px;
    color: var(--mint);
    display: flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in srgb, var(--mint) 5%, transparent);
    border-radius: 12px;
    border: 1px solid color-mix(in srgb, var(--mint) 13%, transparent);
    margin-bottom: 24px;
  }
  .audit-icon.failed {
    color: var(--coral);
  }
  .audit-panel h2 {
    margin: 10px 0;
  }
  .audit-panel > p {
    color: var(--secondary);
    font-size: 11px;
    line-height: 1.8;
    margin-bottom: 28px;
  }
  .audit-stat {
    border-top: 1px solid var(--line);
    display: flex;
    justify-content: space-between;
    padding: 14px 0;
    font-size: 10px;
    color: var(--muted);
  }
  .audit-stat strong {
    font-weight: 500;
    color: var(--secondary);
  }
  .violation {
    background: color-mix(in srgb, var(--coral) 5%, transparent);
    border: 1px solid color-mix(in srgb, var(--coral) 13%, transparent);
    border-radius: 6px;
    padding: 12px;
    margin-bottom: 12px;
    font-size: 11px;
    overflow-wrap: anywhere;
  }
  .violation strong {
    display: block;
    color: var(--coral);
    margin: 8px 0;
  }
  .violation span {
    color: var(--secondary);
  }
  .violation pre {
    white-space: pre-wrap;
    font-size: 10px;
  }
  .violation summary {
    margin-top: 10px;
    cursor: pointer;
  }

@media (min-width: 1550px) {
    .community-layout {
      grid-template-columns: minmax(0, 1fr) 360px;
    }
    .inspector {
      height: 660px;
    }
    .metric-value {
      font-size: 32px;
    }
    .metric-value svg {
      width: 95px;
    }
  }
  @media (max-width: 1150px) {
    .topbar {
      padding: 0 24px;
      gap: 18px;
    }
    .brand-edition,
    .brand-divider {
      display: none;
    }
    .main-nav {
      gap: 18px;
    }
    main {
      padding: 28px 24px 0;
    }
    .community-layout {
      grid-template-columns: minmax(0, 1fr) 300px;
    }
    .metric {
      padding: 15px;
    }
    .metric-value {
      font-size: 26px;
    }
    .metric-value svg {
      width: 42px;
    }
    .metric-value > span {
      font-size: 9px;
    }
    .timeline-bottom {
      flex-wrap: wrap;
      gap: 13px;
    }
    .keyboard-hint {
      display: none;
    }
    .milestones {
      gap: 16px;
    }
    .context-strip {
      gap: 12px;
    }
    footer {
      align-items: flex-start;
    }
  }
  @media (max-width: 900px) {
    .topbar {
      flex-wrap: wrap;
      padding-top: 17px;
      padding-bottom: 0;
      gap: 16px;
    }
    .main-nav {
      order: 3;
      width: 100%;
      justify-content: center;
      height: 40px;
      gap: 38px;
    }
    .topbar-actions {
      margin-left: auto;
    }
    .metrics {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: 10px;
    }
    .metric-value svg {
      width: 100px;
    }
    .community-layout {
      grid-template-columns: minmax(0, 1fr);
    }
    .inspector {
      height: 430px;
    }
    .inspector-heading {
      padding: 20px;
    }
    .inspector-footer {
      display: none;
    }
    .day-summary {
      margin-bottom: 13px;
    }
    .context-strip {
      justify-content: center;
      padding: 20px 0;
    }
    .ledger-layout {
      grid-template-columns: 1fr;
    }
    .audit-panel {
      padding: 22px;
    }
    .timeline-main {
      gap: 14px;
      padding: 18px 15px 13px;
    }
    .timeline-day {
      min-width: 86px;
      padding-left: 15px;
    }
    .about {
      grid-template-columns: 1fr;
      gap: 17px;
    }
    footer {
      flex-direction: column;
      gap: 9px;
    }
}
  @media (max-width: 550px) {
    .topbar {
      padding-inline: 16px;
    }
    .brand strong {
      font-size: 20px;
    }
    .topbar-actions {
      gap: 9px;
    }
    .source-badge {
      font-size: 8px;
    }
    .run-pick {
      height: 28px;
      padding: 0 7px 0 9px;
      gap: 6px;
    }
    .run-pick select {
      font-size: 10px;
      max-width: 16ch;
    }
    .help-button {
      display: none;
    }
    .main-nav {
      gap: 30px;
    }
    .main-nav button {
      font-size: 10px;
    }
    main {
      padding: 25px 16px 0;
    }
    .intro {
      align-items: flex-start;
      gap: 12px;
      margin-bottom: 23px;
    }
    .intro h1 {
      font-size: 28px;
      letter-spacing: -0.9px;
    }
    .intro p {
      font-size: 11px;
      line-height: 1.8;
    }
    .intro-eyebrow {
      font-size: 8px;
      letter-spacing: 1.2px;
    }
    .intro-actions {
      padding-top: 23px;
    }
    .snapshot-label {
      display: none;
    }
    .present-button {
      font-size: 0;
      padding: 8px;
      gap: 0;
    }
    .metrics {
      margin-bottom: 15px;
    }
    .metric {
      padding: 13px;
    }
    .metric-top {
      font-size: 10px;
    }
    .metric-value {
      font-size: 25px;
      flex-wrap: wrap;
      position: relative;
      gap: 5px;
    }
    .metric-value svg {
      position: absolute;
      right: 0;
      bottom: 0;
      width: 34px;
      height: 20px;
      opacity: 0.6;
    }
    .metric-value > span {
      display: block;
      width: 100%;
      font-size: 8px;
    }
    .metric-note {
      font-size: 9px;
      line-height: 1.6;
    }
    .context-strip {
      font-size: 9px;
      gap: 11px;
    }
    .context-divider {
      display: none;
    }
    .timeline-main {
      flex-wrap: wrap;
      gap: 16px;
    }
    .timeline-day {
      flex: 1;
    }
    .timeline-scrubber {
      order: 4;
      flex-basis: 100%;
    }
    .timeline-bottom {
      padding: 13px 15px;
    }
    .timeline-label {
      width: 100%;
    }
    .milestones {
      gap: 12px 20px;
    }
    .activity-bars {
      gap: 2px;
    }
    footer {
      font-size: 8px;
      line-height: 1.7;
    }
    .section-heading {
      padding: 18px;
    }
    .section-heading h2 {
      font-size: 15px;
    }
  }
  @media (min-width: 901px) {
}
  .intro-buttons {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .play-story {
    padding: 8px 11px;
  }
  @media (max-width: 550px) {
    .play-story { display: none; }
  }

  .contract-panel h2 small { font-weight: 400; opacity: 0.6; font-size: 0.6em; }
  .contract-life { display: flex; flex-direction: column; gap: 0.75rem; margin-top: 1rem; }
  .life-step { display: flex; flex-direction: column; gap: 0.2rem; padding-left: 0.75rem; border-left: 2px solid var(--line, rgba(255,255,255,0.12)); }
  .life-step strong { font-weight: 500; }
  .life-act { font-size: 0.82rem; opacity: 0.8; }
  tr.open { background: color-mix(in srgb, var(--text) 6%, transparent); }

  .activity-bars > button {
    border: 0; padding: 0; cursor: pointer; background: var(--line);
    transition: background 0.12s ease, transform 0.12s ease;
  }
  .activity-bars > button:hover { background: var(--mint); transform: scaleY(1.25); }
  .activity-bars > button.here { background: var(--mint); }
  .brand-symbol { display: block; border-radius: 7px; }

  .refusal-block { margin-top: 1.25rem; border-top: 1px solid var(--line); padding-top: 1rem; }
  .refusal { display: flex; flex-direction: column; gap: 0.15rem; padding: 0.4rem 0; border-bottom: 1px solid var(--line); font-size: 0.85rem; }
  .refusal code { font-family: var(--mono); color: var(--coral); }
  /* A code with a sentence behind it: the wallet's own words, on hover. */
  .refusal-aside { margin-top: 0.9rem; }
  .refusal :global(code.has-hint) {
    cursor: help;
    border-bottom: 1px dotted color-mix(in srgb, var(--coral) 55%, transparent);
  }
  .refusal-says { color: var(--muted); font-size: 0.8rem; }

  .flow-block { border-top: 1px solid var(--line); margin-top: 1rem; padding-top: 0.9rem; }
  .flow-arc { display: flex; justify-content: space-between; gap: 0.5rem; padding: 0.3rem 0; border-bottom: 1px solid var(--line); font-size: 0.85rem; }

  .day-summary-people { margin: 0.35rem 0 0; font-size: 0.82rem; line-height: 1.7; }
  .day-summary-people span { color: var(--muted); }

  .anomalies-layout { display: grid; gap: 1rem; }
  .anomaly-panel { padding: 1.9rem 2.1rem 2.2rem; }
  .anomaly-preamble {
    color: var(--secondary);
    max-width: 68ch;
    margin: 0.4rem 0 1.6rem;
    font-size: 0.9rem;
    line-height: 1.8;
  }
  .anomaly-preamble em { color: var(--text); font-style: normal; }
  .anomaly-tabs { display: flex; flex-wrap: wrap; gap: 0.4rem; border-bottom: 1px solid var(--line); padding-bottom: 0.8rem; }
  .anomaly-tabs button {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    background: transparent;
    border: 1px solid var(--line);
    border-radius: 999px;
    color: var(--secondary);
    padding: 0.42rem 0.85rem;
    font-size: 0.82rem;
    cursor: pointer;
    transition: color 0.12s ease, border-color 0.12s ease, background 0.12s ease;
  }
  .anomaly-tabs button:hover { color: var(--text); }
  .anomaly-tabs button.active { color: var(--text); border-color: var(--mint); background: color-mix(in srgb, var(--mint) 12%, transparent); }
  .anomaly-tab-count { font-family: var(--mono); font-size: 0.7rem; color: var(--muted); }
  .anomaly-tabs button.has .anomaly-tab-count { color: var(--mint); }
  .anomaly-note { color: var(--muted); font-size: 0.85rem; line-height: 1.7; margin: 1.1rem 0 0.2rem; max-width: 72ch; }
  .anomaly {
    padding: 1.1rem 0 1.15rem 1.1rem;
    border-left: 2px solid transparent;
    border-bottom: 1px solid var(--line);
    opacity: 0.62;
  }
  .anomaly.found { opacity: 1; border-left-color: var(--mint); }
  .anomaly.grave { border-left-color: var(--coral); }
  .anomaly-head { display: flex; align-items: baseline; justify-content: space-between; gap: 1.5rem; }
  .anomaly-head strong { font-weight: 500; font-size: 0.97rem; }
  .anomaly.grave .anomaly-count { color: var(--coral); }
  .anomaly-count { font-size: 0.72rem; color: var(--muted); font-family: var(--mono); letter-spacing: 0.04em; white-space: nowrap; }
  /* Only the code is hoverable: the sentence around it is ordinary text. */
  .code-hint {
    font-family: var(--mono);
    color: var(--coral);
    border-bottom: 1px dotted color-mix(in srgb, var(--coral) 55%, transparent);
  }
  /* The player's own tooltip: instant, styled, and clipped by nothing. */
  .tip {
    position: fixed;
    z-index: 90;
    max-width: 328px;
    padding: 0.7rem 0.8rem;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--panel);
    box-shadow: 0 14px 38px rgba(0, 0, 0, 0.45);
    color: var(--secondary);
    font-size: 10.5px;
    line-height: 1.75;
    pointer-events: none;
  }
  :global(.has-hint) {
    cursor: help;
  }
  :global(.has-hint:focus-visible) {
    outline: 1px solid var(--mint);
    outline-offset: 2px;
  }
  .anomaly-rule { color: var(--muted); font-size: 0.85rem; margin: 0.5rem 0 0; max-width: 82ch; line-height: 1.75; }
  .anomaly-more { color: var(--muted); font-size: 0.8rem; margin: 0.6rem 0 0; }
  .anomaly-hit {
    display: flex;
    flex-wrap: wrap;
    gap: 0.6rem;
    align-items: baseline;
    margin-top: 0.75rem;
    padding-top: 0.6rem;
    border-top: 1px dashed var(--line);
    font-size: 0.87rem;
    line-height: 1.65;
  }
  .anomaly-detail { flex: 1 1 22rem; }
  .anomaly-people { color: var(--secondary); }
</style>
