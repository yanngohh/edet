// A deterministic, illustrative tape for exploring the interface. These are
// authored examples, not a replay of the kernel or evidence about agent behavior.
export function createDemo() {
  const names = [
    "Elena Rossi",
    "Noah Chen",
    "Amara Okafor",
    "Luca Moretti",
    "Sofia Andersson",
    "Oliver Grant",
    "Maya Patel",
    "Leo Martin",
    "Ines Costa",
    "Felix Weber",
    "Aisha Hassan",
    "Theo Laurent",
    "Clara Silva",
    "Hugo Dubois",
    "Zara Khan",
    "Oscar Berg",
    "Nora Jensen",
    "Mateo Garcia",
    "Freya Wilson",
    "Elias Novak",
    "Lina Park",
    "Arlo James",
    "Ada Kowalski",
    "Milo Santos",
    "Esme Brown",
    "Jasper Lee",
    "Alma Fischer",
    "Ravi Shah",
    "Iris Kim",
    "Finn Murphy",
    "Leila Ali",
    "Jonas Lind",
    "Mina Sato",
    "Louis Petit",
    "Anya Ivanova",
    "Kai Nguyen",
    "Eva Muller",
    "Ruben Diaz",
    "Nadia Ahmed",
    "Sam Taylor",
    "Mira Das",
    "Alex Meyer",
    "Sara Olsen",
    "Ben Cooper",
    "Nia Williams",
    "Max Schmidt",
    "Lara Romano",
    "Idris Bello",
  ];
  const trades = [
    "community grocer",
    "furniture maker",
    "neighborhood baker",
    "bicycle mechanic",
    "market gardener",
    "ceramicist",
    "textile designer",
    "solar installer",
    "coffee roaster",
    "bookshop owner",
    "software developer",
    "electrician",
  ];
  const persons = names.map((name, i) => ({
    index: i,
    card_name: name,
    member: i + 1,
    founder: i < 5,
    tier: i < 5 ? "paper" : "basic",
    address: `demo-${String(i + 1).padStart(4, "0")}`,
    introduced_by: i < 5 ? null : i % 5,
    joined_tick: i < 36 ? 1 : (i - 35) * 5,
    retired: null,
    card: `${name} is a ${trades[i % trades.length]}. ${i < 5 ? "A founding underwriter, they put their own declared supply behind the community." : "They trade with neighbors and build capacity through the obligations they repay."} This persona and its decisions are authored for the illustrative demo.`,
  }));
  const days = Array.from({ length: 60 }, (_, index) => {
    const t = index + 1;
    const count = Math.min(48, 36 + Math.floor(t / 5));
    const crisis = t >= 28 && t < 43;
    const edges = [];
    for (let i = 5; i < count; i++) {
      if (t > 1)
        edges.push([
          (i % 5) + 1,
          i + 1,
          Math.round((40000 + ((i * 7919) % 120000)) * Math.min(1, t / 18)),
        ]);
      if (t > 6)
        edges.push([
          ((i + 3) % (count - 5)) + 6,
          i + 1,
          Math.round((15000 + ((i * 3571) % 55000)) * Math.min(1, t / 24)),
        ]);
      if (t > 14 && i % 3 === 0) edges.push([((i + 13) % (count - 5)) + 6, i + 1, 25000 + i * 330]);
    }
    const contracts = Array.from({ length: Math.max(0, t * 3 - 2) }, (_, id) => {
      const birth = Math.floor((id + 2) / 3) + 1;
      const age = t - birth;
      const original = 8000 + ((id * 6551) % 85000);
      const defaultsAtMaturity = id % 31 === 24 || id % 31 === 25;
      const status =
        age > 17
          ? defaultsAtMaturity
            ? "cured"
            : "settled"
          : age > 10
            ? defaultsAtMaturity
              ? "expired"
              : "settled"
            : "active";
      const birthCount = Math.min(48, 36 + Math.floor(birth / 5));
      const debtor = 6 + ((id * 7) % (birthCount - 5));
      let creditor = 1 + ((id * 11) % birthCount);
      if (creditor === debtor) creditor = (creditor % birthCount) + 1;
      return {
        id: id + 1,
        debtor,
        creditor,
        original,
        outstanding: status === "settled" || status === "cured" ? 0 : original,
        maturity_epoch: birth + 10,
        status,
        insured: id % 4 !== 0 && birth > 6,
      };
    });
    const members = persons.slice(0, count).map((p, i) => ({
      id: i + 1,
      person: i,
      status: "active",
      supply: i < 5 ? 960000 : 0,
      capacity:
        t === 1
          ? 0
          : Math.round(
              (45000 + ((i * 13271) % 210000)) * Math.min(1, t / 20) * (crisis ? 0.82 : 1),
            ),
      debt: contracts.filter((c) => c.debtor === i + 1).reduce((s, c) => s + c.outstanding, 0),
      open_default: contracts
        .filter((c) => c.debtor === i + 1 && c.status === "expired")
        .reduce((s, c) => s + c.outstanding, 0),
    }));
    const cut = crisis ? 10 + (t % 7) : t > 18 ? 2 + (t % 3) : 0;
    const purses = persons
      .slice(0, count)
      .map((p, i) => ({
        person: i,
        cash: Math.max(0, 170000 + ((i * 11113) % 480000) - (crisis ? 140000 : 0) + t * 840),
        arrears: crisis && i < cut ? 20000 + i * 1000 : 0,
        income_cut: i < cut,
      }));
    const actor = (t * 3) % count;
    const recipient = (actor + 7) % count;
    const events = [
      {
        event: "day",
        person: actor,
        refused: [],
        acts: [
          {
            act: {
              act: "offer",
              describe: `Offered goods to ${names[recipient]}`,
              paid_with: "value",
            },
            result: { result: "applied" },
            says: "Offer recorded on the ledger",
          },
        ],
      },
      {
        event: "day",
        person: recipient,
        refused: [],
        acts: [
          {
            act: {
              act: "pay_cash",
              describe: "Paid a neighbor in cash",
              amount_minor: 14500 + t * 270,
            },
            result: { result: "paid" },
            says: "Cash payment completed",
          },
        ],
      },
      {
        event: "day",
        person: (actor + 4) % count,
        refused: [],
        acts: [
          {
            act: {
              act: "diary",
              text: crisis
                ? "Orders are still coming in, but cash is tighter. I need to see which obligations I can clear through my work."
                : "A promise means more when you can see who stands behind it. Today I traded with someone new.",
            },
            result: { result: "noted" },
            says: "Diary recorded",
          },
        ],
      },
    ];
    // What a refusal looks like. A panel that is always empty teaches a reader
    // nothing, and the ledger refuses far more often than it fails: every code
    // below is one the real ledger returns, and the player reads its English
    // out of the wallet's own locale rather than out of this file. The shapes
    // are authored like everything else here — one member whose writes keep
    // meeting the operation bond, and settlements tried before maturity once
    // the crisis starts.
    const bound = 3;
    const refused = [];
    if (t % 5 === 0 && t > 5)
      refused.push({
        act: { act: "sign", describe: `Signed the offer from ${names[recipient]}` },
        result: { result: "refused", code: "ET-BND-001" },
        says: "refused by the ledger: ET-BND-001",
      });
    if (crisis && t % 3 === 1)
      refused.push({
        act: { act: "settle", describe: "Settled what is outstanding on #12" },
        result: { result: "refused", code: "ET-CTR-006" },
        says: "refused by the ledger: ET-CTR-006",
      });
    if (refused.length) events.push({ event: "day", person: bound, refused, acts: [] });
    return {
      tick: t,
      epoch: t,
      members,
      edges,
      contracts,
      purses,
      events,
      economy: {
        index_ppm: 1000000 + t * 1700 + Math.max(0, t - 27) * 1200,
        regime: crisis ? "crisis" : t < 15 ? "calm" : "boom",
        published: {
          price_index: ((1000000 + t * 1700 + Math.max(0, t - 27) * 1200) / 1e6).toFixed(4),
          change_over_last_30_days: `+${(Math.min(30, t) * 0.17).toFixed(2)}%`,
          households_with_income_cut: cut,
          households: count,
        },
      },
      seed: 4800000,
      seat_committed: count * 2000,
      pending_offers: 2 + (t % 5),
      square: [],
      mail: [],
    };
  });
  const lives = new Map(
    persons.map((p, person) => [
      person,
      new Blob([
        JSON.stringify(
          days
            .filter((d) => d.events.some((e) => e.person === person))
            .map((d) => ({
              tick: d.tick,
              what: "day",
              messages: [
                {
                  role: "assistant",
                  content: d.events
                    .find((e) => e.person === person)
                    .acts.map((a) => ({
                      type: "text",
                      text: a.act.text || `${a.act.describe}. ${a.says}.`,
                    })),
                },
              ],
            })),
        ),
      ]),
    ]),
  );
  return {
    demo: true,
    run: {
      model: "Illustrative scenario",
      world_seed: 42,
      control: false,
      turns_used: 180,
      turn_budget: 180,
      not_a_run: true,
      ended: "Demo complete",
      persons,
    },
    days,
    lives,
  };
}
