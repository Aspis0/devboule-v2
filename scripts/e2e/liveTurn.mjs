/**
 * One real agent turn through the app: start an agent in the smoke's workspace
 * the way a person does, ask it to read a file, edit it and run a command, and
 * wait for the turn to finish and both effects to be observable on disk.
 *
 * Local-only, behind DEVBOULE_E2E_LIVE_PROVIDER=pi|codex: CI has no provider
 * accounts and the turn spends real tokens. The provider is an allowlist and
 * the session that actually opened is checked against it before anything is
 * sent; the one provider this must not run is Claude.
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  click,
  clickByText,
  requireSelector,
  selectorPresent,
  sleep,
  typeInto,
  waitFor,
} from "./drive.mjs";
import { evaluate } from "./cdp.mjs";

const TURN_BUDGET_MS = 10 * 60_000;
const EDIT = "edited by the e2e smoke";

const PROMPT = [
  "Read hello.txt in this folder, then do all of this without asking:",
  `1. Replace the file's contents with exactly: ${EDIT}`,
  "2. Run the shell command `node --version` and write its output — one line, nothing else — to version.txt in this folder.",
  "Reply with the version when you are done.",
].join("\n");

/** The version the agent's own command must have produced. */
function expectedNodeVersion() {
  const result = spawnSync("node", ["--version"], { encoding: "utf8", windowsHide: true });
  if (result.status !== 0) throw new Error("node --version did not answer on this machine");
  return (result.stdout ?? "").trim();
}

/** Click the option of whichever menu is open (new tab, provider picker). */
async function clickOption(session, name) {
  await clickByText(session, "button.workspace-surface-option .workspace-surface-name", name);
}

/**
 * Answer a permission card, and only ever with a one-time approval: an
 * unattended loop must not grant a durable or session-wide permission the
 * person did not see. A card offering nothing but "always" is left standing,
 * and the turn fails on its own budget.
 */
async function approvePermissionOnce(session) {
  return evaluate(
    session,
    `(() => {
      const card = [...document.querySelectorAll(".permission-card")].find(
        (element) => element.querySelector(".permission-card-actions") !== null,
      );
      if (card === undefined) return false;
      const once = [...card.querySelectorAll("button")].find((button) => {
        if (button.disabled) return false;
        const text = (button.textContent ?? "").trim().toLowerCase();
        if (/always|session|for all|permanent|everything/.test(text)) return false;
        return text === "allow once" || text === "allow";
      });
      if (once === undefined) return false;
      once.click();
      return true;
    })()`,
  );
}

function readIfPresent(path) {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return null;
  }
}

/** The session the app just opened, and the provider it actually runs. */
async function openedSession(session) {
  const rows = await evaluate(session, `window.__TAURI_INTERNALS__.invoke("sessions_list")`);
  const dated = rows.filter((row) => typeof row.createdAtMs === "number");
  dated.sort((left, right) => right.createdAtMs - left.createdAtMs);
  return dated[0] ?? null;
}

/**
 * The rail's own word for the session's turn: `working` while it runs, `idle`
 * once it has finished, `Failed` when the provider refused. The protocol's
 * `sessions_list` rows carry no activity (that field rides the roster push
 * only), so the app's rendering is the observable.
 */
const SUB_WORD_EXPRESSION = `document.querySelector(".workspace-agent-row .workspace-agent-sub")?.textContent?.trim() ?? ""`;
const SETTLED_EXPRESSION = `(() => {
  const sub = ${SUB_WORD_EXPRESSION};
  return sub.startsWith("idle") || sub.startsWith("Done");
})()`;
/** The two words that end the wait early: no once-only answer can rescue them. */
const STALLED_EXPRESSION = `(() => {
  const sub = ${SUB_WORD_EXPRESSION};
  return sub.startsWith("Failed") || sub.startsWith("Needs your approval");
})()`;

/**
 * Start the agent, send the prompt, and wait for the turn to complete with both
 * files on disk. Throws with what was missing when the budget runs out.
 */
