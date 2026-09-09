import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte({ hot: false })],
  define: {
    __APP_VERSION__: JSON.stringify("test"),
  },
  test: {
    environment: "jsdom",
    include: ["src/**/__tests__/**/*.test.ts"],
    globals: true,
  },
});
