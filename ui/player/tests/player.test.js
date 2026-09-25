import test from "node:test";
import assert from "node:assert/strict";
import { loadRun, loadLife, money, scenarioEntries } from "../src/load.js";
import { describeRun, scenariosIn } from "../scenarios.mjs";
import {
  snapshotStats,
  economySeries,
  contractRegister,
  registerMatches,
  milestones,
  naming,
  otherParties,
  outcomeText,
  peopleOn,
  personFacts,
} from "../src/analytics.js";
import { createDemo } from "../src/demo.js";
import { layoutNetwork, networkScales, orbitWatch, orbitRested, satellitesOf, ORBIT_LAG_MS } from "../src/network.js";

const emptyDay = () => ({
  epoch: 1,
  tick: 1,
  members: [],
  contracts: [],
  edges: [],
  purses: [],
  events: [],
  mail: [],
  seed: 10000,
  seat_committed: 0,
  economy: {
    index_ppm: 1000000,
    regime: "calm",
    published: { households: 0, households_with_income_cut: 0, price_index: "1.0" },
  },
});
const file = (path, text) => ({
  name: path.split("/").at(-1),
  webkitRelativePath: path,
  text: async () => text,
});
const tape = (root = "", days = [emptyDay()]) => [
  file(`${root}run.json`, JSON.stringify({ persons: [] })),
  file(`${root}days.jsonl`, days.map((d) => JSON.stringify(d)).join("\n") + "\n"),
];

test("imports both indexed folders and enclosing run folders without depending on their names", async () => {
  for (const root of ["", "player/", "renamed-export/", "myplayer-test/a-run/player/"]) {
    const data = await loadRun([...tape(root), file(`${root}lives/4.json`, '[{"tick":1}]')]);
    assert.equal(data.days.length, 1);
    assert.deepEqual(await loadLife(data.lives, 4), [{ tick: 1 }]);
  }
});

test("rejects empty, ambiguous, and malformed tapes with actionable errors", async () => {
  await assert.rejects(loadRun([]), /no files to read/);
  await assert.rejects(loadRun([file("other.json", "{}")]), /no run.json and days.jsonl/);
  await assert.rejects(loadRun([...tape("one/"), ...tape("two/")]), /several runs/);
  await assert.rejects(loadRun(tape("", [])), /no snapshots/);
  await assert.rejects(loadRun([file("run.json", "{"), tape()[1]]), /run.json is not valid/);
  await assert.rejects(loadRun([file("run.json", "{}"), tape()[1]]), /list of people/);
  await assert.rejects(loadRun([tape()[0], file("days.jsonl", "{")]), /snapshot 1/);
  await assert.rejects(loadRun(tape("", [{ tick: 1 }])), /incomplete/);
});

test("life caches are isolated by run, including repeated person IDs", async () => {
  const a = new Map([[2, file("", '[{"tick":1}]')]]);
  const b = new Map([[2, file("", '[{"tick":9}]')]]);
  assert.deepEqual(await loadLife(a, 2), [{ tick: 1 }]);
  assert.deepEqual(await loadLife(b, 2), [{ tick: 9 }]);
  assert.deepEqual(await loadLife(a, 99), []);
});

test("outstanding credit excludes closed and transferred obligations", () => {
  const day = emptyDay();
  day.contracts = ["active", "expired", "settled", "cured", "transferred"].map((status, i) => ({
    id: i,
    status,
    outstanding: 1000,
    insured: status === "active",
  }));
  const stats = snapshotStats(day);
  assert.equal(stats.outstanding, 2000);
  assert.equal(stats.active, 1);
  assert.equal(stats.defaults, 1);
  assert.equal(stats.closed, 2);
  assert.equal(stats.live, 2);
  assert.equal(stats.insured, 1);
});

