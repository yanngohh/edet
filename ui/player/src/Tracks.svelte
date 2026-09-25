<script>
  import { createEventDispatcher } from "svelte";
  import { hint } from "./hint.js";
  import { money } from "./load.js";
  import { economySeries, ledgerSeries } from "./analytics.js";
  import Icon from "./Icon.svelte";
  export let days;
  export let at;
  const dispatch = createEventDispatcher();
  const width = 540,
    height = 140;
  $: series = economySeries(days).map((d, i) => ({ ...d, ...ledger[i] }));
  $: ledger = ledgerSeries(days);
  $: cursor = (at / Math.max(series.length - 1, 1)) * width;
  const pct = (n) => `${n.toFixed(0)}%`;
  const count = (n) => String(n);
  const tracks = [
    {
      field: "insuredShare",
      label: "Share of credit the community insures",
      eyebrow: "WHAT THE STAKES CARRY",
      fmt: pct,
      color: "var(--mint)",
      unit: "% of what is owed",
      icon: "shield",
      description:
        "Insured at acceptance, which only falls — measured over the AMOUNT outstanding, not the number of rows. A community starts at zero: the first trades are uninsured, and capacity is the residue of repaying them.",
    },
    {
      field: "capacity",
      label: "Capacity the community has earned",
      eyebrow: "WHAT THE STAKES CARRY",
      fmt: money,
      color: "var(--mint)",
      unit: "credit units",
      icon: "network",
      description:
        "The sum of what every member may borrow insured — a maximum flow over the stakes, which are written only by discharge.",
    },
    {
      field: "refused",
      label: "What the ledger refused",
      eyebrow: "THE FRICTION",
      fmt: count,
      color: "var(--coral)",
      unit: "acts a day",
      icon: "shield",
      description:
        "Acts a person tried and the ledger would not take — an activity limit, a bad amount, a contract in the wrong state. Refused before anything was applied.",
    },
    {
      field: "discharged",
      label: "Debt discharged",
      eyebrow: "WHAT THE LEDGER DID ITSELF",
      fmt: money,
      color: "var(--lavender)",
      unit: "credit units / day",
      icon: "link",
      description:
        "What outstanding fell by, whatever discharged it: a settlement, a cure, a part payment, netting between two people or a ring the sweep closed. The tape does not say which — an act names a contract only when it opened one — so this names no cause. Inferred by comparing snapshots.",
    },
    {
      field: "handedOn",
      label: "Debts that changed hands",
      eyebrow: "WHAT THE LEDGER DID ITSELF",
      fmt: count,
      color: "var(--lavender)",
      unit: "contracts a day",
      icon: "link",
      description:
        "A contract taken over by another debtor — a transfer, or a sale's cascade. The row closes and a new one opens in the successor's name, so this counts the closing. Inferred by comparing snapshots.",
    },
    {
      field: "substituted",
      label: "Underwriters stepping in",
      eyebrow: "WHAT THE LEDGER DID ITSELF",
      fmt: count,
      color: "var(--coral)",
      unit: "contracts a day",
      icon: "shield",
      description:
        "A contract whose creditor changed: the subrogation leg of an insured default, where the underwriter takes over the claim. The substitution leg opens new contracts instead and is not counted here. Inferred by comparing snapshots.",
    },
    {
      field: "expiredToday",
      label: "Defaults the sweep marked",
      eyebrow: "WHAT THE LEDGER DID ITSELF",
      fmt: count,
      color: "var(--coral)",
      unit: "contracts a day",
      icon: "shield",
      description:
        "Contracts the morning sweep found unpaid past maturity, whether or not a cure or an underwriter closed them again before the day ended. The close-of-day figures elsewhere miss those; this does not.",
    },
    {
      field: "pending",
      label: "Offers waiting for a signature",
      eyebrow: "WHAT IS NOT YET ON THE LEDGER",
      fmt: count,
      color: "var(--gold)",
      unit: "offers",
      icon: "activity",
      description: "Entries in the pending pool: offered, not yet signed by everybody they name, and expiring if nobody does.",
    },
    {
      field: "dueSoon",
      label: "Falling due within thirty days",
      eyebrow: "WHAT IS COMING",
      fmt: money,
      color: "var(--gold)",
      unit: "credit units",
      icon: "chart",
      description: "What live contracts owe with a maturity inside the next thirty days — what the community has to find.",
    },
    {
      field: "creditBooked",
      label: "Credit first seen",
      eyebrow: "ON THE LEDGER",
      fmt: money,
      color: "var(--mint)",
      unit: "credit units / day",
      icon: "network",
      description:
        "Original value of rows accepted for the first time. A row a handover or a cascade reopened under the same original is not counted again. The first snapshot may include earlier credit.",
    },
    {
      field: "cashPaid",
      label: "Cash paid between people",
      eyebrow: "BESIDE THE LEDGER",
      fmt: money,
      color: "var(--gold)",
      unit: "cash units / day",
      icon: "activity",
      g: true,
      description:
        "Successful cash payments and cash moved by applied actions. Income and household bills are excluded.",
    },
    {
      field: "priceIndex",
      label: "Price level",
      eyebrow: "THE ECONOMIC WEATHER",
      fmt: (x) => x.toFixed(4),
      color: "var(--lavender)",
      unit: "index · genesis = 1",
      description: "The price index in the run’s separate cash economy.",
      nonzero: true,
    },
    {
      field: "householdsCut",
      label: "Income under pressure",
      eyebrow: "HOUSEHOLD CONDITIONS",
      fmt: (x) => x,
      color: "var(--coral)",
      unit: "households with an income cut",
      description: "Households currently affected, out of the households present on that day.",
    },
    {
      field: "members",
      label: "The growing community",
      eyebrow: "MEMBERSHIP",
      fmt: (x) => x,
      color: "var(--mint)",
      unit: "ledger members",
      description: "Accounts present on the ledger, including members who are not active.",
    },
    {
      field: "cashOffers",
      label: "Offers paid in cash",
      eyebrow: "GENERATED CHOICES",
      fmt: (x) => x,
      color: "var(--gold)",
      unit: "offers / day",
      g: true,
      description:
        "Offers with cash selected as the payment method, including unsuccessful attempts.",
    },
    {
      field: "valueOffers",
      label: "Offers paid in value",
      eyebrow: "GENERATED CHOICES",
      fmt: (x) => x,
      color: "var(--mint)",
      unit: "offers / day",
      g: true,
      description:
        "Offers with value selected as the payment method, including unsuccessful attempts.",
    },
  ];
  // **What does not move with the cursor is computed once.** Every one of
  // these — the scale, the whole-run path, the centred mean — depends on the
  // run and not on the day, and rebuilding all fifteen on every arrow key was
  // ~2.5 ms of a step spent redrawing lines that had not changed.
  $: shapes = tracks.map((track) => {
    const values = series.map((s) => s[track.field]);
    // The floor is the series' own least value, never a number below it:
    // a price index floored at 0.999 read as a fall that never happened.
    const lo = track.nonzero ? Math.min(...values) : 0;
    const hi = Math.max(...values, lo + (track.nonzero ? 0.001 : 1));
    const y = (value) => height - 8 - ((value - lo) / (hi - lo)) * (height - 16);
    const path = (end) =>
      values
        .slice(0, end)
        .map((v, i) => `${i ? "L" : "M"}${(i / Math.max(values.length - 1, 1)) * width},${y(v)}`)
        .join(" ");
    // A day's figure jumps about; what a reader wants beside it is the level
    // it is jumping around. A centred mean over a window that scales with the
    // run — a week of a short one, wider on a long one — drawn faintly so it
    // never competes with the day itself.
    const span = Math.max(2, Math.round(values.length / 14));
    const smooth = values.map((_, i) => {
      const from = Math.max(0, i - span), to = Math.min(values.length - 1, i + span);
      let sum = 0;
      for (let k = from; k <= to; k++) sum += values[k];
      return sum / (to - from + 1);
    });
    const smoothPath = smooth
      .map((v, i) => `${i ? "L" : "M"}${(i / Math.max(values.length - 1, 1)) * width},${y(v)}`)
      .join(" ");
    return { ...track, lo, hi, values, y, path, all: path(values.length), mean: smoothPath, smooth };
  });
  // **No empty cell at the end of the grid.** Sixteen tracks in three
  // columns leave one alone on the last row; the first few are drawn two to a
  // row instead, as many as it takes for the rest to fill three-column rows
  // exactly: four of them when the count is one past a multiple of three,
  // two when it is two past. In the two-column layout the first track spans
  // the row when the count is odd.
  $: wide = shapes.length % 3 === 1 ? 4 : shapes.length % 3 === 2 ? 2 : 0;
  $: oddPair = shapes.length % 2 === 1;
  $: charts = shapes.map((shape) => ({
    ...shape,
    past: shape.path(at + 1),
    meanNow: shape.smooth[at],
    y: shape.y(shape.values[at]),
    value: shape.values[at],
  }));
  function seek(e) {
    if (!e.detail) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const index = Math.round(((e.clientX - rect.left) / rect.width) * (days.length - 1));
    dispatch("seek", Math.max(0, Math.min(days.length - 1, index)));
  }
  function key(e) {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
    e.preventDefault();
    e.stopPropagation();
    dispatch(
      "seek",
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? days.length - 1
          : Math.max(0, Math.min(days.length - 1, at + (e.key === "ArrowLeft" ? -1 : 1))),
    );
  }
