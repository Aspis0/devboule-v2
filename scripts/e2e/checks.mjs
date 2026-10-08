/**
 * The window checks the smoke asserts: the surface renders, the daemon connects
 * inside its budget, Settings opens and closes, a project's workspace reaches
 * the rail, and the console stayed clean. Each throws the sentence the report
 * prints on failure.
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
 * threw, console API calls at error level, and everything the browser logged at
 * error level — a refused module load a check would otherwise not see.
 */
export function collectConsoleErrors(session) {
  const errors = [];
  session.on("Runtime.exceptionThrown", ({ exceptionDetails }) => {
    errors.push(
      exceptionDetails?.exception?.description ?? exceptionDetails?.text ?? "uncaught exception",
    );
  });
  session.on("Runtime.consoleAPICalled", ({ type, args }) => {
    if (type !== "error") return;
    const text = args
      .map((arg) => arg.value ?? arg.description ?? arg.type ?? "")
      .filter((part) => part !== "")
      .join(" ");
    errors.push(`console.error: ${text}`);
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

/**
 * The status bar's dot reaches the connected tone before the launch budget
 * expires. The deadline is absolute, not "however long the window took plus a
 * grace period": a page that only becomes observable after the budget is a
 * failed timing assertion, not a late pass.
 */
export async function daemonConnected(session, deadline) {
  const remaining = deadline - Date.now();
  if (remaining <= 0) {
    throw new Error("the window was not observable before the 60s daemon budget ran out");
  }
  const connected = await waitFor(
    session,
    `document.querySelector(".workspace-status-bar .workspace-status-dot.workspace-dot-green") !== null`,
    remaining,
  );
  if (!connected) throw new Error("the daemon dot did not reach the connected tone");
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
  if (
    !(await waitFor(
      session,
      `document.querySelector("#workspace-project-input") === null`,
      DIALOG_MS,
    ))
  ) {
    throw new Error("the Add project dialog stayed open after Add project was clicked");
  }

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
