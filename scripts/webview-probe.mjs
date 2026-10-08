/**
 * Ask the running app's WebView a question and print the answer.
 *
 * Some facts are only true inside WebView2 and cannot be checked from a normal
 * browser or from Rust: whether a WebGL context is hardware-backed, whether the
 * Content-Security-Policy lets a plugin module load, what origin a registered
 * URI scheme actually gets. Until now those were assumed, and one of them —
 * "PixiJS needs unsafe-eval" — sat wrong in the config for a milestone.
 *
 * WebView2 speaks the Chrome DevTools Protocol when the app is started with
 * `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`. This
 * connects to it, evaluates an expression in the page, and prints the result as
 * JSON. The connection itself lives in scripts/e2e/cdp.mjs, shared with the CI
 * smoke that drives the same window.
 *
 * It also takes the screenshot, because what a plugin *draws* is the other
 * fact no amount of reading gives: every visual defect this milestone found —
 * roads buried under building footprints, a plugin asset requested at an
 * absolute path — was found by looking, and the agents doing the work have no
 * browser of their own.
 *
 *   npm run tauri dev          (with the env var above set)
 *   node scripts/webview-probe.mjs "document.title"
 *   node scripts/webview-probe.mjs --file probe.js
 *   node scripts/webview-probe.mjs --screenshot ../recon/city.png
 *   node scripts/webview-probe.mjs --input steps.json
 *
 * Exit codes: 0 it worked, 1 the expression threw, 2 no WebView was found.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { CdpSession, captureScreenshot, evaluate, findPageTarget } from "./e2e/cdp.mjs";

// Not 9222. That port is a common default and other WebView2 applications sit
// on it — Lenovo Vantage was already there on the development machine — so a
// probe aimed at it reads someone else's window and believes the answer.
const PORT = Number(process.env.WEBVIEW_DEBUG_PORT ?? 9333);
// Two origins serve the same app: `pnpm dev` serves it from the vite server and
// a packaged build serves it from tauri.localhost, so the default accepts both
// — a default that only knew the dev server made every probe against a real
// build time out for its full attach window and report an empty port, which
// reads as a dead app rather than a wrong default. Comma-separated; the
// environment variable overrides the whole list.
const TARGET_URL_MATCHES = (process.env.WEBVIEW_TARGET_MATCH ?? "tauri.localhost,localhost:1420")
  .split(",")
  .map((entry) => entry.trim())
  .filter((entry) => entry.length > 0);
// The app takes as long as its Rust build takes, so the attach polls rather
// than failing on the first refusal.
const ATTACH_TIMEOUT_MS = Number(process.env.WEBVIEW_ATTACH_TIMEOUT_MS ?? 180_000);

function usage(message) {
  console.error(
    `${message}\n\nusage: node scripts/webview-probe.mjs <expression>\n` +
      `       node scripts/webview-probe.mjs --file <path>\n` +
      `       node scripts/webview-probe.mjs --screenshot <path.png>`,
  );
  process.exit(2);
}

function readTask(argv) {
  if (argv[0] === "--input") {
    if (!argv[1]) usage("--input needs a path to a JSON list of steps");
    return { kind: "input", steps: JSON.parse(readFileSync(argv[1], "utf8")) };
  }
  if (argv[0] === "--screenshot") {
    if (!argv[1]) usage("--screenshot needs a path");
    return { kind: "screenshot", path: argv[1] };
  }
  if (argv[0] === "--file") {
    if (!argv[1]) usage("--file needs a path");
    return { kind: "evaluate", expression: readFileSync(argv[1], "utf8") };
  }
  if (argv.length === 0) usage("no expression given");
  return { kind: "evaluate", expression: argv.join(" ") };
}

/**
 * Replay a list of CDP steps against the window: `{ method, params }` for a
 * command, `{ wait: ms }` to let the page settle between them.
 *
 * Input has to be dispatched at the window rather than synthesised in the
 * page, because the only thing worth driving lives in a cross-origin iframe:
 * a MouseEvent constructed in the host document cannot reach it, and the frame
 * is not exposed as a separate debugging target either. A real click at real
 * window coordinates lands wherever the compositor says it lands, which is the
 * same thing that happens to a person.
 */
async function replay(session, steps) {
  const deadline = Date.now() + 60_000;
  const replies = [];
  for (const step of steps) {
    if (typeof step.wait === "number") {
      await new Promise((resolve) => setTimeout(resolve, step.wait));
      continue;
    }
    const remaining = deadline - Date.now();
    if (remaining <= 0) throw new Error("the WebView did not answer within 60s");
    replies.push(await session.send(step.method, step.params ?? {}, remaining));
  }
  return replies;
}

const task = readTask(process.argv.slice(2));
const { target } = await findPageTarget({
  port: PORT,
  urlMatches: TARGET_URL_MATCHES,
  timeoutMs: ATTACH_TIMEOUT_MS,
});
if (!target) {
  console.error(
    `no WebView page matching ${TARGET_URL_MATCHES.map((match) => `"${match}"`).join(" or ")} ` +
      `on port ${PORT}. Start the app with ` +
      `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=${PORT}, and check ` +
      `that nothing else already holds the port — another WebView2 application on it will ` +
      `answer instead of yours.`,
  );
  process.exit(2);
}

console.error(`attached to ${target.url}`);
try {
  const session = await CdpSession.open(target.webSocketDebuggerUrl);
  try {
    if (task.kind === "input") {
      const replies = await replay(session, task.steps);
      console.log(
        JSON.stringify(
          replies.filter((reply) => Object.keys(reply).length > 0),
          null,
          2,
        ),
      );
    } else if (task.kind === "screenshot") {
      const png = await captureScreenshot(session);
      writeFileSync(task.path, png);
      console.error(`wrote ${png.length} bytes to ${task.path}`);
    } else {
      const value = await evaluate(session, task.expression);
      console.log(JSON.stringify(value, null, 2));
    }
  } finally {
    session.close();
  }
} catch (error) {
  console.error(String(error.message ?? error));
  process.exit(1);
}
