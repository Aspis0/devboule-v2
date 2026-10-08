// CLI entry for `pnpm release` and CI's `notes`/`check-tag` forms: argument
// parsing and dispatch only; the flows live in scripts/release/.

import { runCheckTag, runNotes, runRelease } from "./release/flow.mjs";
import { parseReleaseTag } from "./release/versions.mjs";

export function parseReleaseArgs(args) {
  if (args.length === 1 && (args[0] === "patch" || args[0] === "minor" || args[0] === "major")) {
    return { mode: "release", kind: args[0] };
  }
  if (
    args.length === 3 &&
    args[0] === "notes" &&
    parseReleaseTag(args[1]) !== null &&
    args[2] !== ""
  ) {
    return { mode: "notes", version: args[1], output: args[2] };
  }
  if (args.length === 2 && args[0] === "check-tag" && parseReleaseTag(args[1]) !== null) {
    return { mode: "check-tag", version: args[1] };
  }
  throw new Error(
    "usage: release.mjs <patch|minor|major> | release.mjs notes <vX.Y.Z> <file> | release.mjs check-tag <vX.Y.Z>",
  );
}

if (import.meta.main) {
  try {
    const request = parseReleaseArgs(process.argv.slice(2));
    if (request.mode === "release") {
      runRelease(request.kind);
    } else if (request.mode === "check-tag") {
      runCheckTag(request.version);
    } else {
      runNotes(request.version, request.output);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
