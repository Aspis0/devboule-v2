/**
 * Page-driving primitives for the smoke: poll the DOM, click what a check
 * names, and type into the app's controlled fields the way keystrokes do.
 */
import { evaluate } from "./cdp.mjs";

const POLL_MS = 250;

export function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Poll an expression until it is truthy or the deadline passes. */
export async function waitFor(session, expression, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      if (await evaluate(session, expression)) return true;
    } catch {
      // A page mid-navigation can refuse an evaluation; the next poll decides.
    }
    if (Date.now() >= deadline) return false;
    await sleep(POLL_MS);
  }
}

export async function selectorPresent(session, selector, timeoutMs) {
  return waitFor(
    session,
    `document.querySelector(${JSON.stringify(selector)}) !== null`,
    timeoutMs,
  );
}

/** Wait for a selector, throwing the check's own sentence when it never appears. */
export async function requireSelector(session, selector, timeoutMs, message) {
  if (!(await selectorPresent(session, selector, timeoutMs))) throw new Error(message);
}

/** Click the one element a selector names. */
export async function click(session, selector) {
  const clicked = await evaluate(
    session,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (element === null) return false;
      element.click();
      return true;
    })()`,
  );
  if (!clicked) throw new Error(`no element matches ${selector}`);
}

/**
 * Put text into a React-controlled input or textarea. Assigning `.value` alone
 * leaves React's own value tracker believing the field never changed, so the
 * native setter is used and an input event is dispatched — the pair a keystroke
 * produces.
 */
export async function typeInto(session, selector, text) {
  const typed = await evaluate(
    session,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (element === null) return false;
      const prototype =
        element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, ${JSON.stringify(text)});
      element.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    })()`,
  );
  if (!typed) throw new Error(`no element matches ${selector}`);
}

/**
 * Open the crescent navigation and pick a surface, the way the app's own
 * pointer path does it: the sliver opens the band, the band's point switches
 * the surface.
 */
export async function selectSurface(session, key, surfaceSelector) {
  await click(session, ".crescent-sliver");
  await requireSelector(
    session,
    ".crescent-nav-open",
    15_000,
    "the surface navigation did not open",
  );
  await click(session, `[data-surface-key="${key}"]`);
  await requireSelector(session, surfaceSelector, 20_000, `the ${key} surface did not render`);
}
