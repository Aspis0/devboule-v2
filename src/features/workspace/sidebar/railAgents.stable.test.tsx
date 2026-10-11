// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { useRailAgents, type RailAgents } from "./railAgents";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const KEY = localWorkspaceKey("workspace-1") as WorkspaceKey;

const agent = (over: Partial<Session> = {}): Session => ({
  id: "session",
  workspaceId: "workspace-1",
  kind: "claude",
  title: "agent",
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
  ...over,
});

function Probe({ sessions, seen }: { sessions: Session[]; seen: RailAgents[] }) {
  seen.push(useRailAgents(sessions, KEY, null));
  return null;
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

describe("the rail's agent rows across roster pushes", () => {
  it("keeps the same object while the listed agents are unchanged", async () => {
    const seen: RailAgents[] = [];
    await act(async () => root.render(<Probe sessions={[agent({ id: "a" })]} seen={seen} />));
    const first = seen.at(-1);

    await act(async () => root.render(<Probe sessions={[agent({ id: "a" })]} seen={seen} />));

    expect(seen.at(-1)).toBe(first);
  });

  it("gives a new object when an agent's row changes", async () => {
    const seen: RailAgents[] = [];
    await act(async () => root.render(<Probe sessions={[agent({ id: "a" })]} seen={seen} />));
    const first = seen.at(-1);

    await act(async () =>
      root.render(<Probe sessions={[agent({ id: "a", activity: "working" })]} seen={seen} />),
    );

    expect(seen.at(-1)).not.toBe(first);
    expect(seen.at(-1)?.rows[0]?.state).toBe("running");
  });
});
