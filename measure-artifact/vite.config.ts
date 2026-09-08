import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const measureDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(measureDir, "..");

export default defineConfig({
  root: repoRoot,
  publicDir: false,
  server: {
    host: "127.0.0.1",
    port: 4177,
    strictPort: true,
    fs: { allow: [repoRoot] },
    watch: {
      ignored: [
        "**/measure-artifact/samples/**",
        "**/measure-artifact/.browser-profile/**",
        "**/measure-artifact/results.json",
        "**/measure-artifact/machine.json",
        "**/target/**",
      ],
    },
  },
});
