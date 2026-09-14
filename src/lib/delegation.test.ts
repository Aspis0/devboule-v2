import { describe, expect, it, vi } from "vitest";
import { createDelegationController } from "./delegation";
import type { DelegationReply } from "../types/ipc";

const fileOn: DelegationReply = { enabled: true, source: "file" };

describe("delegation controller", () => {
  it("adopts the store's answer, value and source together", async () => {
    const get = vi.fn(async () => fileOn);
    const controller = createDelegationController({ get, set: async () => undefined });
    await controller.load();
    expect(controller.getState().reply).toEqual(fileOn);
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().loadFailed).toBe(false);
  });

  it("a failed first load is terminal, with the daemon's sentence and a way back", async () => {
    const get = vi.fn(async (): Promise<DelegationReply> => {
      throw new Error("the daemon refused");
    });
    const controller = createDelegationController({ get, set: async () => undefined });
    await controller.load();
    expect(controller.getState().loadFailed).toBe(true);
    expect(controller.getState().enabled).toBeNull();
    expect(controller.getState().error).toBe("the daemon refused");
    // A later successful load recovers the panel.
    get.mockResolvedValue({ enabled: false, source: "default" });
    await controller.load();
    expect(controller.getState().loadFailed).toBe(false);
    expect(controller.getState().enabled).toBe(false);
  });

  it("an optimistic write keeps its value when it confirms", async () => {
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set: vi.fn(async () => undefined),
    });
    await controller.load();
    const confirmed = await controller.setEnabled(true);
    expect(confirmed).toBe(true);
    expect(controller.getState().enabled).toBe(true);
  });

  it("a refused write reverts to the value the human was seeing and reports the sentence", async () => {
    const set = vi.fn(async () => {
      throw new Error("the store would not take it");
    });
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set,
    });
    await controller.load();
    const confirmed = await controller.setEnabled(true);
    expect(confirmed).toBe(false);
    expect(controller.getState().enabled).toBe(false);
    expect(controller.getState().error).toBe("the store would not take it");
  });

  it("an older rejection owns nothing once a newer write superseded it", async () => {
    // The first write's set hangs until the test releases it; the second
    // write runs and confirms first, so when the first rejection lands it
    // must revert nothing and report nothing — the newer value stands.
    let releaseFirst!: () => void;
    const firstSet = new Promise<void>((resolve) => {
      releaseFirst = () => resolve();
    });
    const calls: boolean[] = [];
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set: async (enabled) => {
        calls.push(enabled);
        if (calls.length === 1) {
          await firstSet;
          throw new Error("stale rejection");
        }
      },
    });
    await controller.load();
    const first = controller.setEnabled(true);
    // The second write is issued in the same tick, so its base reads the
    // first write's optimistic value from the ref, not a render closure.
    const second = controller.setEnabled(false);
    releaseFirst();
    await Promise.all([first, second]);
    expect(calls).toEqual([true, false]);
    expect(controller.getState().enabled).toBe(false);
    expect(controller.getState().error).toBeNull();
  });

  it("a fetch that overlapped a write adopts nothing — the write's settle is the record", async () => {
    let releaseGet!: () => void;
    const gate = new Promise<DelegationReply>((resolve) => {
      releaseGet = () => resolve({ enabled: false, source: "default" });
    });
    // The first read answers at once so the panel holds a value (a write
    // never starts from a guess); the refresh overlaps a write.
    let call = 0;
    const controller = createDelegationController({
      get: async () => (call++ === 0 ? { enabled: false, source: "file" } : gate),
      set: async () => undefined,
    });
    await controller.load();
    const fetch = controller.load();
    // The write starts while the fetch is in flight; it is newer than
    // whatever the fetch will answer.
    const write = controller.setEnabled(true);
    releaseGet();
    await Promise.all([fetch, write]);
    expect(controller.getState().enabled).toBe(true);
  });

  it("a fetch started before a write adopts nothing when the write lands mid-flight", async () => {
    let releaseGet!: () => void;
    const gate = new Promise<DelegationReply>((resolve) => {
      releaseGet = () => resolve({ enabled: false, source: "default" });
    });
    let call = 0;
    const controller = createDelegationController({
      get: async () => (call++ === 0 ? { enabled: false, source: "file" } : gate),
      set: async () => undefined,
    });
    await controller.load();
    const fetch = controller.load();
    const write = controller.setEnabled(true);
    // Both settle; the guard must reject the reply on the sequence alone too
    // (the write bumped it while the fetch flew).
    releaseGet();
    await Promise.all([fetch, write]);
    expect(controller.getState().enabled).toBe(true);
    // The raced refresh's reply was refused; the standing reply is still the
    // one the first load adopted (source "file"), not the racer's.
    expect(controller.getState().reply?.source).toBe("file");
  });

  it("set does nothing from a guess: no write before the store has answered", async () => {
    const set = vi.fn(async () => undefined);
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set,
    });
    const confirmed = await controller.setEnabled(true);
    expect(confirmed).toBe(false);
    expect(set).not.toHaveBeenCalled();
  });

  it("refuses a reply whose enabled field is absent — silence is not an answer to write from", async () => {
    // DelegationReply is typed, but the module reaches the wire through a
    // boundary cast; the cast is undone here so the test can build the reply
    // a stub or a field-skipping serializer could actually deliver. An absent
    // `enabled` read as a real answer would authorise a write from a guess.
    const replyWithoutEnabled = { source: "file" } as unknown as DelegationReply;
    const set = vi.fn(async () => undefined);
    const controller = createDelegationController({
      get: vi.fn(async () => replyWithoutEnabled),
      set,
    });
    await controller.load();
    expect(controller.getState().reply).toBeNull();
    expect(controller.getState().enabled).toBeNull();
    expect(controller.getState().loadFailed).toBe(true);
    expect(controller.getState().error).toContain("incomplete");
    // And the write path stays locked: no write ever starts from that silence.
    const confirmed = await controller.setEnabled(true);
    expect(confirmed).toBe(false);
    expect(set).not.toHaveBeenCalled();
  });

  it("refuses a reply whose source field is absent", async () => {
    const replyWithoutSource = { enabled: true } as unknown as DelegationReply;
    const controller = createDelegationController({
      get: vi.fn(async () => replyWithoutSource),
      set: async () => undefined,
    });
    await controller.load();
    expect(controller.getState().reply).toBeNull();
    expect(controller.getState().enabled).toBeNull();
    expect(controller.getState().loadFailed).toBe(true);
  });

  it("a successful write adopts the reply, so the source sentence cannot contradict the switch", async () => {
    // The daemon took the write, which makes the stored answer this human's
    // own deliberate one — the fact its `file` source names. Freezing the
    // LAST FETCH's reply left the panel saying "delegation reads off" beside
    // a switch it had just turned on.
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "quarantined" }),
      set: async () => undefined,
    });
    await controller.load();
    expect(controller.getState().reply?.source).toBe("quarantined");
    const confirmed = await controller.setEnabled(true);
    expect(confirmed).toBe(true);
    expect(controller.getState().reply).toEqual({ enabled: true, source: "file" });
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().error).toBeNull();
  });

  it("two refused rapid writes revert onto the last CONFIRMED value, not an optimistic one", async () => {
    // OFF (confirmed by load). Write A flips ON; write B flips OFF before A
    // settles, so B's optimistic base was A's unconfirmed true. Both writes
    // are refused. Reverting onto B's base would leave the panel showing ON
    // — a value the daemon never accepted — over a daemon holding OFF.
    let releaseFirst!: () => void;
    const firstSet = new Promise<void>((resolve) => {
      releaseFirst = () => resolve();
    });
    const calls: boolean[] = [];
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set: async (enabled) => {
        calls.push(enabled);
        if (calls.length === 1) {
          await firstSet;
          throw new Error("store A refused");
        }
        throw new Error("store B refused");
      },
    });
    await controller.load();
    const first = controller.setEnabled(true);
    const second = controller.setEnabled(false);
    releaseFirst();
    await Promise.all([first, second]);
    expect(calls).toEqual([true, false]);
    // The daemon confirmed OFF at load and nothing since: OFF is what shows.
    expect(controller.getState().enabled).toBe(false);
    // B's refusal is the newest, so its sentence is the one reported.
    expect(controller.getState().error).toBe("store B refused");
  });

  it("a refused write after a CONFIRMED write reverts onto the confirmation", async () => {
    // ON confirmed by the daemon; a refused OFF must put ON back — the value
    // the daemon actually holds — not the pre-write optimistic base.
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set: vi
        .fn()
        .mockResolvedValueOnce(undefined)
        .mockRejectedValueOnce(new Error("the store refused the second")),
    });
    await controller.load();
    expect(await controller.setEnabled(true)).toBe(true);
    expect(await controller.setEnabled(false)).toBe(false);
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().error).toBe("the store refused the second");
  });
});
