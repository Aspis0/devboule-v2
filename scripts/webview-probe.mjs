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
 * JSON. No dependencies: Node has had a global WebSocket since 22.
 *
 *   npm run tauri dev          (with the env var above set)
 *   node scripts/webview-probe.mjs "document.title"
 *   node scripts/webview-probe.mjs --file probe.js
 *
 * Exit codes: 0 the expression evaluated, 1 it threw, 2 no WebView was found.
 */

import { readFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";

// Not 9222. That port is a common default and other WebView2 applications sit
// on it — Lenovo Vantage was already there on the development machine — so a
// probe aimed at it reads someone else's window and believes the answer.
const PORT = Number(process.env.WEBVIEW_DEBUG_PORT ?? 9333);
// The debug port lists every page the browser process owns. Ours is the one
// serving the app, so the target is matched rather than assumed to be first.
const TARGET_URL_MATCH = process.env.WEBVIEW_TARGET_MATCH ?? "localhost:1420";
const ATTACH_TIMEOUT_MS = Number(process.env.WEBVIEW_ATTACH_TIMEOUT_MS ?? 180_000);

function usage(message) {
  console.error(`${message}\n\nusage: node scripts/webview-probe.mjs <expression>`);
  process.exit(2);
}

function readExpression(argv) {
  if (argv[0] === "--file") {
    if (!argv[1]) usage("--file needs a path");
    return readFileSync(argv[1], "utf8");
  }
  if (argv.length === 0) usage("no expression given");
  return argv.join(" ");
}

/**
 * Wait for a page target to appear. The app takes as long as its Rust build
 * takes, so this polls rather than failing on the first refusal.
 */
async function waitForTarget() {
  const deadline = Date.now() + ATTACH_TIMEOUT_MS;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/json/list`);
      const targets = await response.json();
      const page = targets.find(
        (target) =>
          target.type === "page" &&
          target.webSocketDebuggerUrl &&
          String(target.url).includes(TARGET_URL_MATCH),
      );
      if (page) return page;
    } catch {
      // The port is not open until the WebView is created; keep waiting.
    }
    await delay(1000);
  }
  return null;
}

function evaluate(webSocketDebuggerUrl, expression) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(webSocketDebuggerUrl);
    const timer = setTimeout(() => {
      socket.close();
      reject(new Error("the WebView did not answer within 30s"));
    }, 30_000);

    socket.addEventListener("open", () => {
      socket.send(
        JSON.stringify({
          id: 1,
          method: "Runtime.evaluate",
          params: {
            expression,
            awaitPromise: true,
            returnByValue: true,
            // The probes call app code, which is not a user gesture; without
            // this some APIs refuse in ways that look like feature absence.
            userGesture: true,
          },
        }),
      );
    });
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== 1) return;
      clearTimeout(timer);
      socket.close();
      if (message.error) return reject(new Error(message.error.message));
      const { result, exceptionDetails } = message.result;
      if (exceptionDetails) {
        return reject(new Error(exceptionDetails.exception?.description ?? "threw"));
      }
      resolve(result.value);
    });
    socket.addEventListener("error", () => {
      clearTimeout(timer);
      reject(new Error(`could not open ${webSocketDebuggerUrl}`));
    });
  });
}

const expression = readExpression(process.argv.slice(2));
const target = await waitForTarget();
if (!target) {
  console.error(
    `no WebView page matching "${TARGET_URL_MATCH}" on port ${PORT}. Start the app with ` +
      `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=${PORT}, and check ` +
      `that nothing else already holds the port — another WebView2 application on it will ` +
      `answer instead of yours.`,
  );
  process.exit(2);
}

console.error(`attached to ${target.url}`);
try {
  const value = await evaluate(target.webSocketDebuggerUrl, expression);
  console.log(JSON.stringify(value, null, 2));
} catch (error) {
  console.error(String(error.message ?? error));
  process.exit(1);
}
