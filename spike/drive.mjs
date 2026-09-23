// SPIKE ONLY — file-RPC driver: write spike/cmd.json, wait for the matching
// result line in spike/out.ndjson.
// Usage: node spike/drive.mjs <op> ['{"json":"args"}']
// Exit 0 + one JSON result line on stdout, 1 on op error, 2 on timeout.
import { readFileSync, writeFileSync, existsSync } from "node:fs";

const [op, argsJson] = process.argv.slice(2);
if (!op) {
  console.error("usage: node spike/drive.mjs <op> [jsonArgs]");
  process.exit(64);
}

const CMD = "spike/cmd.json";
const OUT = "spike/out.ndjson";

let id = Date.now();
if (existsSync(CMD)) {
  try {
    const prev = JSON.parse(readFileSync(CMD, "utf8"));
    if (typeof prev.id === "number" && prev.id >= id) id = prev.id + 1;
  } catch {}
}
const cmd = { id, op, args: argsJson ? JSON.parse(argsJson) : {} };
writeFileSync(CMD, JSON.stringify(cmd));

const deadline = Date.now() + 90000;
const seen = new Set();

async function tick() {
  if (existsSync(OUT)) {
    for (const line of readFileSync(OUT, "utf8").split("\n")) {
      if (!line.trim() || seen.has(line)) continue;
      seen.add(line);
      let entry;
      try {
        entry = JSON.parse(line);
      } catch {
        continue;
      }
      if (entry.id === cmd.id) {
        console.log(line);
        process.exit(entry.ok === false ? 1 : 0);
      }
    }
  }
  if (Date.now() > deadline) {
    console.error(`TIMEOUT waiting for op=${op} id=${cmd.id}`);
    process.exit(2);
  }
  await new Promise((r) => setTimeout(r, 100));
}

for (;;) await tick();
