/**
 * One real agent turn through the app: start an agent in the smoke's workspace
 * the way a person does, ask it to read a file, edit it and run a command, and
 * wait for both effects on disk.
 *
 * Local-only, behind DEVBOULE_E2E_LIVE_PROVIDER=pi|codex: CI has no provider
 * accounts and the turn spends real tokens. The provider is named by the flag
 * and never defaulted — the one provider this must not run is Claude.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { click, requireSelector, selectorPresent, sleep, typeInto, waitFor } from "./drive.mjs";
import { evaluate } from "./cdp.mjs";

const TURN_BUDGET_MS = 10 * 60_000;
const EDIT = "edited by the e2e smoke";

const PROMPT = [
  "Read hello.txt in this folder, then do all of this without asking:",
  `1. Replace the file's contents with exactly: ${EDIT}`,
  "2. Run the shell command `node --version` and write its output to version.txt in this folder.",
  "Reply with the version when you are done.",
].join("\n");

/** Click the option of whichever menu is open (new tab, provider picker). */
async function clickOption(session, name) {
  const clicked = await evaluate(
    session,
    `(() => {
      const option = [...document.querySelectorAll("button.workspace-surface-option")].find(
        (element) => element.querySelector(".workspace-surface-name")?.textContent === ${JSON.stringify(name)},
      );
      if (option === undefined) return false;
      option.click();
      return true;
    })()`,
  );
  if (!clicked) throw new Error(`no menu option named ${name} is on screen`);
}

/**
 * Answer whatever permission card is up, preferring a durable allow: an
 * unattended smoke cannot wait for a person, and a card left parked ends the
 * turn. Returns true when a card was answered.
 */
async function approvePermissionCard(session) {
  return evaluate(
    session,
    `(() => {
      const card = [...document.querySelectorAll(".permission-card")].find(
        (element) => element.querySelector(".permission-card-actions") !== null,
      );
      if (card === undefined) return false;
      const allows = [...card.querySelectorAll("button")].filter(
        (button) => !button.disabled && /allow/i.test(button.textContent ?? ""),
      );
      const allow = allows.find((button) => /always/i.test(button.textContent ?? "")) ?? allows[0];
      if (allow === undefined) return false;
      allow.click();
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

/**
 * Start the agent, send the prompt, and wait for the two files the turn was
 * asked to write. Throws with what was missing when the budget runs out.
 */
export async function runLiveTurn({ session, provider, repoDir }) {
  const startedAt = Date.now();
  const helloPath = join(repoDir, "hello.txt");
  const versionPath = join(repoDir, "version.txt");

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
  while (Date.now() < deadline) {
    if ((readIfPresent(helloPath) ?? "").trim() === EDIT) {
      version = (readIfPresent(versionPath) ?? "").trim();
      if (version !== "") break;
    }
    await approvePermissionCard(session);
    await sleep(1000);
  }
  const seconds = Math.round((Date.now() - startedAt) / 1000);
  if (version === null || version === "") {
    const edited = (readIfPresent(helloPath) ?? "").trim() === EDIT;
    throw new Error(
      `the turn did not finish in ${seconds}s: hello.txt ${edited ? "was rewritten" : "was not rewritten"}, version.txt missing`,
    );
  }
  return `${provider} turn in ${seconds}s: hello.txt rewritten, version.txt = ${version}`;
}
