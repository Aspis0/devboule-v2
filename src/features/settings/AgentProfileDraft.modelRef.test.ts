// The composite model identity, without a render: keys, bare ids, exact
// matching, and the ambiguous-save refusal.
import { describe, expect, it } from "vitest";
import type { SessionModel } from "../../types/ipc";
import {
  bareModelIdOf,
  matchModelItem,
  modelKeyOf,
  modelOptionLabel,
  pairRefusal,
  parseModelRef,
} from "./AgentProfileDraft";

function item(modelId: string, providerId?: string | null, name?: string): SessionModel {
  return {
    modelId,
    name: name ?? modelId,
    providerId: providerId ?? null,
  };
}

const ITEMS: SessionModel[] = [
  item("opencode-go/mimo-v2-6-flash", "opencode-go", "MiMo V2.6 Flash"),
  item("openrouter/mimo-v2-6-flash", "openrouter", "MiMo V2.6 Flash"),
  item("opus", null, "Opus"),
];

describe("modelKeyOf", () => {
  it("joins provider and id, or keeps the bare id", () => {
    expect(modelKeyOf("opencode-go", "mimo")).toBe("opencode-go/mimo");
    expect(modelKeyOf(null, "opus")).toBe("opus");
    expect(modelKeyOf("", "opus")).toBe("opus");
  });
});

describe("bareModelIdOf", () => {
  it("strips the serving prefix, and leaves bare rows alone", () => {
    expect(bareModelIdOf(ITEMS[0]!)).toBe("mimo-v2-6-flash");
    expect(bareModelIdOf(ITEMS[2]!)).toBe("opus");
  });

  it("never strips a prefix the row does not carry", () => {
    expect(bareModelIdOf(item("weird", "other"))).toBe("weird");
  });
});

describe("parseModelRef", () => {
  it("splits a listed composite back into the pair", () => {
    expect(parseModelRef("openrouter/mimo-v2-6-flash", ITEMS)).toEqual({
      model: "mimo-v2-6-flash",
      modelProvider: "openrouter",
    });
  });

  it("keeps a bare value bare, even when it holds a slash", () => {
    // A legacy bare id that happens to contain a `/` is not a provider the
    // catalog never named.
    expect(parseModelRef("qwen/qwen3-flash", ITEMS)).toEqual({
      model: "qwen/qwen3-flash",
      modelProvider: null,
    });
    expect(parseModelRef("opus", ITEMS)).toEqual({ model: "opus", modelProvider: null });
  });
});

describe("matchModelItem", () => {
  it("matches the exact composite pair", () => {
    expect(matchModelItem(ITEMS, "mimo-v2-6-flash", "openrouter")).toBe(ITEMS[1]);
  });

  it("matches a legacy bare id only onto its single row", () => {
    expect(matchModelItem(ITEMS, "opus", null)).toBe(ITEMS[2]);
  });

  it("matches nothing for an ambiguous bare id or a stale pair", () => {
    expect(matchModelItem(ITEMS, "mimo-v2-6-flash", null)).toBeUndefined();
    expect(matchModelItem(ITEMS, "mimo-v2-6-flash", "gone-provider")).toBeUndefined();
    expect(matchModelItem(ITEMS, "ghost", null)).toBeUndefined();
  });
});

describe("pairRefusal", () => {
  it("names the providers for an ambiguous bare id", () => {
    expect(pairRefusal(ITEMS, "mimo-v2-6-flash", null, "pi")).toBe(
      "Pi model 'mimo-v2-6-flash' is offered by opencode-go and openrouter; pick one in the profile.",
    );
  });

  it("names the providers for a stale pair with surviving alternates", () => {
    expect(pairRefusal(ITEMS, "mimo-v2-6-flash", "gone-provider", "pi")).toContain(
      "offered by opencode-go and openrouter",
    );
  });

  it("stays quiet for exact pairs, single rows, unknown models and other providers", () => {
    expect(pairRefusal(ITEMS, "mimo-v2-6-flash", "openrouter", "pi")).toBeNull();
    expect(pairRefusal(ITEMS, "opus", null, "pi")).toBeNull();
    expect(pairRefusal(ITEMS, "ghost", null, "pi")).toBeNull();
    expect(pairRefusal(ITEMS, "", null, "pi")).toBeNull();
    expect(pairRefusal(ITEMS, "mimo-v2-6-flash", null, "claude")).toBeNull();
  });
});

describe("modelOptionLabel", () => {
  it("leads with the serving provider so same-id rows read apart", () => {
    expect(modelOptionLabel(ITEMS[0]!)).toBe("opencode-go · MiMo V2.6 Flash (mimo-v2-6-flash)");
    expect(modelOptionLabel(ITEMS[2]!)).toBe("Opus (opus)");
  });
});
