import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { readFileSync } from "fs";
import { fileURLToPath } from "url";

const pkg = JSON.parse(readFileSync(fileURLToPath(new URL("package.json", import.meta.url)), "utf8"));

export default defineConfig({
  plugins: [svelte()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  // EDET_* env vars flow into the client: the dev harness binds each UI
  // instance to its node (EDET_NODE) and founder identity (EDET_FOUNDER).
  envPrefix: ["VITE_", "EDET_"],
  publicDir: "public",
  server: { port: 5173, strictPort: false },
});
