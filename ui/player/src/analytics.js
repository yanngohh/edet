// Ledger arithmetic is kept separate from counts of generated decisions.
export function snapshotStats(day, run = null) {
  const contracts = day.contracts;
  const live = contracts.filter((c) => c.status === "active" || c.status === "expired");
  const town = run ? townOn(run, day) : [];
  return {
    members: day.members.length,
    // **The town is larger than the ledger.** `members` is the ledger's own
    // count; `people` is everybody who exists by this day and `waiting` those
    // of them with no account yet. pilot-3 had 15 members and 128 people, and
    // a player that listed only the ledger's rows hid 113 of them.
    people: town.length,
    waiting: town.filter((p) => !day.members.some((m) => m.person === p.index)).length,
    seed: day.seed,
    outstanding: live.reduce((sum, c) => sum + c.outstanding, 0),
    active: contracts.filter((c) => c.status === "active").length,
    defaults: contracts.filter((c) => c.status === "expired").length,
    // **What the sweep marked expired this day**, whatever became of it by
    // the close. A snapshot is the day's close, and a default cured the same
    // day — the insured path, where an underwriter takes the claim over and
    // is paid — never shows in one; the indexer reads it at the opening. A
    // tape indexed before this carries no such list and reads as zero.
    expiredToday: (day.expired_today || []).length,
    closed: contracts.filter((c) => c.status === "settled" || c.status === "cured").length,
    // A transfer CLOSES its row (`status = transferred`, outstanding 0) and
    // opens a new contract for the successor, so a transferred row is neither
    // live nor settled. Counted on its own, or the totals do not add up.
    transferred: contracts.filter((c) => c.status === "transferred").length,
    stakes: day.edges.length,
    insured: live.filter((c) => c.insured).length,
    live: live.length,
  };
}

/** The day number a tick is, as the people were told it. */
export const dayOfTick = (run, tick) => (run?.genesis_epoch || 0) + (tick || 0);

/** Everybody who exists in the town by this day, whether or not the ledger
 *  has a row for them: a person is introduced the day somebody first offers
 *  them a trade, and stays in the town whatever comes of it. */
export function townOn(run, day) {
  return (run?.persons || []).filter((p) => (p.joined_tick ?? 0) <= (day.tick ?? 0));
}

/**
 * **The people list: members first, then everybody still waiting.**
 *
 * A member row carries its ledger row; a waiting row carries nothing of the
 * ledger's, because the ledger knows nothing of them — what it has is the
 * household's purse, the day they were introduced and who introduced them.
 * Both kinds are one shape so the list can be one list.
 */
export function peopleOn(run, day) {
  const purses = new Map((day.purses || []).map((p) => [p.person, p]));
  const seated = new Set(day.members.map((m) => m.person).filter((p) => p != null));
  const members = [...day.members]
    .sort((a, b) => a.id - b.id)
    .map((m) => ({ person: m.person, row: m, purse: purses.get(m.person) ?? null, waiting: false, since: null, sponsor: null }));
  const waiting = townOn(run, day)
    .filter((p) => !seated.has(p.index))
    .sort((a, b) => (a.joined_tick ?? 0) - (b.joined_tick ?? 0) || a.index - b.index)
    .map((p) => ({
      person: p.index,
      row: null,
      purse: purses.get(p.index) ?? null,
      waiting: true,
      since: dayOfTick(run, p.joined_tick),
      sponsor: p.introduced_by ?? null,
    }));
  return [...members, ...waiting];
}

/** What the person view shows of one person on one day, member or not: the
 *  ledger row where there is one, the purse, and how they came to be here. */
export function personFacts(run, day, person) {
  const p = run?.persons?.[person] ?? null;
  const row = day.members.find((m) => m.person === person) ?? null;
  const purse = (day.purses || []).find((x) => x.person === person) ?? null;
  return {
    p,
    row,
    purse,
    waiting: row === null,
    since: p ? dayOfTick(run, p.joined_tick) : null,
    sponsor: p?.introduced_by ?? null,
    connections: row ? day.edges.filter(([a, b]) => a === row.id || b === row.id) : [],
  };
}

