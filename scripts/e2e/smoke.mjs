/**
 * End-to-end smoke of the real desktop app, run by .github/workflows/ci.yml on
 * the Windows runner.
 *
 * CI has no provider accounts, so no agent turn runs here: the script starts
 * devboule.exe with an isolated runtime dir and a WebView2 debug port, drives
 * the window over CDP, and asserts the shell and the daemon came up and the
 * rail took a project. Each check prints one PASS/FAIL line, and a screenshot
 * plus the app's own stdout/stderr land in the artifact directory whether the
 * run passed or failed. Only processes this run started are killed, and the
 * temp dirs it made are removed, unless --keep says otherwise.
 *
 *   node scripts/e2e/smoke.mjs --artifacts <dir> [--keep]
 *
 * DEVBOULE_E2E_PORT overrides the debug port (default 9336; 9333 and 9334
 * belong to other WebView2 apps on the development machine). DEVBOULE_E2E_LIVE_PROVIDER=pi|codex
 * adds one real agent turn; that is local-only and refused when CI is set.
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { CdpSession, captureScreenshot, findPageTarget } from "./cdp.mjs";
import { DEFAULT_DEBUG_PORT, assertPortFree, launchApp, resolveBinaries, stopApp } from "./app.mjs";
import {
  DAEMON_CONNECTED_MS,
  collectConsoleErrors,
  daemonConnected,
  noConsoleErrors,
  settingsOpensAndCloses,
  windowLoads,
  workspaceOnTempRepo,
} from "./checks.mjs";

const repoRoot = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
// Only the embedded build's own origin: a match list that also accepted the
// vite dev server would attach to whatever other Devboule holds the port.
const TARGET_URL_MATCHES = ["tauri.localhost"];
const ATTACH_MS = 120_000;

const CHECK = {
  window: "the window loads and the Workspace surface renders",
  daemon: "the status bar's daemon dot reaches connected",
  settings: "Settings opens and closes",
  rail: "a workspace on a temp git repo shows in the rail",
  console: "no uncaught console errors during the run",
  screenshot: "a screenshot of the window is saved as an artifact",
  teardown: "the app, its daemon and its browser are gone at the end",
};

function parseArgs(argv) {
  let artifacts = join(tmpdir(), "devboule-e2e-artifacts");
  let keep = false;
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--keep") {
      keep = true;
      continue;
    }
    if (argv[index] !== "--artifacts") throw new Error(`unknown argument: ${argv[index]}`);
    if (!argv[index + 1]) throw new Error("--artifacts needs a directory");
    artifacts = argv[index + 1];
    index += 1;
  }
  return { artifacts, keep };
}

const results = [];
function record(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail === "" ? "" : ` — ${detail}`}`);
}

async function check(name, work) {
  try {
    record(name, true, (await work()) ?? "");
  } catch (error) {
    record(name, false, String(error?.message ?? error));
  }
}

/** A git repository with one commit: what a project needs to be real. */
function createTempGitRepo(dir) {
  const git = (...args) => {
    const result = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8", windowsHide: true });
    if (result.status !== 0)
      throw new Error(`git ${args.join(" ")} failed: ${result.stderr.trim()}`);
  };
  mkdirSync(dir, { recursive: true });
  git("init", "-b", "main");
  git("config", "user.email", "smoke@example.invalid");
  git("config", "user.name", "Devboule smoke");
  writeFileSync(join(dir, "hello.txt"), "before\n");
  git("add", "hello.txt");
  git("commit", "-m", "seed the smoke repository");
}

const { artifacts, keep } = parseArgs(process.argv.slice(2));
if (process.platform !== "win32") {
  console.error("the smoke fixture is Windows-only; the macOS slice is not written yet");
  process.exit(2);
}
const port = Number(process.env.DEVBOULE_E2E_PORT ?? DEFAULT_DEBUG_PORT);
const liveProvider = (process.env.DEVBOULE_E2E_LIVE_PROVIDER ?? "").trim();
if (liveProvider !== "" && liveProvider !== "pi" && liveProvider !== "codex") {
  console.error(`DEVBOULE_E2E_LIVE_PROVIDER must be pi or codex, not "${liveProvider}"`);
  process.exit(2);
}
if (liveProvider !== "" && process.env.CI) {
  console.error("the live agent turn is local-only; it is refused when CI is set");
  process.exit(2);
}
try {
  assertPortFree(port);
} catch (error) {
  console.error(`ABORT: ${error.message}`);
  process.exit(2);
}

mkdirSync(artifacts, { recursive: true });
// A previous run's picture must not stand in for this one's.
rmSync(join(artifacts, "smoke.png"), { force: true });
const liveCheck = `one real ${liveProvider} agent turn through the app`;
const workRoot = mkdtempSync(join(tmpdir(), "devboule-e2e-"));
const localAppData = join(workRoot, "localappdata");
const runtimeDir = join(localAppData, "Devboule");
const webviewDir = join(workRoot, "webview2");
const repoDir = join(workRoot, "repo");
mkdirSync(runtimeDir, { recursive: true });
createTempGitRepo(repoDir);

const state = { appPid: null, session: null, cleaned: false, stopped: null };
const launchedAt = Date.now();
let fatal = null;

