/**
 * The window checks the smoke asserts: the surface renders, the daemon connects,
 * Settings opens and closes, a project's workspace reaches the rail, and the
 * console stayed clean. Each throws the sentence the report prints on failure.
 */
import { evaluate } from "./cdp.mjs";
import { click, requireSelector, selectSurface, typeInto, waitFor } from "./drive.mjs";

export const WINDOW_READY_MS = 90_000;
/** The instruction's own budget, counted from the app's launch. */
export const DAEMON_CONNECTED_MS = 60_000;
const SURFACE_MS = 20_000;
const DIALOG_MS = 15_000;
const RAIL_MS = 20_000;

/**
 * Collect what the page reports as an uncaught failure: exceptions the runtime
 * threw, and everything the browser logged at error level — a refused module
 * load or a broken promise a check would otherwise not see.
 */
export function collectConsoleErrors(session) {
  const errors = [];
  session.on("Runtime.exceptionThrown", ({ exceptionDetails }) => {
    errors.push(
      exceptionDetails?.exception?.description ?? exceptionDetails?.text ?? "uncaught exception",
    );
  });
  session.on("Log.entryAdded", ({ entry }) => {
    // The browser asks every page for its icon; the app ships none, and a
    // missing icon is not an app failure.
    if (entry.level === "error" && !String(entry.url ?? "").endsWith("/favicon.ico")) {
      errors.push(`${entry.source}: ${entry.text}${entry.url ? ` (${entry.url})` : ""}`);
    }
  });
  return errors;
}

/** The window's document is loaded and the Workspace surface is on screen. */
export async function windowLoads(session) {
  const ready = await waitFor(
    session,
    `document.readyState === "complete" && document.querySelector("section.workspace-screen") !== null`,
    WINDOW_READY_MS,
  );
  if (!ready) throw new Error("the Workspace surface did not render");
}

/** The status bar's dot reaches the connected tone inside the launch budget. */
export async function daemonConnected(session, launchedAt) {
  // The budget counts from the launch, but the dot cannot be read before the
  // window renders; when the render already spent it, the state is what is
  // checked, not the clock.
  const remaining = Math.max(10_000, DAEMON_CONNECTED_MS - (Date.now() - launchedAt));
  const connected = await waitFor(
    session,
    `document.querySelector(".workspace-status-bar .workspace-status-dot.workspace-dot-green") !== null`,
    remaining,
  );
  if (!connected) throw new Error("the daemon dot did not reach the connected tone");
}

/** The daemon's PID, as the app's own status command answers it. */
export async function readDaemonPid(session) {
  const status = await evaluate(session, `window.__TAURI_INTERNALS__.invoke("daemon_status")`);
  return typeof status?.pid === "number" ? status.pid : null;
}

export async function settingsOpensAndCloses(session) {
  await selectSurface(session, "settings", "section.settings-surface");
  await click(session, ".settings-back-row");
  await requireSelector(
    session,
    "section.workspace-screen",
    SURFACE_MS,
    "Settings did not close back to the Workspace surface",
  );
}

/**
 * Add the temp repository as a project through the Add project dialog, then
 * create its workspace through the app's own command boundary — the road the
 * project's "+" takes once a provider is chosen, and CI has no provider to
 * choose. The rail must show the row after it reads the daemon again.
 */
export async function workspaceOnTempRepo(session, { repoDir, repoName }) {
  await click(session, '[aria-label="New project"]');
  await requireSelector(
    session,
    "#workspace-project-input",
    DIALOG_MS,
    "the Add project dialog did not open",
  );
  await typeInto(session, "#workspace-project-input", repoDir);
  await click(session, '.workspace-project-dialog-actions button[type="submit"]');

  const projectListed = `[...document.querySelectorAll(".workspace-project-name")].some((element) => element.textContent === ${JSON.stringify(repoName)})`;
  if (!(await waitFor(session, projectListed, RAIL_MS))) {
    throw new Error(`the project ${repoName} did not appear in the rail`);
  }

  const created = await evaluate(
    session,
    `(async () => {
      const projects = await window.__TAURI_INTERNALS__.invoke("projects_list");
      const project = projects.find((row) => row.name === ${JSON.stringify(repoName)});
      if (project === undefined) throw new Error("the daemon did not list the project it just accepted");
      const workspace = await window.__TAURI_INTERNALS__.invoke("workspace_create", {
        projectId: project.id,
        isolation: "local",
        branch: null,
      });
      return workspace.id;
    })()`,
  );

  // The surface remount is what makes the rail read the daemon again; the
  // row can only come from the daemon's own list.
  await selectSurface(session, "settings", "section.settings-surface");
  await selectSurface(session, "workspace", "section.workspace-screen");
  const rowListed = `[...document.querySelectorAll(".workspace-project")].some((group) => group.querySelector(".workspace-project-name")?.textContent === ${JSON.stringify(repoName)} && group.querySelectorAll("button.workspace-row").length > 0)`;
  if (!(await waitFor(session, rowListed, RAIL_MS))) {
    throw new Error("the workspace row did not appear under the project");
  }
  return `workspace ${created}`;
}

export function noConsoleErrors(errors) {
  const unique = [...new Set(errors)];
  if (unique.length > 0) {
    throw new Error(`${unique.length} console error(s): ${unique.slice(0, 3).join(" | ")}`);
  }
}
