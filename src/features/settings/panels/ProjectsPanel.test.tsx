// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return {
    ...actual,
    isCommandError: vi.fn(
      (error: unknown) =>
        typeof error === "object" && error !== null && "code" in error && "message" in error,
    ),
    daemonStatus: vi.fn(async () => ({
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities: [
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "tool_policy",
      ],
      message: null,
    })),
    journalRetentionGet: vi.fn(),
    journalRetentionSet: vi.fn(),
    journalUsage: vi.fn(),
    // The General panel's close-behavior and notification-sound rows read
    // and write their own surface settings.
    surfaceSettingsGet: vi.fn(async () => ({ status: "absent" })),
    surfaceSettingsSet: vi.fn(async () => undefined),
    projectAdd: vi.fn(),
    projectsList: vi.fn(async () => []),
    providersList: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providersRefresh: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providerUpdate: vi.fn(async () => ({ ok: true, exitCode: 0, log: "" })),
    toolPolicyGet: vi.fn(async () => ({ policies: [] })),
    toolPolicySet: vi.fn(async () => undefined),
    agentProfilesGet: vi.fn(async () => ({
      document: {
        profiles: [],
        standingInstructions: "",
      } as AgentProfilesDocument,
    })),
    agentProfilesSet: vi.fn(async () => undefined),
    // No default answer: a vocabulary query only ever leaves the app when the
    // handshake advertised `provider_vocabulary`, and the tests that arm it
    // queue their own replies.
    providerVocabularyGet: vi.fn(),
    // The delegation pair: never called unless the handshake advertised
    // `permission_delegation`, and every test arms its own replies.
    delegationGet: vi.fn(),
    delegationSet: vi.fn(async () => undefined),
    workspacesList: vi.fn(async () => []),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import { projectAdd, projectsList, workspacesList } from "../../../lib/tauri";