test("first-seen credit is counted once and cash includes only successful movements", () => {
  const first = emptyDay();
  first.contracts = [{ id: 1, original: 2500, outstanding: 2000, status: "active", insured: true }];
  const second = {
    ...emptyDay(),
    epoch: 2,
    tick: 2,
    contracts: [
      ...first.contracts,
      { id: 2, original: 4500, outstanding: 4500, status: "active", insured: false },
    ],
    events: [
      {
        event: "day",
        acts: [
          { act: { act: "pay_cash", amount_minor: 300 }, result: { result: "paid" } },
          { act: { act: "pay_cash", amount_minor: 900 }, result: { result: "rejected" } },
          {
            act: { act: "offer", paid_with: "cash" },
            result: { result: "applied", cash_moved: 700 },
          },
          { act: { act: "offer", paid_with: "value" }, result: { result: "rejected" } },
        ],
      },
      {
        event: "instructions",
        fired: [{ act: { act: "pay_cash", amount_minor: 200 }, result: { result: "paid" } }],
      },
    ],
  };
  const series = economySeries([first, second]);
  assert.deepEqual(
    series.map((s) => s.creditBooked),
    [2500, 4500],
  );
  assert.equal(series[1].cashPaid, 1200);
  assert.equal(series[1].cashOffers, 1);
  assert.equal(series[1].valueOffers, 1);
});

test("milestones use snapshot positions even when epochs have gaps", () => {
  const days = [
    emptyDay(),
    { ...emptyDay(), epoch: 100, edges: [[1, 2, 100]], contracts: [{ status: "expired" }] },
  ];
  assert.deepEqual(
    milestones(days).map((p) => [p.label, p.at]),
    [
      ["First snapshot", 0],
      ["First backing", 1],
      ["First default", 1],
      ["Last snapshot", 1],
    ],
  );
  assert.equal(milestones([emptyDay()]).length, 1);
});

test("the illustrative tape is deterministic and its lifecycle does not revive closed contracts", () => {
  const { days } = createDemo();
  assert.deepEqual(days, createDemo().days);
  const states = new Map();
  for (const day of days) {
    const ids = new Set(day.members.map((m) => m.id));
    for (const [a, b] of day.edges) {
      assert.ok(ids.has(a));
      assert.ok(ids.has(b));
      assert.notEqual(a, b);
    }
    for (const contract of day.contracts) {
      assert.ok(ids.has(contract.debtor) && ids.has(contract.creditor));
      if (contract.status === "active") assert.ok(day.epoch <= contract.maturity_epoch);
      if (contract.status === "expired") assert.ok(day.epoch > contract.maturity_epoch);
      if (["settled", "cured"].includes(states.get(contract.id)))
        assert.ok(["settled", "cured"].includes(contract.status));
      states.set(contract.id, contract.status);
    }
  }
});

test("the layout handles missing accounts and keeps deterministic finite coordinates", () => {
  assert.equal(layoutNetwork([emptyDay()]).size, 0);
  const days = [
    {
      ...emptyDay(),
      members: [{ id: 41 }, { id: 82 }],
      edges: [
        [41, 82, 500],
        [82, 999, 100],
      ],
    },
  ];
  const layout = layoutNetwork(days);
  assert.deepEqual(layout, layoutNetwork(days));
  for (const point of layout.values())
    for (const axis of ["x", "y", "z"]) assert.ok(Number.isFinite(point[axis]));
});

test("money remains exact in minor units and does not invent missing balances", () => {
  assert.equal(money(105), "1.05");
  assert.equal(money(-9), "-0.09");
  assert.equal(money(0), "0.00");
  assert.equal(money(null), "–");
});

test("visual scales remain fixed as capacities and backings change over time", () => {
  const days = [
    { ...emptyDay(), members: [{ id: 1, capacity: 400 }], edges: [[1, 2, 900]] },
    { ...emptyDay(), members: [{ id: 1, capacity: 800 }], edges: [[1, 2, 100]] },
  ];
  assert.deepEqual(networkScales(days), { capacity: 800, stake: 900 });
  assert.deepEqual(networkScales([emptyDay()]), { capacity: 1, stake: 1 });
});

