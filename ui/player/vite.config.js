import { createReadStream, cpSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The version is the workspace's, read where it is written rather than
// copied here to go stale.
const version =
  readFileSync(new URL("../../Cargo.toml", import.meta.url), "utf8").match(
    /^version\s*=\s*"([^"]+)"/m,
  )?.[1] ?? "unknown";

// What a refusal code means is the WALLET's sentence for it, read from the
// client's locale at build time rather than restated here: the player renders
// the same file the client renders, so the two cannot drift and a code added
// to the ledger arrives here with its English or with nothing — never with a
// stale guess.
//
// What `just ci` gates about that file is the code SET (`error-codes.test.ts`,
// against `crates/state/src/errors.rs`) and key parity across locales — NOT
// the English, which is why reading it here rather than copying it is the
// whole of the guarantee. An earlier version of this comment claimed the
// sentences were gated against the client's inline English; they are not, and
// they cannot be: `submit.ts` builds the key as `errors.${code}` and one
// generic fallback covers all 64.
const codes =
  JSON.parse(
    readFileSync(new URL("../wallet/src/locales/en.json", import.meta.url), "utf8"),
  ).errors ?? {};

import { filesOf, INDEXED, scenariosIn } from "./scenarios.mjs";

// **The runs beside the tree, offered without a folder picker.**
//
// `civitas-runs/` is where a tape lives and is never in the repository, so
// this serves it rather than copying it: the list is what you have indexed,
// and it is read at request time so a run finishing while the player is open
// appears on the next look. Nothing leaves the machine — the dev server reads
// a file and hands it to the page on the same host.
const RUNS =
  process.env.CIVITAS_RUNS || fileURLToPath(new URL("../../civitas-runs/", import.meta.url));

function scenarios() {
  const baked = (process.env.CIVITAS_SCENARIOS || "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  return {
    name: "civitas-scenarios",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const url = (req.url || "").split("?")[0];
        if (url === "/scenarios.json") {
          res.setHeader("Content-Type", "application/json");
          return res.end(JSON.stringify(scenariosIn(RUNS).map((s) => ({ ...s, dir: undefined }))));
        }
        if (!url.startsWith("/scenarios/")) return next();
        const rest = decodeURIComponent(url.slice("/scenarios/".length));
        const [id, ...parts] = rest.split("/");
        const found = scenariosIn(RUNS).find((s) => s.id === id);
        // A path is only ever one of the files the index wrote: no traversal,
        // and nothing outside a run's own indexed directory is reachable.
        if (!found) return next();
        const want = parts.join("/");
        if (want === "files.json") {
          res.setHeader("Content-Type", "application/json");
          return res.end(JSON.stringify(filesOf(found.dir)));
        }
        if (!filesOf(found.dir).includes(want)) return next();
        res.setHeader("Content-Type", want.endsWith(".json") ? "application/json" : "text/plain");
        return createReadStream(join(found.dir, want)).pipe(res);
      });
    },
    // A static build carries a run only when somebody names it, because that
    // is a decision about tens of megabytes and not a default.
    closeBundle() {
      if (!baked.length) return;
      const out = join(fileURLToPath(new URL("./dist/scenarios/", import.meta.url)));
      const have = scenariosIn(RUNS).filter((s) => baked.includes(s.id));
      for (const s of have) {
        mkdirSync(join(out, s.id), { recursive: true });
        cpSync(s.dir, join(out, s.id), { recursive: true });
      }
      mkdirSync(out, { recursive: true });
      const listed = have.map((s) => ({ ...s, dir: undefined }));
      writeFileSync(join(out, "..", "scenarios.json"), JSON.stringify(listed));
      for (const s of have) {
        writeFileSync(
          join(out, s.id, "files.json"),
          JSON.stringify(filesOf(s.dir)),
        );
      }
      const missing = baked.filter((id) => !have.some((s) => s.id === id));
      if (missing.length) throw new Error(`CIVITAS_SCENARIOS names no indexed run: ${missing.join(", ")}`);
    },
  };
}

// Runs are read from a directory the viewer picks, or served from
// `civitas-runs/` beside the tree. No data leaves the browser.
export default defineConfig({
  plugins: [svelte(), scenarios()],
  base: "./",
  define: {
    __EDET_VERSION__: JSON.stringify(version),
    __EDET_CODES__: JSON.stringify(codes),
  },
  server: { port: 5190, strictPort: false },
});