export function economySeries(days) {
  const seen = new Set();
  return days.map((day) => {
    let booked = 0;
    for (const contract of day.contracts) {
      if (!seen.has(contract.id)) {
        seen.add(contract.id);
        // A row a handover or a cascade reopened inherits its acceptance and
        // restarts its creation; counting its original again would book the
        // same credit twice. A tape without the two epochs counts every row.
        const successor =
          contract.accepted_epoch !== undefined &&
          contract.created_epoch !== undefined &&
          contract.accepted_epoch !== contract.created_epoch;
        if (!successor) booked += contract.original;
      }
    }
    let cash = 0;
    let cashOffers = 0;
    let valueOffers = 0;
    for (const e of day.events) {
      const acts = e.event === "day" ? e.acts : e.event === "instructions" ? e.fired : [];
      for (const { act, result } of acts || []) {
        if (!act || !result) continue;
        if (act.act === "pay_cash" && result.result === "paid") cash += act.amount_minor;
        if (result.result === "applied" && result.cash_moved) cash += result.cash_moved;
        if (act.act === "offer" && act.paid_with === "cash") cashOffers++;
        if (act.act === "offer" && act.paid_with === "value") valueOffers++;
      }
    }
    return {
      ...snapshotStats(day),
      epoch: day.epoch,
      creditBooked: booked,
      cashPaid: cash,
      priceIndex: day.economy.index_ppm / 1e6,
      householdsCut: day.economy.published.households_with_income_cut,
      cashOffers,
      valueOffers,
    };
  });
}

/** Who is under an income cut today, by person index. The economy draws the
 *  cut; the callout that mentions it should be able to say whose it is. */
export function cutHouseholds(day) {
  return (day.purses || []).filter((p) => p.income_cut).map((p) => p.person);
}

export function dayHighlights(day, previous) {
  const stats = snapshotStats(day);
  const before = previous ? snapshotStats(previous) : null;
  const cut = day.economy.published.households_with_income_cut;
  const changes = [];
  if (before && stats.members > before.members)
    changes.push({ text: `${stats.members - before.members} new member${stats.members - before.members === 1 ? "" : "s"} joined the ledger.` });
  if (before && stats.closed > before.closed)
    changes.push({ text: `${stats.closed - before.closed} contract${stats.closed - before.closed === 1 ? "" : "s"} reached settlement or cure.` });
  if (stats.defaults)
    changes.push({ text: `${stats.defaults} contract${stats.defaults === 1 ? " is" : "s are"} in default. Select a coral-ringed member to inspect their obligations.` });
  if (cut)
    changes.push(
      { text: `${cut} of ${day.economy.published.households} households have had their income cut.`, people: cutHouseholds(day) },
    );
  if (!changes.length)
    changes.push({ text: `${stats.active} active contracts connect this community, with ${stats.stakes} recorded stakes backing its members.` });
  return changes;
}

export function milestones(days) {
  const points = [{ at: 0, label: "First snapshot", kind: "start" }];
  const firstStake = days.findIndex((d) => d.edges.length > 0);
  // The sweep's own list where the tape carries it; the closes otherwise.
  const firstDefault = days.some((d) => d.expired_today?.length)
    ? days.findIndex((d) => d.expired_today?.length)
    : days.findIndex((d) => d.contracts.some((c) => c.status === "expired"));
  const firstCrisis = days.findIndex((d) => d.economy.regime === "crisis");
  if (firstStake >= 0) points.push({ at: firstStake, label: "First backing", kind: "stake" });
  if (firstDefault >= 0) points.push({ at: firstDefault, label: "First default", kind: "default" });
  if (firstCrisis >= 0)
    points.push({ at: firstCrisis, label: "Economic pressure", kind: "crisis" });
  if (days.length > 1) points.push({ at: days.length - 1, label: "Last snapshot", kind: "end" });
  return points.sort((a, b) => a.at - b.at);
}