test("the ledger's numbers read as the people the player can see", () => {
  const run = {
    persons: [
      { index: 0, member: 0, card_name: "teenager" },
      { index: 1, member: 3, card_name: "market stallholder" },
      { index: 2, member: null, card_name: "" },
    ],
  };
  const named = naming(run);
  assert.equal(
    named("member 3 lends 120.00 to member 0, due in 30 days"),
    "market stallholder (member 3) lends 120.00 to teenager (member 0), due in 30 days",
  );
  // A person index is what the world counts in, and the ledger never sees.
  assert.equal(named("message person 2"), "message newcomer (no account)");
  // A number naming nobody in this run is left as it came: a wrong name is
  // worse than a number.
  assert.equal(named("member 9 lends 1.00 to member 3"), "member 9 lends 1.00 to market stallholder (member 3)");
  // Only people are named. A contract is a contract.
  assert.equal(named("settle 10.00 on contract 3"), "settle 10.00 on contract 3");
});

test("an outcome that carries only a reference still says who it is with", () => {
  const run = {
    persons: [
      { index: 0, member: 0, card_name: "teenager" },
      { index: 1, member: 3, card_name: "market stallholder" },
    ],
  };
  const offer = { parties: [{ member: 0 }, { member: 3 }] };
  // The person whose day it is never appears: they know who they are.
  assert.deepEqual(otherParties(run, offer, 0), ["market stallholder (member 3)"]);
  assert.deepEqual(otherParties(run, offer, 1), ["teenager (member 0)"]);
  // Somebody with no row yet is named for what they are.
  assert.deepEqual(otherParties(run, { parties: [{ newcomer: true, owner: 1, n: 0 }] }, 1), ["a newcomer"]);
  // Once the key has become somebody, that is who the offer was with.
  const grown = {
    persons: [...run.persons, { index: 2, member: 12, card_name: "neighbour", introduced_by: 1 }],
  };
  assert.deepEqual(otherParties(grown, { parties: [{ newcomer: true, owner: 1, n: 0 }] }, 1), [
    "neighbour (member 12)",
  ]);
  // An act about nobody says nobody, and an un-indexed act does not throw.
  assert.deepEqual(otherParties(run, { parties: [] }, 0), []);
  assert.deepEqual(otherParties(run, {}, 0), []);
});

test("the orbit stops itself only when the scene stays too slow to draw", () => {
  const cheap = ORBIT_LAG_MS / 4;
  // A scene the machine can afford turns for ever.
  let state = orbitRested();
  for (let i = 0; i < 500; i++) {
    state = orbitWatch(state, cheap);
    assert.equal(state.stop, false);
  }
  // One slow frame decides nothing: a collection, or a tab coming back.
  state = orbitWatch(state, ORBIT_LAG_MS * 20);
  assert.equal(state.stop, false);
  // A scene that stays slow stops, and says so once rather than every frame.
  let stops = 0;
  state = orbitRested();
  for (let i = 0; i < 200; i++) {
    state = orbitWatch(state, ORBIT_LAG_MS * 3);
    if (state.stop) stops++;
  }
  assert.ok(stops >= 1, "a sustained cost must stop the orbit");
  assert.ok(stops <= 5, `it must not fire every frame, fired ${stops}`);
});

test("an outcome names the person where it used to say 'the others'", () => {
  const run = {
    persons: [
      { index: 0, member: 0, card_name: "former crypto trader" },
      { index: 1, member: 12, card_name: "part-time carer" },
    ],
  };
  const row = {
    act: { parties: [{ member: 0 }, { member: 12 }] },
    says: "offer sent, waiting for the others (ref 6c6b82c03a2b)",
    result: { result: "opened" },
  };
  // The day belongs to member 12, so the person waited on is the other one.
  assert.equal(
    outcomeText(run, row, 1),
    "offer sent, waiting for former crypto trader (member 0) · ref 6c6b82c03a2b",
  );
  // Read from the other side, the same offer waits on the carer.
  assert.equal(
    outcomeText(run, row, 0),
    "offer sent, waiting for part-time carer (member 12) · ref 6c6b82c03a2b",
  );
  // One bracketed aside to a line: the person keeps it, the reference does not.
  assert.equal(outcomeText(run, { act: {}, says: "declined (ref 0a1b2c3d)" }, 0), "declined · ref 0a1b2c3d");
  // An outcome about nobody is left exactly as the ledger wrote it.
  assert.equal(
    outcomeText(run, { act: { parties: [] }, says: "paid", result: { result: "paid" } }, 0),
    "paid",
  );
  // A refusal keeps its code intact, so the wallet's own sentence still shows.
  const refused = {
    act: { parties: [{ member: 0 }] },
    says: "refused by the ledger: ET-BND-001",
    result: { result: "refused" },
  };
  assert.equal(outcomeText(run, refused, 1), "refused by the ledger: ET-BND-001 with former crypto trader (member 0)");
});

