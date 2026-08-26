import { readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

const nodeModules = resolve('node_modules');
const findings = [];
const pending = [nodeModules];

while (pending.length > 0) {
  const directory = pending.pop();
  if (!directory) continue;

  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`Cannot scan ${directory}: ${message}`);
    process.exitCode = 1;
    continue;
  }

  for (const entry of entries) {
    const entryPath = join(directory, entry.name);
    if (entry.isFile() && entry.name === 'npm-shrinkwrap.json') {
      findings.push(entryPath);
      continue;
    }

    if (entry.isDirectory()) {
      pending.push(entryPath);
      continue;
    }

    // pnpm package links are symlinks. The concrete package directory is
    // already present below node_modules/.pnpm, so do not follow links.
    if (entry.isSymbolicLink() && entry.name === 'npm-shrinkwrap.json') {
      try {
        if (statSync(entryPath).isFile()) findings.push(entryPath);
      } catch {
        // A dangling package link is outside this check's scope.
      }
    }
  }
}

if (findings.length > 0) {
  console.error('Forbidden npm-shrinkwrap.json files found inside node_modules:');
  for (const finding of findings.sort()) console.error(`- ${finding}`);
  process.exit(1);
}

console.log('No npm-shrinkwrap.json files found inside node_modules.');
