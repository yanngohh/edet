// Reading a run: the files `edet-civitas index` writes under <run>/player/,
// served from the runs directory beside the tree. Everything below validates
// what came back — the naming, the shapes, the partial tail — and is reached
// through `loadScenario`, never from a folder a browser handed over: a tape is
// tens of megabytes and the reader should not be its file manager.

export async function loadRun(files) {
  if (!files?.length) throw new Error("That run has no files to read.");
  const entries = [...files].map((file) => ({ file, path: file.webkitRelativePath || file.name }));
  const manifests = entries.filter(({ path }) => path === "run.json" || path.endsWith("/run.json"));
  const candidates = manifests.filter(({ path }) => {
    const root = path.slice(0, -"run.json".length);
    return entries.some((entry) => entry.path === `${root}days.jsonl`);
  });
  if (candidates.length > 1)
    throw new Error("This folder contains several runs. Choose one run or its player folder.");
  const root = candidates[0]?.path.slice(0, -"run.json".length) ?? "";
  const byPath = new Map();
  for (const { file, path } of entries) {
    if (path.startsWith(root)) byPath.set(path.slice(root.length), file);
  }
  // The run's own directory names it. Picking `player/` — which the error
  // below invites — named every such run "player".
  const folder = (root.replace(/\/player\/$/, "").replace(/\/$/, "").split("/").pop() || "").trim();
  const runFile = byPath.get("run.json");
  const daysFile = byPath.get("days.jsonl");
  if (!runFile || !daysFile) {
    throw new Error(
      "That run has no run.json and days.jsonl. Run `edet-civitas index` on it first.",
    );
  }
  let run;
  try {
    run = JSON.parse(await runFile.text());
    if (run && folder) run.name = folder;
  } catch {
    throw new Error("run.json is not valid JSON. Re-index the run and try again.");
  }
  if (!run || !Array.isArray(run.persons))
    throw new Error("run.json is missing its list of people. Re-index the run and try again.");
  // **A run being written is the ordinary case, not a corrupt one.** One day
  // is one line, appended as the day closes, so a run still going — or one
  // killed mid-write — ends in half a line. Refusing the file for it cost the
  // reader all the days that WERE complete; a partial tail is dropped instead,
  // and said out loud.
  const lines = (await daysFile.text()).split("\n").filter((l) => l.trim());
  const days = [];
  let unreadable = 0;
  for (const [i, line] of lines.entries()) {
    let day;
    try {
      day = JSON.parse(line);
    } catch {
      if (i === lines.length - 1 && days.length) {
        unreadable += 1;
        break;
      }
      throw new Error(`Could not read snapshot ${i + 1} in days.jsonl.`);
    }
    if (
      !day ||
      !["members", "edges", "contracts", "purses", "events"].every((key) =>
        Array.isArray(day[key]),
      ) ||
      !day.economy?.published ||
      !Number.isFinite(day.economy.index_ppm) ||
      !Number.isFinite(day.epoch) ||
      !Number.isFinite(day.tick) ||
      !Number.isFinite(day.seed)
    ) {
      if (i === lines.length - 1 && days.length) {
        unreadable += 1;
        break;
      }
      throw new Error(`Snapshot ${i + 1} is incomplete. Re-index the run and try again.`);
    }
    day.mail ??= [];
    days.push(day);
  }
  if (!days.length) throw new Error("This run has no snapshots to play yet.");
  // A manifest that does not say what it is gets the careful answer: a tape
  // with no `not_a_run` field is treated as scripted rather than as a run,
  // because the label a scripted tape must carry is the one nothing enforces.
  run = { ...run, not_a_run: run.not_a_run ?? true, partial: unreadable > 0 };
  const lives = new Map();
  for (const [path, f] of byPath) {
    const m = path.match(/^lives\/(\d+)\.json$/);
    if (m) lives.set(Number(m[1]), f);
  }
  return { run, days, lives };
}

/**
 * **A run served beside the player rather than picked from a folder.**
 *
 * `civitas-runs/` is where tapes live and they are never in the repository, so
 * the dev server reads them there and this fetches what it lists. The files
 * are the same files `index` wrote, so they go through `loadRun` unchanged —
 * the validation, the partial-tail rule and the naming are not written twice.
 *
 * A life is fetched only if somebody opens that person: pilot-3's are 37 MB
 * between 128 of them, and a reader who scrubs the timeline never asks.
 */
export async function loadScenarios(base = "") {
  const res = await fetch(`${base}/scenarios.json`, { cache: "no-store" });
  if (!res.ok) return [];
  const list = await res.json();
  return Array.isArray(list) ? list : [];
}

export async function loadScenario(id, base = "") {
  const at = `${base}/scenarios/${encodeURIComponent(id)}`;
  const res = await fetch(`${at}/files.json`, { cache: "no-store" });
  if (!res.ok) throw new Error(`No run called ${id} is being served.`);
  const files = await res.json();
  return loadRun(scenarioEntries(id, files, (path) => `${at}/${path}`));
}

/**
 * The file list as `loadRun` takes it: one entry per path, each fetching only
 * when read. The `<id>/player/` prefix is what names the run, exactly as a
 * picked folder does.
 */
export function scenarioEntries(id, files, url) {
  return files.map((path) => ({
    webkitRelativePath: `${id}/player/${path}`,
    name: path.split("/").pop(),
    async text() {
      const res = await fetch(url(path));
      if (!res.ok) throw new Error(`Could not read ${path} of ${id}.`);
      return res.text();
    },
  }));
}

const cache = new WeakMap();

export async function loadLife(lives, person) {
  if (cache.has(lives) && cache.get(lives).has(person)) return cache.get(lives).get(person);
  const f = lives.get(person);
  const life = f ? JSON.parse(await f.text()) : [];
  if (!cache.has(lives)) cache.set(lives, new Map());
  cache.get(lives).set(person, life);
  return life;
}

export const money = (minor) => {
  if (minor === null || minor === undefined) return "–";
  const sign = minor < 0 ? "-" : "";
  const x = Math.abs(minor);
  return `${sign}${Math.floor(x / 100)}.${String(x % 100).padStart(2, "0")}`;
};

export const personName = (run, i) => {
  if (i === null || i === undefined) return "–";
  const p = run.persons[i];
  if (!p) return `person ${i}`;
  const who = p.member !== null && p.member !== undefined ? `member ${p.member}` : "no account";
  return `${p.card_name || "newcomer"} · ${who}`;
};