test("a served run is described from the manifest, and says when it is not a run", () => {
  const run = { persons: [{}, {}, {}], ticks: [{}, {}], model: "gpt-5.6-luna", not_a_run: true };
  const d = describeRun("pilot-x", run, 4096);
  assert.equal(d.id, "pilot-x");
  assert.equal(d.people, 3);
  assert.equal(d.days, 2);
  assert.equal(d.model, "gpt-5.6-luna");
  // A scripted tape must say so wherever it is offered, not only once opened.
  assert.equal(d.not_a_run, true);
  // A manifest missing a field describes an empty run rather than throwing.
  assert.deepEqual(
    { people: 0, days: 0, not_a_run: false },
    (({ people, days, not_a_run }) => ({ people, days, not_a_run }))(describeRun("empty", {}, 0)),
  );
});

test("a served run reaches loadRun in the shape a picked folder has", async () => {
  const asked = [];
  const entries = scenarioEntries("pilot-x", ["run.json", "days.jsonl", "lives/7.json"], (p) => {
    asked.push(p);
    return `/scenarios/pilot-x/${p}`;
  });
  // The prefix is what names the run: `loadRun` takes the folder's own name.
  assert.deepEqual(
    entries.map((e) => e.webkitRelativePath),
    ["pilot-x/player/run.json", "pilot-x/player/days.jsonl", "pilot-x/player/lives/7.json"],
  );
  // **Nothing is fetched until it is read.** pilot-3's lives are 37 MB across
  // 128 people and a reader who scrubs the timeline opens none of them.
  assert.deepEqual(asked, []);
});

test("the scenario list is what has actually been indexed", () => {
  // Reads the repository's own runs directory: an empty or absent one is an
  // empty list, never a throw, because a tree with no runs in it is ordinary.
  const found = scenariosIn(new URL("../../civitas-runs/", import.meta.url).pathname);
  assert.ok(Array.isArray(found));
  for (const s of found) {
    assert.equal(typeof s.id, "string");
    assert.ok(s.days >= 0 && s.people >= 0);
  }
  assert.deepEqual(scenariosIn("/nowhere/at/all"), []);
});

// **The town is larger than the ledger.** pilot-3 had 128 people and 15
// members; a player that listed only `day.members` hid 113 lives.
const townRun = () => ({
  genesis_epoch: 0,
  persons: [
    { index: 0, card_name: "founder", member: 0, founder: true, joined_tick: 0, introduced_by: null },
    { index: 1, card_name: "never adopted", member: null, founder: false, joined_tick: 0, introduced_by: null },
    { index: 2, card_name: "introduced early", member: null, founder: false, joined_tick: 3, introduced_by: 0 },
    { index: 3, card_name: "introduced later", member: null, founder: false, joined_tick: 9, introduced_by: 0 },
  ],
});
const townDay = (tick) => ({
  ...emptyDay(),
  tick,
  epoch: tick,
  members: [{ id: 0, person: 0, status: "active", capacity: 500, debt: 0, supply: 1000, open_default: 0 }],
  purses: [
    { person: 0, cash: 100, arrears: 0, income_cut: false },
    { person: 1, cash: 200, arrears: 5, income_cut: false },
    { person: 2, cash: 300, arrears: 0, income_cut: false },
    { person: 3, cash: 400, arrears: 0, income_cut: false },
  ],
});