/** The window as a PNG, from the live session or from a fresh one. Never throws. */
async function captureBestEffort() {
  try {
    if (state.session !== null) {
      try {
        return await captureScreenshot(state.session);
      } catch {
        // The session may have died with the page; a fresh attach can still see it.
      }
    }
    const target = await findPageTarget({
      port,
      urlMatches: TARGET_URL_MATCHES,
      timeoutMs: 10_000,
    });
    if (target === null) return null;
    const fresh = await CdpSession.open(target.webSocketDebuggerUrl, 10_000);
    try {
      return await captureScreenshot(fresh);
    } finally {
      fresh.close();
    }
  } catch {
    return null;
  }
}

/**
 * The one teardown, run from the ordinary road and from a signal: screenshot
 * first (the window is still alive), then only this run's PIDs, then the temp
 * dirs. Idempotent, so a Ctrl+C during the finally cannot double-kill.
 */
async function shutdown() {
  if (state.cleaned) return state.stopped;
  state.cleaned = true;
  try {
    const png = await captureBestEffort();
    if (png === null) {
      record(CHECK.screenshot, false, "the window did not answer the screenshot");
    } else {
      const path = join(artifacts, "smoke.png");
      writeFileSync(path, png);
      record(CHECK.screenshot, true, `wrote ${png.length} bytes to ${path}`);
    }
  } catch (error) {
    record(CHECK.screenshot, false, String(error?.message ?? error));
  }
  try {
    state.stopped = await stopApp({ appPid: state.appPid, runtimeDir, webviewDir });
    record(
      CHECK.teardown,
      state.stopped.gone,
      `app ${state.appPid ?? "none"}, daemon ${state.stopped.daemonPid ?? "none"}, ` +
        `WebView2 ${state.stopped.browserPids.join(", ") || "none"}` +
        (state.stopped.stubborn.length === 0
          ? ""
          : ` — still alive: ${state.stopped.stubborn.join(", ")}`),
    );
  } catch (error) {
    record(CHECK.teardown, false, String(error?.message ?? error));
  }
  // The CDP socket is a live handle: left open, it holds the process after the
  // last check and a CI step waits out its whole timeout on a finished run.
  state.session?.close();
  if (!keep) rmSync(workRoot, { recursive: true, force: true });
  return state.stopped;
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    console.log(`received ${signal} — cleaning up this run's processes`);
    void shutdown().finally(() => process.exit(130));
  });
}

try {
  const binaries = resolveBinaries(repoRoot);
  const app = await launchApp({
    binary: binaries.app,
    runtimeDir,
    localAppData,
    webviewDir,
    logDir: artifacts,
    port,
  });
  state.appPid = app.pid;

  const target = await findPageTarget({
    port,
    urlMatches: TARGET_URL_MATCHES,
    timeoutMs: ATTACH_MS,
  });
  if (target === null) throw new Error(`no WebView page answered on port ${port}`);
  state.session = await CdpSession.open(target.webSocketDebuggerUrl, 15_000);
  const consoleErrors = collectConsoleErrors(state.session);
  await state.session.send("Runtime.enable");
  // Not swallowed: if the browser's own log stream cannot be enabled, the
  // console check would pass without watching anything, so the run fails here.
  await state.session.send("Log.enable");
  await state.session.send("Page.enable");
  // The instrumentation has to exist before the app's scripts run, and the page
  // is already loaded by the time a client can attach — so the page is reloaded
  // under the listeners and the checks run against that load.
  const loaded = state.session.once("Page.loadEventFired", 60_000);
  await state.session.send("Page.reload", { ignoreCache: false });
  await loaded;

  await check(CHECK.window, () => windowLoads(state.session));
  await check(CHECK.daemon, () => daemonConnected(state.session, launchedAt + DAEMON_CONNECTED_MS));
  await check(CHECK.settings, () => settingsOpensAndCloses(state.session));
  await check(CHECK.rail, () =>
    workspaceOnTempRepo(state.session, { repoDir, repoName: basename(repoDir) }),
  );
  if (liveProvider !== "") {
    const { runLiveTurn } = await import("./liveTurn.mjs");
    await check(liveCheck, () =>
      runLiveTurn({ session: state.session, provider: liveProvider, repoDir }),
    );
  }
  await check(CHECK.console, () => noConsoleErrors(consoleErrors));
} catch (error) {
  fatal = error;
} finally {
  await shutdown();
}

if (fatal !== null) {
  const reason = String(fatal?.message ?? fatal);
  const attempted = new Set(results.map((result) => result.name));
  const expected = [CHECK.window, CHECK.daemon, CHECK.settings, CHECK.rail];
  if (liveProvider !== "") expected.push(liveCheck);
  expected.push(CHECK.console);
  for (const name of expected) {
    if (!attempted.has(name)) record(name, false, `not run: ${reason}`);
  }
}

const failed = results.filter((result) => !result.ok).length;
console.log(`${results.length - failed}/${results.length} checks passed`);
// Forced, not `process.exitCode`: a handle nothing here knows about must not
// turn a finished run into a hanging step.
process.exit(failed === 0 ? 0 : 1);