</script>

<div class="economy-intro"><span><Icon name="info" size={15} />Credit units and cash are distinct. Each chart has its own labeled scale.</span></div>
<div class="tracks">
  {#each charts as chart, i}
    <section class="track panel" class:wide={i < wide} class:lead={oddPair && i === 0} style="--chart-color: {chart.color}">
      <div class="track-header"><div><div class="eyebrow">{chart.eyebrow}</div><h2>{chart.label} {#if chart.g}<span class="g" use:hint={"A sample of generated decisions, not a behavioral measurement"}>ⓖ</span>{/if}</h2></div><span class="chart-icon"><Icon name={chart.icon || 'chart'} size={18} /></span></div>
      <div class="reading"><strong>{chart.fmt(chart.value)}</strong><span>{chart.unit}</span></div>
      <div class="chart-axis"><span>{chart.fmt(chart.hi)}</span></div>
      <button class="chart" on:click={seek} on:keydown={key} aria-label={`${chart.label}: ${chart.fmt(chart.value)} on day ${days[at].epoch}. Use left and right arrows to change day.`}>
        <svg viewBox="-5 -3 {width + 10} {height + 6}" preserveAspectRatio="none" aria-hidden="true">
          {#each [8, height / 2, height - 8] as y}<line x1="0" x2={width} y1={y} y2={y} stroke="color-mix(in srgb, var(--line) 7%, transparent)" stroke-dasharray="3 5" />{/each}
          <path d={chart.all} fill="none" stroke={chart.color} stroke-opacity=".2" stroke-width="1.5" vector-effect="non-scaling-stroke" />
          <path d={chart.mean} fill="none" stroke="var(--text)" stroke-opacity=".22" stroke-width="2.5" stroke-linecap="round" vector-effect="non-scaling-stroke" />
          <path d={`${chart.past} L${cursor},${height} L0,${height} Z`} fill={chart.color} fill-opacity=".055" />
          <path d={chart.past} fill="none" stroke={chart.color} stroke-width="1.8" vector-effect="non-scaling-stroke" />
          <line x1={cursor} x2={cursor} y1="0" y2={height} stroke={chart.color} stroke-opacity=".4" stroke-dasharray="3 4" />
          <circle cx={cursor} cy={chart.y} r="4" fill={chart.color} stroke="var(--panel)" stroke-width="2" />
        </svg>
      </button>
      <div class="chart-axis"><span>{chart.fmt(chart.lo)}</span><span class="axis-days"><span>DAY {days[0].epoch}</span><span class="axis-now">DAY {days[at].epoch}</span><span>DAY {days.at(-1).epoch}</span></span></div>
      <p class="description">{chart.description}</p>
    </section>
  {/each}
</div>

<style>
  .economy-intro {
    display: flex;
    justify-content: space-between;
    gap: 15px;
    margin: 4px 0 20px;
    font-size: 10px;
    color: var(--muted);
  }
  .economy-intro > span {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .tracks {
    display: grid;
    /* Six columns, so a track can take a third (span 2) or a half (span 3)
       of the row and the two shapes share one grid. */
    grid-template-columns: repeat(6, minmax(0, 1fr));
    gap: 16px;
    margin-bottom: 24px;
  }
  .track {
    grid-column: span 2;
    padding: 23px;
  }
  .track.wide {
    grid-column: span 3;
  }
  .track-header {
    display: flex;
    justify-content: space-between;
    gap: 10px;
  }
  .track-header h2 {
    margin-top: 8px;
    font-size: 15px;
  }
  .chart-icon {
    color: var(--chart-color);
  }
  .reading {
    display: flex;
    align-items: baseline;
    gap: 7px;
    margin: 22px 0 18px;
    flex-wrap: wrap;
  }
  .reading strong {
    font-size: 25px;
    font-weight: 450;
    letter-spacing: -0.6px;
  }
  .reading > span {
    font-size: 9px;
    color: var(--muted);
  }
  .reading > small {
    margin-left: auto;
    font-family: var(--mono);
    font-size: 8px;
    color: var(--muted);
  }
  .chart {
    display: block;
    padding: 0;
    border: 0;
    background: transparent;
    width: 100%;
    cursor: crosshair;
  }
  .chart svg {
    width: 100%;
    height: 132px;
    display: block;
  }
  .chart-axis {
    display: flex;
    justify-content: space-between;
    color: var(--muted);
    font-family: var(--mono);
    font-size: 8px;
    padding-block: 7px;
  }
  .chart-axis > span:last-child {
    text-align: right;
    flex: 1;
    margin-left: 15px;
  }
  .axis-days {
    display: inline-flex;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
  }
  .axis-now {
    color: var(--chart-color);
    font-weight: 500;
  }
  .description {
    font-size: 10px;
    line-height: 1.7;
    color: var(--muted);
    border-top: 1px solid var(--line);
    padding-top: 12px;
    margin-top: 6px;
  }
  @media (max-width: 1100px) {
    .tracks {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .track,
    .track.wide {
      grid-column: span 1;
    }
    .track.lead {
      grid-column: 1 / -1;
    }
    .economy-intro {
      flex-direction: column;
    }
    .track {
      padding: 20px;
    }
  }
  @media (max-width: 600px) {
    .tracks {
      grid-template-columns: 1fr;
    }
    .track,
    .track.wide,
    .track.lead {
      grid-column: auto;
    }
    .track-header h2 {
      font-size: 16px;
    }
    .reading strong {
      font-size: 28px;
    }
    .economy-intro {
      line-height: 1.7;
    }
  }
</style>
