import { defineConfig } from "vite";

export default defineConfig({
  // The host mounts this document below /<plugin-id>/. Relative URLs keep
  // bundled assets inside that plugin namespace on the custom asset scheme.
  base: "./",
  build: {
    emptyOutDir: true,
    // This build is one self-contained file. The preload helper is unnecessary
    // and constructs root-relative URLs that would escape the plugin namespace.
    modulePreload: false,
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
        entryFileNames: "assets/polis.js",
        chunkFileNames: "assets/[name].js",
        assetFileNames: "assets/[name][extname]",
      },
    },
  },
});
