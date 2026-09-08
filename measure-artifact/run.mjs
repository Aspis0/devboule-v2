/**
 * Drive a real Chromium (Edge, closest to the app's WebView2) against the
 * measurement harness. Writes measure-artifact/results.json incrementally.
 */
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, existsSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { generateSamples } from "./generate-samples.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");
const VITE_PORT = 4177;
const CDP_PORT = 9334;
const PAGE_URL = `http://127.0.0.1:${VITE_PORT}/measure-artifact/index.html`;
const USER_DATA = path.join(os.tmpdir(), "devboule-measure-artifact-profile");
const RESULTS_PATH = path.join(here, "results.json");
const MACHINE_PATH = path.join(here, "machine.json");

const BROWSER_CANDIDATES = [
  process.env.MEASURE_BROWSER,
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
].filter(Boolean);

function browserPath() {
  for (const candidate of BROWSER_CANDIDATES) {
    if (existsSync(candidate)) return candidate;
  }
  throw new Error("No Edge/Chrome binary found");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitForHttp(url, timeoutMs, test) {
  const deadline = Date.now() + timeoutMs;
  let last = "";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      last = `${response.status}`;
      if (test(response)) return response;
    } catch (error) {
      last = error instanceof Error ? error.message : String(error);
    }
    await sleep(200);
  }
  throw new Error(`timeout waiting for ${url} (${last})`);
}

function spawnLogged(command, args, extra = {}) {
  const child = spawn(command, args, {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
    ...extra,
  });
  child.stdout.on("data", (chunk) => process.stdout.write(`[${path.basename(command)}] ${chunk}`));
  child.stderr.on("data", (chunk) => process.stderr.write(`[${path.basename(command)} err] ${chunk}`));
  return child;
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    ws.addEventListener("open", () => resolve(ws));
    ws.addEventListener("error", (event) => {
      reject(new Error(`ws error: ${event.message ?? "unknown"}`));
    });
  });
}

function makeCdp(ws) {
  let nextId = 1;
  const pending = new Map();
  const eventHandlers = new Map();
  ws.addEventListener("message", (event) => {
    const msg = JSON.parse(String(event.data));
    if (msg.id != null) {
      const waiter = pending.get(msg.id);
      if (!waiter) return;
      pending.delete(msg.id);
      if (msg.error) waiter.reject(new Error(`${waiter.method}: ${JSON.stringify(msg.error)}`));
      else waiter.resolve(msg.result);
      return;
    }
    if (msg.method) {
      for (const handler of eventHandlers.get(msg.method) ?? []) handler(msg.params);
    }
  });
  return {
    on(method, handler) {
      const list = eventHandlers.get(method) ?? [];
      list.push(handler);
      eventHandlers.set(method, list);
    },
    send(method, params = {}, timeoutMs = 120_000) {
      const id = nextId++;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`CDP timeout ${method} after ${timeoutMs}ms`));
        }, timeoutMs);
        pending.set(id, {
          method,
          resolve: (value) => {
            clearTimeout(timer);
            resolve(value);
          },
          reject: (error) => {
            clearTimeout(timer);
            reject(error);
          },
        });
        ws.send(JSON.stringify({ id, method, params }));
      });
    },
  };
}