/**
 * **Every contract over the run, with its whole life.** The ledger view lists
 * the state on one day, and a transition inside a day — a default the sweep
 * marks in the morning and a cure pays off by evening — is in no day's state.
 * This is read off every snapshot and off `expired_today`, so it does not
 * move with the cursor: each row names the day the contract was first seen,
 * the parties and amount as booked, and every change of status or creditor
 * with the day it happened, whether or not a close ever showed it.
 *
 * Steps are, in order of appearance: `opened` (first seen, with the status
 * it had), `default marked` (the sweep's list, whatever the close shows),
 * then every status a close shows that the previous close did not, and
 * `creditor changed` where the claim moved — an underwriter taking over an
 * insured default is the case the run produces.
 */
export function contractRegister(days) {
  const rows = new Map();
  for (let i = 0; i < days.length; i++) {
    const day = days[i];
    const marked = new Set(day.expired_today || []);
    for (const c of day.contracts) {
      let r = rows.get(c.id);
      if (!r) {
        r = {
          id: c.id,
          at: i,
          epoch: day.epoch,
          debtor: c.debtor,
          creditor: c.creditor,
          original: c.original,
          insured: c.insured,
          steps: [{ at: i, epoch: day.epoch, what: "opened", status: c.status }],
          status: c.status,
          marked: false,
          lastCreditor: c.creditor,
        };
        rows.set(c.id, r);
        if (marked.has(c.id)) {
          r.steps.push({ at: i, epoch: day.epoch, what: "default marked" });
          r.marked = true;
        }
        continue;
      }
      if (marked.has(c.id)) {
        r.steps.push({ at: i, epoch: day.epoch, what: "default marked" });
        r.marked = true;
      }
      if (c.creditor !== r.lastCreditor) {
        r.steps.push({ at: i, epoch: day.epoch, what: "creditor changed", creditor: c.creditor });
        r.lastCreditor = c.creditor;
      }
      if (c.status !== r.status) {
        r.steps.push({ at: i, epoch: day.epoch, what: c.status });
        r.status = c.status;
      }
    }
  }
  return [...rows.values()].map(({ lastCreditor, ...r }) => r);
}

/** Whether a register row matches a state filter: its status now, or a state
 *  it ever passed through. */
export function registerMatches(r, filter) {
  switch (filter) {
    case "all":
      return true;
    case "ever":
      return r.marked || r.steps.some((s) => s.what === "expired");
    case "expired":
      return r.status === "expired";
    case "closed":
      return r.status === "settled" || r.status === "cured";
    case "insured":
      return r.insured;
    case "underwriter":
      return r.steps.some((s) => s.what === "creditor changed");
    default:
      return r.status === filter;
  }
}

export function sparkline(values, width = 100, height = 30) {
  if (!values.length) return "";
  const lo = Math.min(...values);
  const span = Math.max(...values) - lo || 1;
  return values
    .map(
      (value, i) =>
        `${i ? "L" : "M"}${((i / Math.max(values.length - 1, 1)) * width).toFixed(2)},${(height - 3 - ((value - lo) / span) * (height - 6)).toFixed(2)}`,
    )
    .join(" ");
}

export const compactMoney = (minor) =>
  Number.isFinite(minor)
    ? new Intl.NumberFormat("en", {
        maximumFractionDigits: 1,
        notation: minor >= 1000000 ? "compact" : "standard",
      }).format(minor / 100)
    : "–";
export const shortName = (run, person) =>
  run.persons[person]?.card_name || (person == null ? "Unassigned account" : `Person ${person}`);
/// A person as a sentence should name them: the card the player shows
/// everywhere else, with the member number that tells four stallholders apart.
export const namedPerson = (run, person) => {
  const p = run?.persons?.[person];
  if (!p) return person === null || person === undefined ? "somebody" : `person ${person}`;
  const card = p.card_name || "newcomer";
  return p.member === null || p.member === undefined ? `${card} (no account)` : `${card} (member ${p.member})`;
};

