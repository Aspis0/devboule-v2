/**
 * The desktop app as a test fixture: start it with an isolated runtime dir and
 * a fixed WebView2 debug port, and take down only what this run started.
 *
 * Windows-only, like the CI job that owns the smoke: the teardown is
 * `taskkill`, and the app's own discovery of the daemon beside its executable
 * is the path under test.
 */
import { spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, openSync, readFileSync } from "node:fs";
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
 * Refuse to start when the debug port is already held. A second client on a
 * contended port attaches to the first app's page and drives it, so the run
 * stops here instead of borrowing someone else's window — and nothing is ever
 * killed merely because it listens on the port.
 */
export function assertPortFree(port) {
  if (pidListeningOnPort(port) !== null) {
    throw new Error(
      `port ${port} is already in use (pid ${pidListeningOnPort(port)}); ` +
        `another Devboule or WebView2 app owns it — close it or set DEVBOULE_E2E_PORT`,
    );
  }
}

/** The PID listening on a port, or null. Used to refuse a contended port. */
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

/** Every process, one PowerShell call: pid, parent pid, image name, command line. */
function listProcesses() {
  const script =
    "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine | ConvertTo-Json -Compress";
  const { stdout } = spawnSync("powershell", ["-NoProfile", "-Command", script], {
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 32 * 1024 * 1024,
  });
  const raw = (stdout ?? "").trim();
  if (raw === "") return [];
  const parsed = JSON.parse(raw);
  const rows = Array.isArray(parsed) ? parsed : [parsed];
  return rows.map((row) => ({
    pid: row.ProcessId,
    parentPid: row.ParentProcessId,
    name: row.Name ?? "",
    commandLine: row.CommandLine ?? "",
  }));
}

/** The pids below one root, breadth first, from a process snapshot. */
function descendants(processes, rootPid) {
  const found = [];
  let frontier = [rootPid];
  while (frontier.length > 0) {
    const next = [];
    for (const process of processes) {
      if (frontier.includes(process.parentPid) && !found.includes(process.pid)) {
        found.push(process.pid);
        next.push(process.pid);
      }
    }
    frontier = next;
  }
  return found;
}

/**
 * The daemon this run owns, as the daemon itself recorded it: `pid=` in the
 * runtime dir's lock file. Falls back to the app's own child tree, which still
 * names an orphaned daemon because Windows keeps the parent pid after the
 * parent dies.
 */
export function ownedDaemonPid({ appPid, runtimeDir, processes = listProcesses() }) {
  try {
    const body = readFileSync(join(runtimeDir, "daemon.lock"), "utf8");
    const match = /^pid=(\d+)$/m.exec(body);
    if (match) return Number(match[1]);
  } catch {
    // No lock file yet: the daemon may never have started.
  }
  if (appPid === null) return null;
  const tree = descendants(processes, appPid);
  const daemon = processes.find(
    (process) => tree.includes(process.pid) && process.name.toLowerCase() === "devboule-daemon.exe",
  );
  return daemon?.pid ?? null;
}

/**
 * The WebView2 browser processes this run owns, proven by this run's unique
 * user-data folder in their command line — the browser is not a child of the
 * app, so ancestry alone cannot name it.
 */
export function ownedBrowserProcesses({ webviewDir, processes = listProcesses() }) {
  return processes.filter(
    (process) =>
      process.name.toLowerCase() === "msedgewebview2.exe" &&
      process.commandLine.includes(webviewDir),
  );
}

export function ownedBrowserPids({ webviewDir, processes = listProcesses() }) {
  return ownedBrowserProcesses({ webviewDir, processes }).map((process) => process.pid);
}

/**
 * Start the app and resolve once the process exists, so a failed spawn is an
 * error here rather than a missing PID later. Its output goes to two files in
 * `logDir`, truncated per run so a failure never shows the previous run's tail.
 */
export async function launchApp({
  binary,
  runtimeDir,
  localAppData,
  webviewDir,
  logDir,
  port = DEFAULT_DEBUG_PORT,
}) {
  const stdout = openSync(join(logDir, "app.stdout.log"), "w");
  const stderr = openSync(join(logDir, "app.stderr.log"), "w");
  const child = spawn(binary, [], {
    cwd: runtimeDir,
    env: {
      ...process.env,
      // An `e2e-cdp` build reads this and hands the port to WebView2 as the
      // window's own browser arguments. The environment variable WebView2 is
      // supposed to read instead was measured NOT reaching the browser on the
      // CI runner (0 of 6 processes carried the flag), while the explicit
      // argument does.
      DEVBOULE_E2E_CDP_PORT: String(port),
      // The browser's user data folder, outside the runtime dir: another
      // Devboule running on the machine must not share the browser process, and
      // this path is also what proves ownership at teardown.
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
 * reaches a daemon the app started; `/F` because a GUI process ignores a
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
 * The whole teardown, and only over processes this run started: the app pid it
 * spawned, the daemon pid the run's own runtime dir recorded, and the browser
 * pids whose command line carries the run's own user-data folder. A pid that
 * merely listens on our port is never a target — it belongs to someone else.
 *
 * Each wait gets a second pass: measured on this machine, the same app binary
 * ends in 1.3 s on one run and outlasts a 20 s wait on another (the loaded
 * machine, not the code), so a single deadline would report a leak that is not
 * one.
 */
export async function stopApp({ appPid, runtimeDir, webviewDir }) {
  const processes = listProcesses();
  const daemonPid = ownedDaemonPid({ appPid, runtimeDir, processes });
  const browserPids = ownedBrowserPids({ webviewDir, processes });
  if (appPid !== null) killTree(appPid);
  if (daemonPid !== null) killOne(daemonPid);
  for (const pid of browserPids) killTree(pid);

  const targets = [
    ["app", appPid],
    ["daemon", daemonPid],
    ...browserPids.map((pid) => ["WebView2", pid]),
  ].filter(([, pid]) => pid !== null);
  const stubborn = [];
  for (const [name, pid] of targets) {
    if (await waitForDeath(pid, 20_000)) continue;
    killTree(pid);
    // The second wait is longer than the first: a WebView2 process under load
    // was measured outliving a 20 s second wait, and this is the run's last
    // chance to leave the machine as it found it.
    if (!(await waitForDeath(pid, 30_000))) stubborn.push(`${name} ${pid}`);
  }
  return { gone: stubborn.length === 0, daemonPid, browserPids, stubborn };
}
