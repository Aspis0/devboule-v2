import { readFile } from "node:fs/promises";

const files = ["dist/index.html", "dist/assets/polis.js"];
for (const file of files) {
  const source = await readFile(file, "utf8");
  const paths = source.match(/(?:src|href)=["']\/(?!\/)[^"']+/g) ?? [];
  const pluginPaths = source.match(/["'`]\/(?:assets|atlas|polis)\//g) ?? [];
  const externalUrls = source.match(/https?:\/\//g) ?? [];
  console.log(
    `${file}: root-relative attributes=${paths.length}, plugin-root literals=${pluginPaths.length}, external-url literals=${externalUrls.length}`,
  );
}

const manifest = JSON.parse(await readFile("dist/plugin.json", "utf8"));
console.log(`manifest: id=${manifest.id}, files=${Object.keys(manifest.files).length}`);
