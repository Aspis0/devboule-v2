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
          // Cross-origin plugin frames surface as their own targets; depending
          // on the WebView2 build they are typed "page" or "iframe". The URL
          // match below is what actually picks the document.
          (target.type === "page" || target.type === "iframe") &&
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
function replay(webSocketDebuggerUrl, steps) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(webSocketDebuggerUrl);
    const replies = [];
    let index = 0;
    let id = 0;
    const timer = setTimeout(() => {
      socket.close();
      reject(new Error("the WebView did not answer within 60s"));
    }, 60_000);

    function next() {
      if (index >= steps.length) {
        clearTimeout(timer);
        socket.close();
        resolve(replies);
        return;
      }
      const step = steps[index];
      index += 1;
      if (typeof step.wait === "number") {
        setTimeout(next, step.wait);
        return;
      }
      id += 1;
      socket.send(JSON.stringify({ id, method: step.method, params: step.params ?? {} }));
    }

    socket.addEventListener("open", next);
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== id) return;
      if (message.error) {
        clearTimeout(timer);
        socket.close();
        reject(new Error(`${steps[index - 1].method}: ${message.error.message}`));
        return;
      }
      replies.push(message.result);
      next();
    });
    socket.addEventListener("error", () => {
      clearTimeout(timer);
      reject(new Error(`could not open ${webSocketDebuggerUrl}`));
    });
  });
}

/**
 * Capture the window as a PNG. Two commands in order, not one: a
 * Page.captureScreenshot on a domain that was never enabled answers with an
 * error instead of a picture, and the error looks like a broken port.
 */
function screenshot(webSocketDebuggerUrl) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(webSocketDebuggerUrl);
    const timer = setTimeout(() => {
      socket.close();
      reject(new Error("the WebView did not answer within 30s"));
    }, 30_000);
    const fail = (error) => {
      clearTimeout(timer);
      socket.close();
      reject(error);
    };

    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ id: 1, method: "Page.enable" }));
    });
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id === 1) {
        if (message.error) return fail(new Error(message.error.message));
        socket.send(
          JSON.stringify({
            id: 2,
            method: "Page.captureScreenshot",
            params: { format: "png" },
          }),
        );
        return;
      }
      if (message.id !== 2) return;
      if (message.error) return fail(new Error(message.error.message));
      clearTimeout(timer);
      socket.close();
      resolve(Buffer.from(message.result.data, "base64"));
    });
    socket.addEventListener("error", () => {
      fail(new Error(`could not open ${webSocketDebuggerUrl}`));
    });
  });
}

const task = readTask(process.argv.slice(2));
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
  if (task.kind === "input") {
    const replies = await replay(target.webSocketDebuggerUrl, task.steps);
    console.log(JSON.stringify(replies.filter((reply) => Object.keys(reply).length > 0), null, 2));
  } else if (task.kind === "screenshot") {
    const png = await screenshot(target.webSocketDebuggerUrl);
    writeFileSync(task.path, png);
    console.error(`wrote ${png.length} bytes to ${task.path}`);
  } else {
    const value = await evaluate(target.webSocketDebuggerUrl, task.expression);
    console.log(JSON.stringify(value, null, 2));
  }
} catch (error) {
  console.error(String(error.message ?? error));
  process.exit(1);
}