/// **The ledger knows a number; who that is, is the player's to say.** `index`
/// writes what the world wrote — `member 6`, `person 4` — because that is all a
/// member is told. Here, where the reader can see every card, those read as the
/// people they are. A number naming nobody in this run is left exactly as it
/// came, since a wrong name is worse than a number.
export const naming = (run) => (text) =>
  typeof text !== "string"
    ? text
    : text
        .replace(/\bmember (\d+)\b/g, (whole, id) => {
          const person = (run?.persons || []).find((p) => p.member === Number(id));
          return person ? namedPerson(run, person.index) : whole;
        })
        .replace(/\bperson (\d+)\b/g, (whole, i) =>
          (run?.persons || [])[Number(i)] ? namedPerson(run, Number(i)) : whole,
        );

/// **Who an act is with, from the reader's side.** An outcome line carries a
/// reference and no person — every offer ever sent "waits for the others" —
/// so the people come off the act's own `parties`, which `index` writes as the
/// ledger names them: a member number, or a newcomer with no row yet.
export const otherParties = (run, act, person) =>
  (act?.parties || [])
    .map((p) => {
      if (p.newcomer) {
        // The key became somebody: the nth person this owner brought in.
        const born = (run?.persons || []).filter((q) => q.introduced_by === p.owner)[p.n];
        if (!born) return "a newcomer";
        return born.index === person ? null : namedPerson(run, born.index);
      }
      const i =
        p.person !== undefined && p.person !== null
          ? p.person
          : (run?.persons || []).find((q) => q.member === p.member)?.index;
      return i === undefined || i === null || i === person ? null : namedPerson(run, i);
    })
    .filter(Boolean);

/// **An outcome, with the vagueness replaced by the person.** The ledger's
/// own words are true of every offer ever sent — "waiting for the others" —
/// because an `ActResult` knows a digest and nobody. The act beside it knows
/// who, so the name goes where "the others" stood, and only otherwise is it
/// added at the end.
export const outcomeText = (run, row, person, name = naming(run)) => {
  // A person is named in parentheses, so the reference cannot be: two
  // bracketed asides in a row read as a mistake. The ref is what it always
  // was — the twelve hex the ledger keeps — set off as the note it is.
  const ref = (t) => t.replace(/\(ref ([0-9a-f]+)\)/g, "· ref $1");
  const text = ref(name(row?.says) || row?.result?.result || "");
  const who = otherParties(run, row?.act, person);
  if (!who.length) return text;
  const list = who.join(" and ");
  if (text.includes("the others")) return text.replace("the others", list);
  if (text.includes("others")) return text.replace("others", list);
  return `${text} with ${list}`;
};

export const initials = (name) =>
  name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((n) => n[0])
    .join("")
    .toUpperCase();

/** **What the ledger did, day by day**, beside what people decided.
 *
 * Most of this is not recorded anywhere as an event: the epoch sweep nets
 * rings, substitutes underwriters and marks defaults with nobody's signature
 * on it, and a sale moves a debt onto the buyer inside one transition. What
 * the tape holds is the snapshots, so these are read by DIFFING consecutive
 * days. Inferred, and labelled as inferred.
 *
 * Three of these were read wrong before somebody checked them against the
 * kernel, and the corrections are the reason the names here are careful:
 *
 *   * **A debt that changes hands does not change `debtor` on its row.**
 *     `move_debtor` CLOSES the row (`status = transferred`, outstanding 0,
 *     insured false) and opens a new contract for the successor, so a diff on
 *     `debtor` can never fire. What is counted is the row closing.
 *   * **`insured` falling is not always insurance lost** — the same transfer
 *     clears it on the row it closes. Only a row that stays live counts.
 *   * **An amount that falls is not a netting.** A settlement, a cure, a part
 *     payment and a ring all lower it, and the tape does not say which: an act
 *     names a contract only when it OPENED one. So what is counted is the fall
 *     itself, under its own name, and no cause is claimed.
 */
