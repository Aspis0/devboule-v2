/**
 * The desktop app as a test fixture: start it with an isolated runtime dir and
 * a fixed WebView2 debug port, and take it and its daemon down by PID.
 *
 * Windows-only, like the CI job that owns the smoke: the teardown is
 * `taskkill`, and the app's own discovery of the daemon beside its executable
 * is the path under test.
 */
import { spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, openSync } from "node:fs";
import { join } from "node:path";

/** 9333 belongs to another WebView2 app on the development machine, and 9334
 * was handed to the UX-evidence agent after a shared-port incident. */
export const DEFAULT_DEBUG_PORT = 9336;

/** The two binaries `cargo build -p devboule-daemon -p devboule` leaves here. */
export function resolveBinaries(repoRoot) {
  const debug = join(repoRoot, "target", "debug");
  const app = join(debug, "devboule.exe");
  const daemon = join(debug, "devboule-daemon.exe");
  if (!existsSync(app)) throw new Error(`the app is not built: ${app}`);
  if (!existsSync(daemon)) throw new Error(`the daemon is not built beside it: ${daemon}`);
  return { app, daemon };
}

/**
 * Start the app and resolve once the process exists, so a failed spawn is an
 * error here rather than a missing PID later. Its output goes to two files in
 * `logDir`: the app's stderr is where its own setup failures are written.
 */
export async function launchApp({
  binary,
  runtimeDir,
  localAppData,
  webviewDir,
  logDir,
  port = DEFAULT_DEBUG_PORT,
}) {
  const stdout = openSync(join(logDir, "app.stdout.log"), "a");
  const stderr = openSync(join(logDir, "app.stderr.log"), "a");
  const child = spawn(binary, [], {
    cwd: runtimeDir,
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
      // The browser's user data folder, outside the runtime dir: another
      // Devboule running on the machine must not share the browser process, and
      // a stale browser on the debug port would answer a probe instead of ours.
      WEBVIEW2_USER_DATA_FOLDER: webviewDir,
      // Tauri's own data root (the asset scope for the daemon's staged
      // previews resolves under %LOCALAPPDATA%): the runtime dir sits inside
      // this run's redirected root, so the two agree.
      LOCALAPPDATA: localAppData,
      // The app hands this to the daemon it spawns, so both agree on the pipe
      // name, lock file and journal this run owns.
      DEVBOULE_RUNTIME_DIR: runtimeDir,
    },
    stdio: ["ignore", stdout, stderr],
    windowsHide: false,
  });
  try {
    await new Promise((resolve, reject) => {
      child.once("spawn", resolve);
      child.once("error", reject);
    });
  } finally {
    closeSync(stdout);
    closeSync(stderr);
  }
  return { pid: child.pid, child };
}

/**
 * Kill a process and everything it spawned. `/T` walks the tree, which is what
 * reaches the daemon the app started; `/F` because a GUI process ignores a
 * polite request.
 */
export function killTree(pid) {
  spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
}

/** Kill one process, nothing below it. */
export function killOne(pid) {
  spawnSync("taskkill", ["/PID", String(pid), "/F"], { stdio: "ignore", windowsHide: true });
}

export function isAlive(pid) {
  // CSV rows are `"image","pid","session","number","memory"`; the pid is the
  // second field, never the first.
  const { stdout } = spawnSync("tasklist", ["/FI", `PID eq ${pid}`, "/NH", "/FO", "CSV"], {
    encoding: "utf8",
    windowsHide: true,
  });
  return (stdout ?? "").includes(`,"${pid}",`);
}

/** Wait for a process to be gone, so the run does not end on a live orphan. */
export async function waitForDeath(pid, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (isAlive(pid) && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  return !isAlive(pid);
}

/**
 * The PID listening on our debug port. WebView2's browser process outlives the
 * app process that started it — measured here, `taskkill /T` on the app left it
 * listening — and the port is what names it without touching any other
 * Devboule's processes.
 */
export function pidListeningOnPort(port) {
  const { stdout } = spawnSync("netstat", ["-ano"], { encoding: "utf8", windowsHide: true });
  for (const line of (stdout ?? "").split(/\r?\n/)) {
    const fields = line.trim().split(/\s+/);
    if (fields.length >= 5 && fields[1].endsWith(`:${port}`) && fields[3] === "LISTENING") {
      return Number(fields[4]);
    }
  }
  return null;
}

/**
 * The whole teardown, by PID only: the app tree first (its daemon is inside
 * it), then the daemon the app named, then the WebView2 browser process this
 * run's debug port belongs to — it outlives the app, so it is not inside the
 * tree.
 *
 * Each wait gets a second pass: measured on this machine, the same app binary
 * ends in 1.3 s on one run and outlasts a 20 s wait on another (the loaded
 * machine, not the code), so a single deadline would report a leak that is not
 * one.
 */
export async function stopApp({ appPid, daemonPid, port }) {
  const browserPid = port === undefined ? null : pidListeningOnPort(port);
  if (appPid !== null) killTree(appPid);
  if (daemonPid !== null) killOne(daemonPid);
  if (browserPid !== null) killTree(browserPid);
  const targets = [
    ["app", appPid],
    ["daemon", daemonPid],
    ["WebView2", browserPid],
  ].filter(([, pid]) => pid !== null);
  const stubborn = [];
  for (const [name, pid] of targets) {
    if (await waitForDeath(pid, 20_000)) continue;
    killTree(pid);
    if (!(await waitForDeath(pid, 20_000))) stubborn.push(`${name} ${pid}`);
  }
  return { gone: stubborn.length === 0, browserPid, stubborn };
}
