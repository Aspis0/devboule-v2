import { readFile } from "node:fs/promises";

const files = ["dist/index.html", "dist/assets/polis.js"];
let failed = false;
for (const file of files) {
  const source = await readFile(file, "utf8");
  const paths = source.match(/(?:src|href)=["']\/(?!\/)[^"']+/g) ?? [];
  const pluginPaths = source.match(/["'`]\/(?:assets|atlas|polis)\//g) ?? [];
  const externalUrls = source.match(/https?:\/\//g) ?? [];
  console.log(
    `${file}: root-relative attributes=${paths.length}, plugin-root literals=${pluginPaths.length}, external-url literals=${externalUrls.length}`,
  );
  if (paths.length > 0 || pluginPaths.length > 0) {
    console.error(
      `${file}: REJECTED — bundled output must not contain root-relative URLs (they would escape the plugin namespace on the custom asset scheme)`,
    );
    failed = true;
  }
}

const manifest = JSON.parse(await readFile("dist/plugin.json", "utf8"));
console.log(`manifest: id=${manifest.id}, files=${Object.keys(manifest.files).length}`);

if (failed) process.exit(1);