async function pageTarget() {
  const deadline = Date.now() + 30_000;
  let last = "no response";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`);
      const targets = await response.json();
      last = JSON.stringify(targets.map((target) => ({ type: target.type, url: target.url })));
      const page =
        targets.find((target) => target.type === "page" && String(target.url).includes(":4177")) ??
        targets.find((target) => target.type === "page");
      if (page) return page;
    } catch (error) {
      last = error instanceof Error ? error.message : String(error);
    }
    await sleep(200);
  }
  throw new Error(`CDP on ${CDP_PORT} has no page target (${last})`);
}

function machineInfo(browserVersion) {
  const cpus = os.cpus();
  return {
    os: `${os.type()} ${os.release()}`,
    platform: os.platform(),
    arch: os.arch(),
    hostname: os.hostname(),
    cpu: cpus[0]?.model ?? "unknown",
    cores: cpus.length,
    totalmemBytes: os.totalmem(),
    node: process.version,
    browser: browserVersion,
    measuredAt: new Date().toISOString(),
  };
}

function killTree(child) {
  if (!child || child.killed) return;
  try {
    spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
  } catch {
    child.kill();
  }
}

async function main() {
  mkdirSync(USER_DATA, { recursive: true });
  console.log("generating samples");
  const manifest = generateSamples();

  const vite = spawnLogged(
    "pnpm",
    ["exec", "vite", "--config", "measure-artifact/vite.config.ts", "--clearScreen", "false"],
    { shell: true },
  );
  const browser = browserPath();
  let chrome = null;
  let ws = null;

  const shutdown = () => {
    if (ws && ws.readyState === WebSocket.OPEN) ws.close();
    killTree(chrome);
    killTree(vite);
  };
  process.on("exit", shutdown);
  process.on("SIGINT", () => {
    shutdown();
    process.exit(130);
  });

  try {
    await waitForHttp(PAGE_URL, 30_000, (res) => res.ok);
    console.log("vite ready", PAGE_URL);

    chrome = spawnLogged(browser, [
      `--remote-debugging-port=${CDP_PORT}`,
      `--user-data-dir=${USER_DATA}`,
      "--headless=new",
      "--disable-gpu",
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-extensions",
      "--disable-background-timer-throttling",
      "--disable-renderer-backgrounding",
      "--disable-backgrounding-occluded-windows",
      "--force-device-scale-factor=1",
      "--js-flags=--expose-gc",
      `--window-size=1200,800`,
      PAGE_URL,
    ]);

    const target = await pageTarget();
    ws = await connect(target.webSocketDebuggerUrl);
    const cdp = makeCdp(ws);
    await cdp.send("Runtime.enable");
    await cdp.send("Page.enable");
    cdp.on("Runtime.consoleAPICalled", (params) => {
      const text = (params.args ?? [])
        .map((arg) => arg.value ?? arg.description ?? "")
        .join(" ");
      if (text) console.log("[page]", text);
    });
    const version = await cdp.send("Browser.getVersion");
    const machine = machineInfo(version);
    writeFileSync(MACHINE_PATH, `${JSON.stringify(machine, null, 2)}\n`);
    console.log("browser", version.product, version.userAgent);

    await cdp.send("Page.navigate", { url: PAGE_URL });

    const readyDeadline = Date.now() + 30_000;
    let ready = false;
    while (Date.now() < readyDeadline) {
      const result = await cdp.send("Runtime.evaluate", {
        expression: "Boolean(window.measureReady)",
        returnByValue: true,
      });
      if (result.result?.value === true) {
        ready = true;
        break;
      }
      await sleep(150);
    }
    if (!ready) throw new Error("harness did not set window.measureReady");

    const constants = await cdp.send("Runtime.evaluate", {
      expression: "window.measureConstants",
      returnByValue: true,
    });
    console.log("real-module constants", constants.result?.value);

    const results = {
      machine,
      constants: constants.result?.value ?? null,
      samples: [],
    };

    for (const entry of manifest) {
      console.log(`\n== ${entry.family} ${entry.kib} KiB (${entry.startTags} tags) ==`);
      const started = Date.now();
      const evaluated = await cdp.send(
        "Runtime.evaluate",
        {
          expression: `window.measureSample(${JSON.stringify(entry)})`,
          returnByValue: true,
          awaitPromise: true,
        },
        15 * 60 * 1000,
      );
      if (evaluated.exceptionDetails) {
        const detail =
          evaluated.exceptionDetails.exception?.description ??
          JSON.stringify(evaluated.exceptionDetails);
        console.error("EXCEPTION", detail);
        results.samples.push({ entry, error: detail });
      } else {
        results.samples.push(evaluated.result.value);
        const critic = evaluated.result.value?.isolated?.critic?.total;
        console.log(
          `done in ${((Date.now() - started) / 1000).toFixed(1)}s  critic median=${critic?.median?.toFixed?.(1) ?? "?"}ms  exceeds=${evaluated.result.value?.isolated?.critic?.exceedsProductTimeoutCount}`,
        );
      }
      writeFileSync(RESULTS_PATH, `${JSON.stringify(results, null, 2)}\n`);
    }

    console.log("wrote", RESULTS_PATH);
  } finally {
    shutdown();
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
