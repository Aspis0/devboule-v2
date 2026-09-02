import type { CityAgent } from "./model";
import { providerColor } from "./palette";

export interface RosterCounts {
  placed: number;
  roster: number;
}

export interface SessionFeedFailure {
  state: string;
  message: string;
}

export interface HostRosterReadout {
  fail(failure: SessionFeedFailure): void;
  render(agents: readonly CityAgent[], knownFileIds: ReadonlySet<string>): RosterCounts;
}

/** Keep a host-feed failure authoritative across later city-stat renders. */
export function createHostRosterReadout(container: HTMLElement): HostRosterReadout {
  let failure: SessionFeedFailure | null = null;
  return {
    fail(nextFailure) {
      failure = nextFailure;
      container.textContent = formatSessionFeedFailure(nextFailure);
    },
    render(agents, knownFileIds) {
      const counts = countRosterAgents(agents, knownFileIds);
      if (failure !== null) {
        container.textContent = formatSessionFeedFailure(failure);
        return counts;
      }
      renderAgentRoster(container, agents, knownFileIds);
      return counts;
    },
  };
}

/**
 * File-less host sessions are deliberately rendered as a DOM roster. Their
 * null fileId is a privacy and truth boundary: this function never invents a
 * building position for them.
 */
export function renderAgentRoster(
  container: HTMLElement,
  agents: readonly CityAgent[],
  knownFileIds: ReadonlySet<string>,
): RosterCounts {
  const { placed, roster: rosterCount } = countRosterAgents(agents, knownFileIds);
  const roster = agents.filter((agent) => agent.fileId === null || !knownFileIds.has(agent.fileId));
  container.replaceChildren();

  if (roster.length === 0) {
    container.textContent = "Roster: empty";
    return { placed, roster: 0 };
  }

  const heading = document.createElement("div");
  heading.textContent = `Roster: ${roster.length} session${roster.length === 1 ? "" : "s"} without a resolved file (not drawn)`;
  container.appendChild(heading);

  const list = document.createElement("ul");
  list.className = "polis-roster-list";
  for (const agent of roster) {
    const item = document.createElement("li");
    const title = document.createElement("span");
    title.textContent = agent.title ?? "Untitled session";
    const chip = document.createElement("span");
    chip.className = "polis-roster-provider";
    const color = providerColor(agent.provider);
    chip.dataset.color = color.toString(16);
    chip.textContent = displayProvider(agent.provider);
    chip.style.backgroundColor = `#${color.toString(16).padStart(6, "0")}`;
    chip.style.color = readableTextColor(color);
    chip.style.marginLeft = "0.4rem";
    chip.style.padding = "0.1rem 0.35rem";
    chip.style.borderRadius = "0.25rem";
    item.append(title, chip, document.createTextNode(` · ${agent.state}`));
    list.appendChild(item);
  }
  container.appendChild(list);
  return { placed, roster: rosterCount };
}

function countRosterAgents(
  agents: readonly CityAgent[],
  knownFileIds: ReadonlySet<string>,
): RosterCounts {
  const placed = agents.filter(
    (agent) => agent.fileId !== null && knownFileIds.has(agent.fileId),
  ).length;
  return { placed, roster: agents.length - placed };
}

function formatSessionFeedFailure(failure: SessionFeedFailure): string {
  return `Roster: live session feed ${failure.state} — ${failure.message}`;
}

function displayProvider(provider: string | null): string {
  if (provider === null) return "Unknown provider";
  if (provider.toLowerCase() === "opencode") return "OpenCode";
  return provider;
}

function readableTextColor(color: number): string {
  const red = (color >> 16) & 0xff;
  const green = (color >> 8) & 0xff;
  const blue = color & 0xff;
  return red * 0.299 + green * 0.587 + blue * 0.114 > 150 ? "#07101c" : "#ffffff";
}
