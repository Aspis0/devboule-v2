// @vitest-environment happy-dom

// Which read each refusal retries: the projects block re-reads projects, the
// provider block re-reads the provider catalog. Each Retry sits beside the
// sentence it can clear, so clicking one must ask for that read and no other.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const REFUSAL: ErrorSentence = { sentence: "The daemon did not answer.", detail: null };

function treeProps(over: Partial<WorkspaceTreeProps> = {}): WorkspaceTreeProps {
  return {
    projects: [],
    loading: false,
    error: null,
    providerError: null,
    selectedWorkspace: null,
    onRetryProjects: vi.fn(),
    onRetryProviders: vi.fn(),
    onSelectWorkspace: vi.fn(),
    onNewWorkspace: vi.fn(),
    onRenameWorkspace: vi.fn(async () => null),
    onDeleteWorkspace: vi.fn(async () => null),
    providerMenuAnchorProjectId: null,
    providerMenu: null,
    stats: new Map(),
    branches: new Map(),
    hostNames: new Map(),
    ...over,
  };
}

describe("what each sidebar refusal retries", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function clickRetry(): Promise<void> {
    const retry = container.querySelector<HTMLButtonElement>(".workspace-secondary-action");
    if (retry === null) throw new Error("the refusal drew no retry");
    await act(async () => retry.click());
  }

  it("re-reads the provider catalog behind the provider refusal", async () => {
    const onRetryProjects = vi.fn();
    const onRetryProviders = vi.fn();
    await act(async () => {
      root.render(
        <WorkspaceTree
          {...treeProps({
            providerError: REFUSAL,
            onRetryProjects,
            onRetryProviders,
          })}
        />,
      );
    });

    await clickRetry();

    expect(onRetryProviders).toHaveBeenCalledTimes(1);
    expect(onRetryProjects).not.toHaveBeenCalled();
  });

  it("re-reads projects behind the projects refusal", async () => {
    const onRetryProjects = vi.fn();
    const onRetryProviders = vi.fn();
    await act(async () => {
      root.render(
        <WorkspaceTree {...treeProps({ error: REFUSAL, onRetryProjects, onRetryProviders })} />,
      );
    });

    await clickRetry();

    expect(onRetryProjects).toHaveBeenCalledTimes(1);
    expect(onRetryProviders).not.toHaveBeenCalled();
  });
});
