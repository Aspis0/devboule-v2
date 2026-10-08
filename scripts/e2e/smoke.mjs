/**
 * End-to-end smoke of the real desktop app, run by .github/workflows/ci.yml on
 * the Windows runner.
 *
 * CI has no provider accounts, so no agent turn runs here: the script starts
 * devboule.exe with an isolated runtime dir and a WebView2 debug port, drives
 * the window over CDP, and asserts the shell and the daemon came up and the
 * rail took a project. Each check prints one PASS/FAIL line, and a screenshot
 * plus the app's own stdout/stderr land in the artifact directory whether the
 * run passed or failed. The app and its daemon are killed by PID on every road
 * out, including the failing one.
 *
 *   node scripts/e2e/smoke.mjs --artifacts <dir>
 *
 * DEVBOULE_E2E_PORT overrides the debug port (default 9336; 9333 and 9334
 * belong to other WebView2 apps on the development machine). DEVBOULE_E2E_LIVE_PROVIDER=pi|codex
 * adds one real agent turn and is local-only.
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { CdpSession, captureScreenshot, findPageTarget } from "./cdp.mjs";
import { DEFAULT_DEBUG_PORT, launchApp, resolveBinaries, stopApp } from "./app.mjs";
import {
  collectConsoleErrors,
  daemonConnected,
  noConsoleErrors,
  readDaemonPid,
  settingsOpensAndCloses,
  windowLoads,
  workspaceOnTempRepo,
} from "./checks.mjs";

const repoRoot = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
// Only the embedded build's own origin: a match list that also accepted the
// vite dev server would attach to whatever other Devboule holds the port.
const TARGET_URL_MATCHES = ["tauri.localhost"];

const CHECK = {
  window: "the window loads and the Workspace surface renders",
  daemon: "the status bar's daemon dot reaches connected",
  settings: "Settings opens and closes",
  rail: "a workspace on a temp git repo shows in the rail",
  console: "no uncaught console errors during the run",
  screenshot: "a screenshot of the window is saved as an artifact",
  teardown: "the app and its daemon are gone at the end",
};

function parseArgs(argv) {
  let artifacts = join(tmpdir(), "devboule-e2e-artifacts");
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] !== "--artifacts") throw new Error(`unknown argument: ${argv[index]}`);
    if (!argv[index + 1]) throw new Error("--artifacts needs a directory");
    artifacts = argv[index + 1];
    index += 1;
  }
  return { artifacts };
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

/** The window as a PNG, from the live session or from a fresh one. */
async function captureBestEffort(port, session) {
  if (session !== null) {
    try {
      return await captureScreenshot(session);
    } catch {
      // The session may have died with the page; a fresh attach can still see it.
    }
  }
  const target = await findPageTarget({ port, urlMatches: TARGET_URL_MATCHES, timeoutMs: 10_000 });
  if (target === null) return null;
  const fresh = await CdpSession.open(target.webSocketDebuggerUrl);
  try {
    return await captureScreenshot(fresh);
  } catch {
    return null;
  } finally {
    fresh.close();
  }
}

const { artifacts } = parseArgs(process.argv.slice(2));
mkdirSync(artifacts, { recursive: true });
if (process.platform !== "win32") {
  console.error("the smoke fixture is Windows-only; the macOS slice is not written yet");
  process.exit(2);
}

const port = Number(process.env.DEVBOULE_E2E_PORT ?? DEFAULT_DEBUG_PORT);
const liveProvider = (process.env.DEVBOULE_E2E_LIVE_PROVIDER ?? "").trim();
if (liveProvider === "claude") {
  console.error("the live turn must not run Claude; set DEVBOULE_E2E_LIVE_PROVIDER to pi or codex");
  process.exit(2);
}
const liveCheck = `one real ${liveProvider} agent turn through the app`;
const workRoot = mkdtempSync(join(tmpdir(), "devboule-e2e-"));
const localAppData = join(workRoot, "localappdata");
const runtimeDir = join(localAppData, "Devboule");
const webviewDir = join(workRoot, "webview2");
const repoDir = join(workRoot, "repo");
mkdirSync(runtimeDir, { recursive: true });
createTempGitRepo(repoDir);

let appPid = null;
let daemonPid = null;
let session = null;
let fatal = null;
const launchedAt = Date.now();

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
  appPid = app.pid;

  const target = await findPageTarget({ port, urlMatches: TARGET_URL_MATCHES, timeoutMs: 120_000 });
  if (target === null) throw new Error(`no WebView page answered on port ${port}`);
  session = await CdpSession.open(target.webSocketDebuggerUrl);
  const consoleErrors = collectConsoleErrors(session);
  await session.send("Runtime.enable");
  // The browser's own log stream is the second source; where it refuses to
  // enable, the exception stream still carries every uncaught failure.
  await session.send("Log.enable").catch(() => {});

  await check(CHECK.window, () => windowLoads(session));
  await check(CHECK.daemon, () => daemonConnected(session, launchedAt));
  daemonPid = await readDaemonPid(session).catch(() => null);
  await check(CHECK.settings, () => settingsOpensAndCloses(session));
  await check(CHECK.rail, () =>
    workspaceOnTempRepo(session, { repoDir, repoName: basename(repoDir) }),
  );
  if (liveProvider !== "") {
    const { runLiveTurn } = await import("./liveTurn.mjs");
    await check(liveCheck, () => runLiveTurn({ session, provider: liveProvider, repoDir }));
  }
  await check(CHECK.console, () => noConsoleErrors(consoleErrors));
} catch (error) {
  fatal = error;
} finally {
  const png = await captureBestEffort(port, session);
  if (png === null) {
    record(CHECK.screenshot, false, "the window did not answer the screenshot");
  } else {
    const path = join(artifacts, "smoke.png");
    writeFileSync(path, png);
    record(CHECK.screenshot, true, `wrote ${png.length} bytes to ${path}`);
  }
  const stopped = await stopApp({ appPid, daemonPid, port });
  record(
    CHECK.teardown,
    stopped.gone,
    `app ${appPid ?? "none"}, daemon ${daemonPid ?? "none"}, WebView2 ${stopped.browserPid ?? "none"}` +
      (stopped.stubborn.length === 0 ? "" : ` — still alive: ${stopped.stubborn.join(", ")}`),
  );
  // The CDP socket is a live handle: left open, it holds the process after the
  // last check and a CI step waits out its whole timeout on a finished run.
  session?.close();
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
