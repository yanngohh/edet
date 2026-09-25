/**
 * **The runs this player can open without being handed a folder.**
 *
 * A tape is a run's only existence and can be very large — pilot-3 is 44 MB,
 * 37 of it the people's own words — so it is kept beside the tree and never in
 * it (`.gitignore`). The player therefore does not BUNDLE runs: the dev server
 * reads whatever `civitas-runs/` holds and serves it, so the list is the runs
 * you actually have, and nothing is copied anywhere.
 *
 * A static build carries none of them unless `CIVITAS_SCENARIOS` names some,
 * which is the one case where somebody has decided the size is worth it —
 * publishing a player with a run inside it, for people who cannot run one.
 *
 * Nothing here decides what a run MEANS. `not_a_run` rides through from the
 * manifest, because a scripted tape must say so wherever it is offered.
 */

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

/** Where a run's indexed files live, relative to the run's own directory. */
export const INDEXED = "player";

/**
 * One scenario, as the picker shows it. Pure, so it can be tested without a
 * disk: everything it says comes from the manifest `index` wrote.
 */
export function describeRun(id, run, bytes) {
  return {
    id,
    people: Array.isArray(run?.persons) ? run.persons.length : 0,
    days: Array.isArray(run?.ticks) ? run.ticks.length : 0,
    model: run?.model || "",
    backend: run?.backend || "",
    // A scripted tape is not a run, and says so here as it does in the player.
    not_a_run: run?.not_a_run === true,
    control: run?.control === true,
    ended: run?.ended || "",
    world_seed: run?.world_seed,
    bytes: bytes || 0,
  };
}

/** Every indexed run under `root`, newest first. */
export function scenariosIn(root) {
  if (!existsSync(root)) return [];
  const out = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const dir = join(root, entry.name, INDEXED);
    const manifest = join(dir, "run.json");
    const days = join(dir, "days.jsonl");
    if (!existsSync(manifest) || !existsSync(days)) continue;
    let run;
    try {
      run = JSON.parse(readFileSync(manifest, "utf8"));
    } catch {
      continue; // a half-written index is not an offer
    }
    const bytes = statSync(days).size + statSync(manifest).size;
    out.push({ ...describeRun(entry.name, run, bytes), dir, mtime: statSync(days).mtimeMs });
  }
  return out.sort((a, b) => b.mtime - a.mtime).map(({ mtime, ...rest }) => rest);
}

/** The files a scenario is made of, as paths under its indexed directory. */
export function filesOf(dir) {
  const lives = join(dir, "lives");
  const names = existsSync(lives)
    ? readdirSync(lives)
        .filter((n) => n.endsWith(".json"))
        .map((n) => `lives/${n}`)
    : [];
  return ["run.json", "days.jsonl", ...names];
}