export function ledgerSeries(days) {
  const prev = new Map();
  return days.map((day) => {
    let substituted = 0, handedOn = 0, discharged = 0, defaulted = 0, lostInsurance = 0;
    let refused = 0;
    for (const e of day.events || []) {
      if (e.event === "day") refused += (e.refused || []).length;
      // An autopilot act the ledger turned down is a refusal too. It has no
      // `refused` list of its own: a fired act carries its result inline.
      if (e.event === "instructions")
        for (const a of e.fired || [])
          if (a.result && ["refused", "rejected"].includes(a.result.result)) refused += 1;
    }
    for (const c of day.contracts) {
      const was = prev.get(c.id);
      if (was) {
        if (was.creditor !== c.creditor) substituted += 1;
        if (was.status !== "transferred" && c.status === "transferred") handedOn += 1;
        if (c.outstanding < was.outstanding) discharged += was.outstanding - c.outstanding;
        if (was.status !== "expired" && c.status === "expired") defaulted += 1;
        if (was.insured && !c.insured && c.status !== "transferred") lostInsurance += 1;
      }
      prev.set(c.id, c);
    }
    const live = day.contracts.filter((c) => c.status === "active" || c.status === "expired");
    const owed = live.reduce((sum, c) => sum + c.outstanding, 0);
    return {
      refused,
      // A share of CREDIT is a share of what is owed, not of how many rows
      // there are: on the pilot's last day those are 4.7% and 12.8%.
      insuredShare: owed ? (live.filter((c) => c.insured).reduce((sum, c) => sum + c.outstanding, 0) / owed) * 100 : 0,
      capacity: day.members.reduce((sum, m) => sum + (m.capacity || 0), 0),
      pending: day.pending_offers ?? 0,
      // Still to come, not already late: an expired row is past its maturity
      // for ever and would otherwise sit in this figure to the end of the run.
      dueSoon: day.contracts.reduce(
        (sum, c) =>
          c.status === "active" && c.maturity_epoch >= day.epoch && c.maturity_epoch - day.epoch <= 30
            ? sum + c.outstanding
            : sum,
        0,
      ),
      substituted,
      handedOn,
      discharged,
      lostInsurance,
      defaulted,
    };
  });
}

/** **Every act that was tried and not taken, by what refused it.**
 *
 * Two different things end up here and they are not the same: the LEDGER
 * refuses with a code (`ET-BND-001`), and the world around it rejects with a
 * sentence ("the payer's cash does not cover the payment") — a wallet-level
 * check that never reached the ledger. Lumping the prose in with the codes
 * put two spellings of one cash shortfall in a monospace chip under a heading
 * that said the ledger had refused them. `fromLedger` says which is which.
 *
 * An autopilot act is included: it carries its result inline rather than in a
 * `refused` list, and an act the ledger turned down is no less refused for
 * having been fired by a standing instruction.
 */
export function refusalsByCode(days, upto) {
  const by = new Map();
  const note = (r, i, person) => {
    const fromLedger = !!(r.result && r.result.code);
    const code = (r.result && (r.result.code || r.result.reason)) || "refused";
    const seen = by.get(code) || { code: code, fromLedger: fromLedger, n: 0, acts: new Set(), last: null };
    seen.n += 1;
    if (r.act && r.act.act) seen.acts.add(r.act.act);
    seen.last = { epoch: days[i].epoch, at: i, person, says: r.says };
    by.set(code, seen);
  };
  for (let i = 0; i <= upto && i < days.length; i++)
    for (const e of days[i].events || []) {
      if (e.event === "day") for (const r of e.refused || []) note(r, i, e.person);
      if (e.event === "instructions")
        for (const r of e.fired || [])
          if (r.result && ["refused", "rejected"].includes(r.result.result)) note(r, i, r.person);
    }
  return [...by.values()].sort((a, b) => b.n - a.n);
}

