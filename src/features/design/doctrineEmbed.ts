// The doctrine block framing, pure: delimiters, disclaimer, restatement.
//
// Moved whole from `agentHost.ts` so the daemon port and the conformance
// fixtures share the exact framing strings through the manifest;
// `agentHost` re-exports every name, so no caller changes what it imports.

import craftManifest from "./craft/manifest.json";

// The doctrine block is the last substantial text the agent reads, so this line
// is the only thing standing against recency.  It is deliberately a directive
// rather than a description, and it does not restate the constraints verbatim:
// a second copy would be free to drift out of step with the first.
export const DESIGN_DOCTRINE_RESTATEMENT: string = craftManifest.delimiters.restatement;

export const DESIGN_DOCTRINE_BEGIN: string = craftManifest.delimiters.begin;
export const DESIGN_DOCTRINE_END: string = craftManifest.delimiters.end;

export const DESIGN_DOCTRINE_DISCLAIMER: string = craftManifest.delimiters.disclaimer;
export const DOCTRINE_DELIMITER_REMOVED: string = craftManifest.delimiters.removed;

export function embedDoctrineBlock(composedText: string): string {
  if (composedText.length === 0) return "";
  const neutralized = composedText
    .split(DESIGN_DOCTRINE_BEGIN)
    .join(DOCTRINE_DELIMITER_REMOVED)
    .split(DESIGN_DOCTRINE_END)
    .join(DOCTRINE_DELIMITER_REMOVED);
  return [DESIGN_DOCTRINE_BEGIN, DESIGN_DOCTRINE_DISCLAIMER, neutralized, DESIGN_DOCTRINE_END].join(
    "\n\n",
  );
}