export async function runLiveTurn({ session, provider, repoDir }) {
  const startedAt = Date.now();
  const helloPath = join(repoDir, "hello.txt");
  const versionPath = join(repoDir, "version.txt");
  const expected = expectedNodeVersion();

  await click(session, "button.workspace-row");
  await click(session, '[aria-label="New tab"]');
  await requireSelector(
    session,
    '[role="menu"][aria-label="New tab"]',
    15_000,
    "the New tab menu did not open",
  );
  await clickOption(session, "Agent");

  // Two roads: one capable provider starts straight away, more than one opens
  // the picker. The composer is the common end of both.
  const composer = 'textarea[aria-label="Message the agent"]';
  const picked = await waitFor(
    session,
    `document.querySelector('[aria-label="Choose agent"]') !== null`,
    10_000,
  );
  if (picked) {
    await clickOption(session, provider);
    // An npx wrapper asks for consent before it downloads anything; the flag
    // named the provider, so the download is the person's own choice.
    if (await selectorPresent(session, '[aria-label="Confirm agent"]', 2_000)) {
      await click(session, ".workspace-consent-actions .workspace-primary-action");
    }
  }
  await requireSelector(
    session,
    composer,
    60_000,
    `the ${provider} session did not open its composer`,
  );

  // What opened is what was asked for: a single-provider machine auto-starts
  // whichever CLI it has, and this turn must not run under a provider the
  // allowlist did not name.
  const opened = await openedSession(session);
  if (opened === null) throw new Error("the app listed no session after the composer opened");
  if (opened.kind !== provider) {
    throw new Error(`the session that opened is ${opened.kind}, not ${provider}`);
  }

  const typing = `(() => {
    const field = document.querySelector(${JSON.stringify(composer)});
    return field !== null && !field.disabled;
  })()`;
  if (!(await waitFor(session, typing, 60_000))) {
    throw new Error("the composer never became ready for a message");
  }
  await typeInto(session, composer, PROMPT);
  // The send control is disabled while the draft is empty, so it is waited for
  // after typing, not before.
  const sendable = `(() => {
    const send = document.querySelector(".workspace-send-action");
    return send !== null && !send.disabled;
  })()`;
  if (!(await waitFor(session, sendable, 10_000))) {
    throw new Error("the send control stayed disabled after the prompt was typed");
  }
  await click(session, ".workspace-send-action");

  const deadline = startedAt + TURN_BUDGET_MS;
  let version = null;
  let completed = false;
  let stalled = null;
  while (Date.now() < deadline) {
    const hello = (readIfPresent(helloPath) ?? "").trim();
    const written = (readIfPresent(versionPath) ?? "").trim();
    if (
      hello === EDIT &&
      written === expected &&
      (await waitFor(session, SETTLED_EXPRESSION, 1_000))
    ) {
      version = written;
      completed = true;
      break;
    }
    if (await evaluate(session, STALLED_EXPRESSION).catch(() => false)) {
      stalled = await evaluate(session, SUB_WORD_EXPRESSION).catch(() => "unknown");
      break;
    }
    await approvePermissionOnce(session);
    await sleep(1000);
  }
  const seconds = Math.round((Date.now() - startedAt) / 1000);
  if (!completed) {
    const hello = (readIfPresent(helloPath) ?? "").trim();
    const written = (readIfPresent(versionPath) ?? "").trim();
    const sub = stalled ?? (await evaluate(session, SUB_WORD_EXPRESSION).catch(() => "unknown"));
    throw new Error(
      `the turn did not complete in ${seconds}s: hello.txt ${hello === EDIT ? "matches" : `is ${JSON.stringify(hello)}`}, ` +
        `version.txt ${written === expected ? "matches" : `is ${JSON.stringify(written)}`} (expected ${expected}), ` +
        `session says ${JSON.stringify(sub)}`,
    );
  }
  return `${provider} turn in ${seconds}s: hello.txt rewritten, version.txt = ${version}`;
}