test("the people list holds everybody in the town by that day, members first", () => {
  const run = townRun();
  const early = peopleOn(run, townDay(5));
  assert.deepEqual(
    early.map((x) => [x.person, x.waiting, x.since, x.sponsor]),
    [
      [0, false, null, null],
      [1, true, 0, null],
      [2, true, 3, 0],
    ],
    "a person introduced on tick 9 does not exist on day 5",
  );
  assert.equal(early[0].row.id, 0, "a member keeps their ledger row");
  assert.equal(early[2].purse.cash, 300, "a waiting person carries their household purse");
  assert.equal(peopleOn(run, townDay(9)).length, 4, "and appears the day they are introduced");
  // The day a person is told they were introduced is the epoch, not the tick.
  assert.equal(peopleOn({ ...run, genesis_epoch: 100 }, townDay(5))[2].since, 103);
  // A run without persons is an empty town, not a throw.
  assert.deepEqual(peopleOn({}, emptyDay()), []);
});

test("snapshot stats count the town beside the ledger, and only the ledger without a run", () => {
  const stats = snapshotStats(townDay(5), townRun());
  assert.equal(stats.members, 1);
  assert.equal(stats.people, 3);
  assert.equal(stats.waiting, 2);
  const bare = snapshotStats(townDay(5));
  assert.equal(bare.members, 1);
  assert.equal(bare.people, 0);
  assert.equal(bare.waiting, 0);
});

test("a person without an account has facts to show and nothing of the ledger's", () => {
  const facts = personFacts(townRun(), townDay(5), 2);
  assert.equal(facts.row, null);
  assert.equal(facts.waiting, true);
  assert.equal(facts.since, 3);
  assert.equal(facts.sponsor, 0);
  assert.equal(facts.purse.cash, 300);
  assert.deepEqual(facts.connections, []);
  const member = personFacts(townRun(), townDay(5), 0);
  assert.equal(member.row.id, 0);
  assert.equal(member.waiting, false);
  // A person the run does not know is still a shape, not a throw.
  assert.equal(personFacts(townRun(), townDay(5), 99).p, null);
});

test("people without an account are drawn around their sponsor, or on the outer ring", () => {
  const run = townRun();
  const days = [townDay(5)];
  const positions = layoutNetwork(days);
  const sats = satellitesOf(run, days[0], positions);
  assert.deepEqual(sats.map((s) => [s.person, s.sponsor]).sort(), [[1, null], [2, 0]]);
  const anchor = positions.get(0);
  const near = sats.find((s) => s.person === 2);
  assert.ok(Math.hypot(near.x - anchor.x, near.y - anchor.y, near.z - anchor.z) < 60, "tethered close to the sponsor");
  const loose = sats.find((s) => s.person === 1);
  assert.ok(Math.hypot(loose.x, loose.z) > 200, "nobody introduced them: the outer ring");
  for (const s of sats) for (const axis of ["x", "y", "z", "fx", "fy"]) assert.ok(Number.isFinite(s[axis]));
  // The same input draws the same picture.
  assert.deepEqual(sats, satellitesOf(run, days[0], positions));
  // Members are not satellites, and a sponsor with no row on this day means the ring.
  assert.equal(satellitesOf(run, { ...townDay(5), members: [] }, positions).every((s) => s.sponsor === null), true);
});

test("what the sweep marked expired is read from the tape, and a same-day cure still counts", () => {
  const day = { ...emptyDay(), epoch: 5, expired_today: [3, 9] };
  day.contracts = [{ id: 3, status: "cured", outstanding: 0, original: 100, debtor: 1, creditor: 2, maturity_epoch: 4, insured: true }];
  const stats = snapshotStats(day);
  // At the close nothing is expired; the sweep still marked two.
  assert.equal(stats.defaults, 0);
  assert.equal(stats.expiredToday, 2);
  // A tape indexed before the list existed reads as zero, not as missing.
  assert.equal(snapshotStats(emptyDay()).expiredToday, 0);
});

