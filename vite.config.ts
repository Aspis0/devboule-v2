import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  test: {
    include: ["src/**/*.{test,spec}.{ts,tsx}", "scripts/**/*.{test,spec}.{mjs,ts}"],
    exclude: ["plugins/**"],
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/target/**"],
    },
  },
});
