export type MarketplaceKind = "plugin" | "pack" | "skill";

export interface MarketplaceEntry {
  id: string;
  name: string;
  author: string;
  kind: MarketplaceKind;
  price: string;
  description: string;
}

export const MOCK_MARKETPLACE_ENTRIES: readonly MarketplaceEntry[] = [
  {
    id: "polis",
    name: "Polis",
    author: "@devboule",
    kind: "plugin",
    price: "free",
    description: "Explore a codebase as a navigable city beside your workspace.",
  },
  {
    id: "review-kit",
    name: "Review Kit",
    author: "@mira-dev",
    kind: "pack",
    price: "$8",
    description: "A compact set of review skills for risks, tests, and release notes.",
  },
  {
    id: "repo-rhythm",
    name: "Repo Rhythm",
    author: "@lena-code",
    kind: "skill",
    price: "free",
    description: "Turn a repository snapshot into a clear working plan.",
  },
  {
    id: "change-notes",
    name: "Change Notes",
    author: "@lena-code",
    kind: "skill",
    price: "free",
    description: "Draft concise release notes from the changes in a workspace.",
  },
];

export const FREE_SKILL = MOCK_MARKETPLACE_ENTRIES.find(
  (entry) => entry.id === "repo-rhythm",
) as MarketplaceEntry;