test("the first default is the sweep's day where the tape carries it, and the close's otherwise", () => {
  const days = [
    { ...emptyDay(), epoch: 1, expired_today: [] },
    { ...emptyDay(), epoch: 2, expired_today: [7] },
    { ...emptyDay(), epoch: 3, expired_today: [], contracts: [{ status: "expired" }] },
  ];
  assert.deepEqual(
    milestones(days).filter((p) => p.kind === "default").map((p) => p.at),
    [1],
  );
  const closes = [{ ...emptyDay(), epoch: 1 }, { ...emptyDay(), epoch: 2, contracts: [{ status: "expired" }] }];
  assert.deepEqual(milestones(closes).filter((p) => p.kind === "default").map((p) => p.at), [1]);
});

test("credit first seen counts a fresh acceptance once and a reopened row never", () => {
  const fresh = { id: 1, original: 500, status: "active", outstanding: 500, debtor: 1, creditor: 2, maturity_epoch: 30, insured: false, created_epoch: 1, accepted_epoch: 1 };
  const successor = { id: 2, original: 500, status: "active", outstanding: 500, debtor: 3, creditor: 2, maturity_epoch: 30, insured: false, created_epoch: 4, accepted_epoch: 1 };
  const days = [
    { ...emptyDay(), epoch: 1, contracts: [fresh] },
    { ...emptyDay(), epoch: 4, contracts: [{ ...fresh, status: "transferred" }, successor] },
  ];
  assert.deepEqual(economySeries(days).map((s) => s.creditBooked), [500, 0]);
  // Without the epochs every new row counts, as it did before they were written.
  const bare = days.map((d) => ({ ...d, contracts: d.contracts.map(({ created_epoch, accepted_epoch, ...c }) => c) }));
  assert.deepEqual(economySeries(bare).map((s) => s.creditBooked), [500, 500]);
});

test("the register carries every contract's life, including a default the close never showed", () => {
  const row = (over) => ({ id: 7, debtor: 1, creditor: 2, original: 100, outstanding: 60, maturity_epoch: 3, insured: true, status: "active", ...over });
  const days = [
    { ...emptyDay(), epoch: 3, contracts: [row()] },
    // Marked in the morning, taken over by an underwriter and cured by evening.
    { ...emptyDay(), epoch: 4, expired_today: [7], contracts: [row({ status: "cured", outstanding: 0, creditor: 9 })] },
    { ...emptyDay(), epoch: 5, expired_today: [8], contracts: [row({ status: "cured", outstanding: 0, creditor: 9 }), row({ id: 8, status: "expired", insured: false })] },
    { ...emptyDay(), epoch: 6, contracts: [row({ status: "cured", outstanding: 0, creditor: 9 }), row({ id: 8, status: "settled", insured: false })] },
  ];
  const reg = contractRegister(days);
  assert.deepEqual(
    reg.map((r) => [r.id, r.epoch, r.status, r.marked, r.steps.map((s) => `${s.what}@${s.epoch}`)]),
    [
      [7, 3, "cured", true, ["opened@3", "default marked@4", "creditor changed@4", "cured@4"]],
      [8, 5, "settled", true, ["opened@5", "default marked@5", "settled@6"]],
    ],
  );
  // Every state is a filter: now, or ever passed through.
  const ids = (f) => reg.filter((r) => registerMatches(r, f)).map((r) => r.id);
  assert.deepEqual(ids("all"), [7, 8]);
  assert.deepEqual(ids("ever"), [7, 8]);
  assert.deepEqual(ids("expired"), []);
  assert.deepEqual(ids("cured"), [7]);
  assert.deepEqual(ids("settled"), [8]);
  assert.deepEqual(ids("closed"), [7, 8]);
  assert.deepEqual(ids("underwriter"), [7]);
  assert.deepEqual(ids("insured"), [7]);
  assert.deepEqual(ids("active"), []);
  // A tape without the sweep's list still has every contract's closes.
  const bare = days.map(({ expired_today, ...d }) => d);
  assert.deepEqual(contractRegister(bare).map((r) => r.steps.map((s) => s.what)), [["opened", "creditor changed", "cured"], ["opened", "settled"]]);
});