import type { AgentProfilesDocument, Project } from "../../../types/ipc";
import { ProjectsPanel } from "./ProjectsPanel";
import { open } from "@tauri-apps/plugin-dialog";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
describe("Settings projects", () => {
  let container: HTMLDivElement;
  let root: Root;
  const project: Project = {
    id: "project-settings",
    name: "real-project",
    path: "D:\\real-project",
  };

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(projectsList).mockResolvedValue([project]);
    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-settings",
        projectId: project.id,
        title: "feature-x",
        isolation: "worktree",
        path: "D:\\real-project.worktrees\\feature-x-9f2e1a",
      },
    ]);
    vi.mocked(open).mockResolvedValue(null);
  });

  afterEach(() => {
    root.unmount();
    container.remove();
    vi.clearAllMocks();
  });

  async function renderProjects() {
    root = createRoot(container);
    await act(async () => root.render(<ProjectsPanel />));
    await act(async () => undefined);
  }

  it("lists daemon projects and workspace counts", async () => {
    await renderProjects();

    expect(projectsList).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("real-project");
    expect(container.textContent).toContain("D:\\real-project");
    expect(container.textContent).toContain("1 workspace");
    // The checkout path is rendered exactly as the daemon sent it. It differs
    // from the project path here, so a frontend that substituted the project
    // path would fail this assertion.
    expect(container.textContent).toContain("D:\\real-project.worktrees\\feature-x-9f2e1a");
  });

  it("does not repeat a workspace path that equals its project's", async () => {
    // A local workspace's path IS the project's path by construction, so the
    // card would print the same line once per workspace.
    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-local",
        projectId: project.id,
        title: "real-project",
        isolation: "local",
        path: "D:\\real-project",
      },
      {
        id: "workspace-worktree",
        projectId: project.id,
        title: "feature-y",
        isolation: "worktree",
        path: "D:\\real-project.worktrees\\feature-y-1a2b3c",
      },
    ]);
    await renderProjects();

    expect(container.textContent).toContain("D:\\real-project.worktrees\\feature-y-1a2b3c");
    // The project's own line: exactly one, never repeated per workspace.
    const projectPaths = [
      ...container.querySelectorAll(
        "[data-settings-project] .settings-row-description, [data-settings-project] .settings-card-meta",
      ),
    ].filter((meta) => meta.textContent === "D:\\real-project");
    expect(projectPaths).toHaveLength(1);
  });

  it("names two workspaces on the project path as two rows, and prints the path once", async () => {
    // Two real local records on the project folder: both carry the project
    // path, so identifying them means their titles, never the path again.
    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-local-one",
        projectId: project.id,
        title: "real-project",
        isolation: "local",
        path: "D:\\real-project",
      },
      {
        id: "workspace-local-two",
        projectId: project.id,
        title: "real-project",
        isolation: "local",
        path: "D:\\real-project",
      },
    ]);
    await renderProjects();

    const lines = [
      ...container.querySelectorAll(
        "[data-settings-project] .settings-row-description, [data-settings-project] .settings-card-meta",
      ),
    ].map((meta) => meta.textContent);
    // The project's own line, then each row named by its title: the shared
    // folder path prints once, never once per record.
    expect(lines).toEqual(["D:\\real-project", "real-project", "real-project 2"]);
    expect(container.querySelector(".settings-row-title")?.textContent).toBe("real-project");
    expect(container.textContent).toContain("2 workspaces");
  });

  it("keeps other projects visible when one workspace list fails and retries", async () => {
    const brokenProject: Project = {
      id: "project-settings-broken",
      name: "broken-project",
      path: "D:\\broken-project",
    };
    vi.mocked(projectsList).mockResolvedValue([project, brokenProject]);
    vi.mocked(workspacesList).mockImplementation(async (projectId) => {
      if (projectId === brokenProject.id) throw new Error("settings workspace list failed");
      return [
        {
          id: "workspace-settings",
          projectId,
          title: "main",
          isolation: "local",
          path: projectId === project.id ? "D:\\real-project" : "D:\\broken-project",
        },
      ];
    });
    await renderProjects();

    expect(container.textContent).toContain("real-project");
    expect(container.textContent).toContain("broken-project");
    expect(container.textContent).toContain("settings workspace list failed");
    expect(container.textContent).not.toContain("No projects registered");

    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-settings-broken",
        projectId: brokenProject.id,
        title: "fixed",
        isolation: "local",
        path: "D:\\broken-project",
      },
    ]);
    const retry = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Retry",
    );
    if (retry === undefined) throw new Error("settings project retry control did not render");
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(container.textContent).not.toContain("settings workspace list failed");
    expect(container.textContent).toContain("1 workspace");
  });

  it("shows a daemon project-list failure instead of pretending there are no projects", async () => {
    vi.mocked(projectsList).mockRejectedValueOnce(new Error("project journal unavailable"));
    await renderProjects();

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "project journal unavailable",
    );
    expect(container.textContent).not.toContain("No projects registered");
  });

  it("uses the native folder picker and the daemon-returned project row", async () => {
    const added: Project = {
      id: "canonical-project",
      name: "canonical-name",
      path: "D:\\canonical-project",
    };
    vi.mocked(open).mockResolvedValueOnce("D:\\typed-or-picked");
    vi.mocked(projectAdd).mockResolvedValueOnce(added);
    await renderProjects();

    const add = container.querySelector<HTMLButtonElement>('button[aria-label="Add project"]');
    if (!add) throw new Error("Add project control did not render");
    await act(async () => add.click());
    const choose = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Choose folder…",
    );
    if (!choose) throw new Error("Choose folder control did not render");
    await act(async () => choose.click());
    await act(async () => undefined);

    expect(open).toHaveBeenCalledWith({ directory: true });
    const submit = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent === "Add project",
    );
    if (!submit) throw new Error("Add project submit control did not render");
    await act(async () => submit.click());
    await act(async () => undefined);

    expect(projectAdd).toHaveBeenCalledWith("D:\\typed-or-picked");
    expect(container.textContent).toContain("canonical-name");
    expect(container.textContent).toContain("D:\\canonical-project");
  });

  it("lists each project as one row under the Projects label", async () => {
    await renderProjects();

    const section = container.querySelector("[data-settings-section]");
    if (section === null) throw new Error("projects section did not render");
    expect(section.querySelector(".settings-section-label")?.textContent).toBe("Projects");
    expect(section.querySelectorAll("[data-settings-project]")).toHaveLength(1);
    expect(section.textContent).toContain("real-project");
  });

  it("puts the Add action in the section head as a labelled glyph", async () => {
    await renderProjects();

    const add = container.querySelector(".settings-section-action button");
    expect(add?.getAttribute("aria-label")).toBe("Add project");
    expect(add?.textContent).toBe("+");
  });
});
