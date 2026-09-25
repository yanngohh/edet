# the edet run player

A player for an indexed edet run's tape: a perspective 3D community network, a timeline, individual conversations, a contract ledger, and seven economic tracks. It runs entirely in the browser and sends no run data anywhere.

## Start

```sh
npm ci
npm run dev
```

Open the local URL printed by Vite (normally `http://localhost:5190`). **It opens on the newest run it can find**, and the toolbar lists the rest. With nothing indexed yet it falls back to a deterministic, explicitly labeled illustrative demo, whose people and snapshots are authored examples rather than kernel-validated experimental results.

To read your own tape, first run:

```sh
edet-civitas index /path/to/run
```

and it joins the list in the toolbar. Runs are read from `civitas-runs/` beside the tree — a tape is a run's only existence and can be very large, so it is never copied into the repository — and `CIVITAS_RUNS=/somewhere/else npm run dev` points the player at another directory. A run needs `run.json` and `days.jsonl`; `lives/<person>.json` is fetched only when somebody opens that person, because pilot-3's are 37 MB across 128 people.

To publish a player with a run inside it, for somebody who cannot run one:

```sh
CIVITAS_SCENARIOS=pilot-bc npm run build
```

which copies those runs into `dist/scenarios/` and fails rather than shipping an empty picker if one is not indexed.

## Reading a run

- Drag the network to rotate it; scroll or use the zoom controls to get closer. Switch to **2D** to compare sizes without perspective.
- Click a node or find someone in **People** to inspect their capacity, debt, household cash, and recorded conversations. Only conversations through the selected day appear.
- **People** lists the whole town, not only the ledger's rows: a person exists from the day somebody offers them a first trade, and one whose offer was never seated has a household, a life on the tape and no account. They appear after the members as "No account yet · waiting since day N", open like anybody else, and are drawn on the network as faint dots tethered to whoever introduced them, or on an outer ring when nobody did. The headline count stays the ledger's; "+ N without an account" is the rest of the town.
- Toggle **Backings** and **Contracts** independently. Backing arrows point creditor → debtor, and line width represents stake value. Dashed contracts are uninsured.
- Scrub or play the timeline. Milestones jump to events actually present in the tape, such as the first backing, default, or crisis regime.
- Open **Two economies** for credit, cash, prices, membership, income cuts, and generated payment choices. Click a chart to seek; focused charts also support arrow keys.
- Open **Ledger** to filter contracts and inspect recorded invariant failures through the current day.
- Press **P** or choose **Present** for a larger network and compact presentation layout. **Escape** exits; **Space** plays or pauses; **← / →** step through snapshots. Keyboard shortcuts do not intercept focused form controls. The focused network uses arrows to rotate, **+ / −** to zoom, and **Home** to reset.

Node size represents individual residual credit capacity, with a minimum visible radius for zero capacity. Node and backing scales stay fixed across the tape, and positions stay stable while scrubbing. Layout position and moving dots carry no measured economic quantity; dots illustrate backing direction. Gold nodes are underwriters and coral rings mark members with open defaults. The renderer respects reduced-motion preferences and offers an accessible people list alongside the canvas.

Individual capacities are not additive into a community credit budget. Declared backing shows underwriter supply, and outstanding credit sums active and expired obligations. Household cash stays separate. The `ⓖ` mark identifies generated choices, which describe a tape rather than establish how real people behave. Scripted tapes remain labeled, and the ledger view reports recorded violations without claiming to independently audit the tape.

## Build and check

```sh
npm test
npm run build
npm run preview
```

`dist/` is a static site with relative asset paths. There are no remote fonts, visualization dependencies, or external asset requests. The 3D field is drawn on a 2D canvas with perspective projection, lighting, depth sorting, hit testing, and a stable force layout; it does not require WebGL.
