/**
 * Page-driving primitives for the smoke: poll the DOM, click what a check
 * names the way a pointer does, and type into the app's controlled fields.
 *
 * A click goes through CDP's `Input` domain at a point the page itself verified
 * — visible, enabled, and hit-testable — rather than through `element.click()`,
 * which fires a handler even for a button a person could not reach.
 */
import { evaluate } from "./cdp.mjs";

const POLL_MS = 250;
/** One evaluation inside a poll may not outlive the poll's own window. */
const EVALUATE_CAP_MS = 5_000;

export function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Poll an expression until it is truthy or the deadline passes. */
export async function waitFor(session, expression, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const remaining = deadline - Date.now();
    if (remaining <= 0) return false;
    try {
      if (
        await evaluate(session, expression, Math.max(1_000, Math.min(EVALUATE_CAP_MS, remaining)))
      ) {
        return true;
      }
    } catch {
      // A page mid-navigation can refuse an evaluation; the next poll decides.
    }
    const left = deadline - Date.now();
    if (left <= 0) return false;
    await sleep(Math.min(POLL_MS, left));
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

/**
 * Where a pointer could actually land on an element the finder expression
 * returns: null when it is missing, disabled, zero-sized, invisible, or covered
 * at its own centre.
 */
function pointExpression(finder) {
  return `(() => {
    const element = (${finder});
    if (element === null || element === undefined || element.disabled === true) return null;
    if (element.checkVisibility !== undefined && !element.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return null;
    const rect = element.getBoundingClientRect();
    if (rect.width < 1 || rect.height < 1) return null;
    const x = rect.left + rect.width / 2;
    const y = rect.top + rect.height / 2;
    if (x < 0 || y < 0 || x > innerWidth || y > innerHeight) return null;
    const hit = document.elementFromPoint(x, y);
    if (hit === null || !(hit === element || element.contains(hit))) return null;
    return { x, y };
  })()`;
}

function selectorFinder(selector) {
  return `document.querySelector(${JSON.stringify(selector)})`;
}

function textFinder(selector, text) {
  return `[...document.querySelectorAll(${JSON.stringify(selector)})].find((element) => (element.textContent ?? "").trim() === ${JSON.stringify(text)}) ?? null`;
}

/** The clickable centre of an element, or null with a sentence saying why not. */
async function pointOf(session, finder, timeoutMs, description) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const remaining = deadline - Date.now();
    if (remaining <= 0)
      return { point: null, reason: `${description} never became visible and enabled` };
    let point = null;
    try {
      point = await evaluate(
        session,
        pointExpression(finder),
        Math.max(1_000, Math.min(EVALUATE_CAP_MS, remaining)),
      );
    } catch (error) {
      return { point: null, reason: String(error?.message ?? error) };
    }
    if (point !== null) return { point, reason: null };
    await sleep(Math.min(POLL_MS, Math.max(0, deadline - Date.now())));
  }
}

async function clickPoint(session, point, timeoutMs) {
  await session.send(
    "Input.dispatchMouseEvent",
    { type: "mousePressed", x: point.x, y: point.y, button: "left", buttons: 1, clickCount: 1 },
    timeoutMs,
  );
  await session.send(
    "Input.dispatchMouseEvent",
    { type: "mouseReleased", x: point.x, y: point.y, button: "left", buttons: 0, clickCount: 1 },
    timeoutMs,
  );
}

/**
 * Click the one element a selector names, at its verified centre, through the
 * browser's own input pipeline.
 */
export async function click(session, selector, timeoutMs = 10_000) {
  const { point, reason } = await pointOf(session, selectorFinder(selector), timeoutMs, selector);
  if (point === null) throw new Error(`nothing clickable matches ${selector} (${reason})`);
  await clickPoint(session, point, timeoutMs);
}

/**
 * Click the first element a selector names whose trimmed text is exactly
 * `text` — the road the permission cards need, where the control is named by
 * its label rather than by a class of its own.
 */
export async function clickByText(session, selector, text, timeoutMs = 10_000) {
  const { point, reason } = await pointOf(
    session,
    textFinder(selector, text),
    timeoutMs,
    `${selector} "${text}"`,
  );
  if (point === null)
    throw new Error(`nothing clickable matches ${selector} "${text}" (${reason})`);
  await clickPoint(session, point, timeoutMs);
}

/**
 * Put text into a focused input or textarea. The field is clicked first (the
 * app's controls are React-controlled and only a real focus plus a real text
 * input event reach their state), then the text is inserted through the
 * browser's editing pipeline, and the value is read back: a field that did not
 * take the text is an error here, not a silently empty send later.
 */
export async function typeInto(session, selector, text, timeoutMs = 10_000) {
  await click(session, selector, timeoutMs);
  await session.send("Input.insertText", { text }, timeoutMs);
  const value = await evaluate(
    session,
    `document.querySelector(${JSON.stringify(selector)})?.value ?? null`,
    timeoutMs,
  );
  if (value === text) return;
  // Some controlled fields swallow insertText; the native setter plus an input
  // event is the fallback that reaches React's own tracker.
  const typed = await evaluate(
    session,
    `(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (element === null) return null;
      const prototype =
        element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(prototype, "value").set.call(element, ${JSON.stringify(text)});
      element.dispatchEvent(new Event("input", { bubbles: true }));
      return element.value;
    })()`,
    timeoutMs,
  );
  if (typed !== text) throw new Error(`the field ${selector} did not take the text`);
}

/**
 * Open the crescent navigation and pick a surface, the way the app's own
 * pointer path does it: the sliver opens the band, the band's point switches
 * the surface, and the surface root is the assertion that the click landed.
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