/** **What backings reach one member, and how much they could carry.**
 *
 * Capacity is the maximum flow into an account from the community's
 * underwriters across the directed stakes creditors have placed. This
 * computes that flow over the edges the tape carries — each underwriter a
 * source capped by its declared supply, the member the sink, the member's own
 * supply excluded, which is why a coalition cannot underwrite itself.
 *
 * **`reach` is the GROSS cut, and the ledger's capacity is lower.** The
 * model's own sentence ends "outstanding credit reserves its flow": the
 * kernel builds the same network at `supply − committed` and `weight −
 * reserved` (`crates/kernel/src/flow.rs`), and `edet-state` keeps the two
 * readings as separate functions with a warning not to conflate them. The
 * tape carries neither reservation, so the residual figure is NOT
 * reconstructible here — it is on every member row as `capacity`, written by
 * the ledger, and that is the number to show. Measured on the pilot's last
 * day, one member's gross reach was 295.26 where the ledger's capacity was 0:
 * every unit of it was already reserved by credit drawn.
 *
 * So what this is for is the PATH, not the total: which underwriters reach
 * this person, through whom, and where the cut binds.
 */
export function capacityFlow(day, memberId) {
  const nodes = new Map();
  const id = (n) => {
    if (!nodes.has(n)) nodes.set(n, nodes.size);
    return nodes.get(n);
  };
  const SOURCE = id("::source");
  const SINK = id(memberId);
  const cap = new Map();
  const arc = (a, b, c) => {
    const key = `${a}>${b}`;
    cap.set(key, (cap.get(key) || 0) + c);
    if (!cap.has(`${b}>${a}`)) cap.set(`${b}>${a}`, 0);
  };
  for (const m of day.members)
    if (m.supply > 0 && m.id !== memberId) arc(SOURCE, id(m.id), m.supply);
  for (const [from, to, weight] of day.edges) if (weight > 0) arc(id(from), id(to), weight);

  const neighbours = new Map();
  for (const key of cap.keys()) {
    const [a, b] = key.split(">").map(Number);
    if (!neighbours.has(a)) neighbours.set(a, new Set());
    neighbours.get(a).add(b);
  }
  const flow = new Map();
  const left = (a, b) => (cap.get(`${a}>${b}`) || 0) - (flow.get(`${a}>${b}`) || 0);
  let total = 0;
  for (;;) {
    // widest-first is not needed; a shortest augmenting path is enough
    const from = new Map([[SOURCE, null]]);
    const queue = [SOURCE];
    while (queue.length && !from.has(SINK)) {
      const a = queue.shift();
      for (const b of neighbours.get(a) || [])
        if (!from.has(b) && left(a, b) > 0) {
          from.set(b, a);
          queue.push(b);
        }
    }
    if (!from.has(SINK)) break;
    let bottleneck = Infinity;
    for (let b = SINK; from.get(b) != null; b = from.get(b)) bottleneck = Math.min(bottleneck, left(from.get(b), b));
    for (let b = SINK; from.get(b) != null; b = from.get(b)) {
      const a = from.get(b);
      flow.set(`${a}>${b}`, (flow.get(`${a}>${b}`) || 0) + bottleneck);
      flow.set(`${b}>${a}`, (flow.get(`${b}>${a}`) || 0) - bottleneck);
    }
    total += bottleneck;
  }
  const name = new Map([...nodes].map(([n, i]) => [i, n]));
  const carrying = [];
  for (const [key, used] of flow)
    if (used > 0) {
      const [a, b] = key.split(">").map(Number);
      const entry = {
        from: name.get(a) === "::source" ? null : name.get(a),
        to: name.get(b),
        used,
        of: cap.get(key) || 0,
      };
      carrying.push(entry);
    }
  carrying.sort((x, y) => y.used - x.used);
  return { reach: total, carrying };
}

