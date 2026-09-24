import { describe, expect, it, vi } from "vitest";
import { createDelegationController } from "./delegation";
import type { DelegationReply } from "../types/ipc";

const fileOn: DelegationReply = { enabled: true, source: "file" };

/** Lets the controller's un-awaited follow-up reads (a rejection's re-read)
 * finish before the assertions run. */
const flush = async (): Promise<void> => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

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
    expect(controller.getState().error).toEqual({ sentence: "the daemon refused", detail: null });
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
    // The re-read the refusal schedules is held back forever here: this test
    // pins the settlement of the write itself — the revert and the sentence —
    // and the re-read's later correction has its own tests below.
    const get = vi
      .fn<() => Promise<DelegationReply>>()
      .mockResolvedValueOnce({ enabled: false, source: "file" })
      .mockImplementation(() => new Promise<DelegationReply>(() => undefined));
    const set = vi.fn(async () => {
      throw new Error("the store would not take it");
    });
    const controller = createDelegationController({ get, set });
    await controller.load();
    const confirmed = await controller.setEnabled(true);
    expect(confirmed).toBe(false);
    expect(controller.getState().enabled).toBe(false);
    expect(controller.getState().error).toEqual({
      sentence: "the store would not take it",
      detail: null,
    });
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
    // The racer's reply never adopted: the standing reply's source is the
    // write's own minted "file", not the racer's "default". (Source alone
    // cannot tell the first load's reply from the write's mint — both are
    // "file"; the enabled assertion above is what pins whose reply stands.)
    expect(controller.getState().reply?.source).toBe("file");
  });

  it("a fetch that started while a write was in flight adopts nothing, sequence untouched", async () => {
    // The guard's two halves catch two different races. A write issued BEFORE
    // the fetch bumps the sequence before the fetch reads it, so the sequence
    // half alone never fires here: the write was in flight when the fetch
    // started, and whether the reply predates or postdates the write is
    // unknowable. Only the in-flight half can refuse this reply.
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
    const write = controller.setEnabled(true);
    const fetch = controller.load();
    releaseGet();
    await Promise.all([fetch, write]);
    // The write confirmed ON; the racer's OFF never adopts over it — not the
    // switch, and not the stored reply the next refusal would revert onto.
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().reply).toEqual({ enabled: true, source: "file" });
  });

  it("an accepted write that a newer write superseded still stamps the confirmation", async () => {
    // F1 (re-audit): `confirmedRef` is a fact about the DAEMON — it must move
    // whenever the daemon accepts, even when a newer write owns the UI by the
    // time the acceptance lands. OFF confirmed by the load; write A flips ON
    // and the daemon TAKES it; write B flips OFF and the daemon refuses B. A
    // refusal that reverts onto the load's OFF shows a switch reading "off"
    // over a daemon holding ON — agents keep answering their children's
    // cards while the panel says nobody does.
    let releaseA!: () => void;
    const gateA = new Promise<void>((resolve) => {
      releaseA = () => resolve();
    });
    const calls: boolean[] = [];
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set: async (enabled) => {
        calls.push(enabled);
        if (calls.length === 1) {
          await gateA; // A accepted.
          return;
        }
        throw new Error("store B refused"); // B refused.
      },
    });
    await controller.load();
    const first = controller.setEnabled(true);
    const second = controller.setEnabled(false);
    releaseA();
    await Promise.all([first, second]);
    expect(calls).toEqual([true, false]);
    // The daemon accepted ON and refused OFF: ON is what the switch shows.
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().reply).toEqual({ enabled: true, source: "file" });
    expect(controller.getState().error).toEqual({ sentence: "store B refused", detail: null });
  });

  it("an accepted write surfaces even when the newer refusal settled before it", async () => {
    // The other settle order of the same interleaving: B's refusal lands
    // first — the panel reverts onto the load's OFF — and THEN A's acceptance
    // arrives. The acceptance is still a fact about the daemon, and a consent
    // surface never shows less authority than is live. The re-read the
    // refusal schedules is held back: this test pins the settle order, and
    // the re-read would adopt (and clear the sentence) only after it.
    let releaseA!: () => void;
    const gateA = new Promise<void>((resolve) => {
      releaseA = () => resolve();
    });
    let releaseB!: () => void;
    const gateB = new Promise<void>((resolve) => {
      releaseB = () => resolve();
    });
    const calls: boolean[] = [];
    const get = vi
      .fn<() => Promise<DelegationReply>>()
      .mockResolvedValueOnce({ enabled: false, source: "file" })
      .mockImplementation(() => new Promise<DelegationReply>(() => undefined));
    const controller = createDelegationController({
      get,
      set: async (enabled) => {
        calls.push(enabled);
        if (calls.length === 1) {
          await gateA; // A accepted.
          return;
        }
        await gateB;
        throw new Error("store B refused"); // B refused.
      },
    });
    await controller.load();
    const first = controller.setEnabled(true);
    const second = controller.setEnabled(false);
    releaseA();
    await first;
    // A's acceptance landed while B still flew: the daemon holds ON, so the
    // switch shows ON, not B's optimistic OFF.
    expect(controller.getState().enabled).toBe(true);
    releaseB();
    await second;
    // B's refusal reverts onto the stamped confirmation — ON — not onto OFF.
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().reply).toEqual({ enabled: true, source: "file" });
    // B's refusal is still the newest refused write, so its sentence stands.
    expect(controller.getState().error).toEqual({ sentence: "store B refused", detail: null });
  });

  it("two rapid writes both accepted settle on the newer value when responses land in order", async () => {
    // What this interleaving pins is the END of the trace. The published
    // trace is ON -> off -> ON -> off: while B is still in flight, A's
    // superseded acceptance DOES move the display onto A's ON — a consent
    // surface never shows less authority than is live, and the arm at the
    // acceptance says so itself. What must not happen is that move surviving
    // B's own acceptance: B's is the last acceptance to land, so OFF stands
    // at the end with nothing left in flight to ride over it.
    let releaseA!: () => void;
    const gateA = new Promise<void>((resolve) => {
      releaseA = () => resolve();
    });
    let releaseB!: () => void;
    const gateB = new Promise<void>((resolve) => {
      releaseB = () => resolve();
    });
    const calls: boolean[] = [];
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set: async (enabled) => {
        calls.push(enabled);
        if (calls.length === 1) {
          await gateA; // A accepted first.
          return;
        }
        await gateB; // B accepted second.
        return;
      },
    });
    await controller.load();
    const first = controller.setEnabled(true);
    const second = controller.setEnabled(false);
    releaseA();
    await first;
    // The window the old comment denied: A's superseded acceptance holds the
    // display while B flies. Pinned here so the trace this test promises is
    // the trace it assures.
    expect(controller.getState().enabled).toBe(true);
    releaseB();
    await second;
    expect(calls).toEqual([true, false]);
    expect(controller.getState().enabled).toBe(false);
    expect(controller.getState().reply).toEqual({ enabled: false, source: "file" });
    expect(controller.getState().error).toBeNull();
  });

  it("an older acceptance landing after a newer one shows what the daemon took last", async () => {
    // The app cannot see the daemon's processing order — only which
    // acceptance landed last. The responses here deliver B first, A second,
    // so the daemon ends holding A's ON; the panel follows that fact instead
    // of freezing on the write that was issued last. A consent surface
    // erring under unknowable order errs toward the authority that is live.
    let releaseA!: () => void;
    const gateA = new Promise<void>((resolve) => {
      releaseA = () => resolve();
    });
    let releaseB!: () => void;
    const gateB = new Promise<void>((resolve) => {
      releaseB = () => resolve();
    });
    const calls: boolean[] = [];
    const controller = createDelegationController({
      get: async () => ({ enabled: false, source: "file" }),
      set: async (enabled) => {
        calls.push(enabled);
        if (calls.length === 1) {
          await gateA; // A accepted, response delivered second.
          return;
        }
        await gateB; // B accepted, response delivered first.
        return;
      },
    });
    await controller.load();
    const first = controller.setEnabled(true);
    const second = controller.setEnabled(false);
    releaseB();
    await second;
    expect(controller.getState().enabled).toBe(false);
    releaseA();
    await first;
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().reply).toEqual({ enabled: true, source: "file" });
    expect(controller.getState().error).toBeNull();
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
    expect(controller.getState().error?.sentence).toContain("incomplete");
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
    //
    // Scope note (audit 3, F8): at the one execution of the revert line this
    // interleaving reaches, B's optimistic value and the confirmed value
    // coincide (B flips back to OFF), so the PUBLISHED panel cannot
    // discriminate the revert's base here — what this test still guards is
    // the load's stamp feeding that revert (removing it fails these
    // assertions). The base itself is witnessed by the mirror test below, in
    // the interleaving where the two values part ways: one refused write,
    // then a retry of the same value.
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
    expect(controller.getState().error).toEqual({ sentence: "store B refused", detail: null });
  });

  it("a refused write after a CONFIRMED write reverts onto the confirmation", async () => {
    // ON confirmed by the daemon; a refused OFF must put ON back — the value
    // the daemon actually holds — not the pre-write optimistic base. The
    // re-read the refusal schedules is held back: this test pins the revert
    // moment; the re-read's correction has its own tests below.
    const get = vi
      .fn<() => Promise<DelegationReply>>()
      .mockResolvedValueOnce({ enabled: false, source: "file" })
      .mockImplementation(() => new Promise<DelegationReply>(() => undefined));
    const controller = createDelegationController({
      get,
      set: vi
        .fn()
        .mockResolvedValueOnce(undefined)
        .mockRejectedValueOnce(new Error("the store refused the second")),
    });
    await controller.load();
    expect(await controller.setEnabled(true)).toBe(true);
    expect(await controller.setEnabled(false)).toBe(false);
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().error).toEqual({
      sentence: "the store refused the second",
      detail: null,
    });
  });

  it("a rejected write re-reads, and the daemon's answer replaces the fallback", async () => {
    // Audit 3 F1: a rejection is a fact about the transport, not the store —
    // the daemon may have applied the write and lost the reply. The refusal
    // is reported, then the controller goes and finds out; the answer that
    // comes back through the guarded read path is what the panel ends on,
    // with the refusal sentence cleared.
    let releaseReread!: () => void;
    const reread = new Promise<DelegationReply>((resolve) => {
      releaseReread = () => resolve({ enabled: true, source: "file" });
    });
    let call = 0;
    const controller = createDelegationController({
      get: vi.fn(async (): Promise<DelegationReply> =>
        call++ === 0 ? { enabled: false, source: "file" } : reread,
      ),
      set: vi.fn(async () => {
        throw new Error("the app did not answer");
      }),
    });
    await controller.load();
    expect(await controller.setEnabled(true)).toBe(false);
    // The fallback stands only until the re-read answers.
    expect(controller.getState().enabled).toBe(false);
    expect(controller.getState().error).toEqual({
      sentence: "the app did not answer",
      detail: null,
    });
    releaseReread();
    await flush();
    // The daemon HAD applied the lost write: the panel now says so.
    expect(controller.getState().enabled).toBe(true);
    expect(controller.getState().reply).toEqual({ enabled: true, source: "file" });
    expect(controller.getState().error).toBeNull();
  });

  it("a rejected write whose re-read also fails leaves the panel unknown, never a definite off", async () => {
    // After a rejected write, the last confirmed value is a belief about a
    // store the app just failed to reach; if the read sent to settle that
    // doubt fails too, rendering that belief as a definite switch is the
    // false claim this file exists to prevent. The panel goes to the named
    // unknown — where Retry lives, and where the take-back still works.
    let call = 0;
    const controller = createDelegationController({
      get: vi.fn(async (): Promise<DelegationReply> => {
        call += 1;
        if (call === 1) return { enabled: false, source: "file" };
        throw new Error("the daemon is gone");
      }),
      set: vi.fn(async () => {
        throw new Error("the app did not answer");
      }),
    });
    await controller.load();
    expect(await controller.setEnabled(true)).toBe(false);
    await flush();
    expect(controller.getState().enabled).toBeNull();
    expect(controller.getState().loadFailed).toBe(true);
    expect(controller.getState().error).toEqual({ sentence: "the daemon is gone", detail: null });
  });

  it("the control that stops delegation works from the unknown state; granting still needs an answer", async () => {
    // The asymmetric half of the write rule (audit 3, F2): `false` can only
    // reduce what the daemon exercises, so the take-back must act from the
    // unknown — the human may be reaching for it BECAUSE the answer cannot
    // be read. `true` from silence is still the write-from-a-guess this
    // store refuses, and `false` over a confirmed `false` stays a no-op.
    const set = vi.fn(async () => undefined);
    const controller = createDelegationController({
      get: vi.fn(async (): Promise<DelegationReply> => {
        throw new Error("the daemon is unreachable");
      }),
      set,
    });
    await controller.load();
    expect(controller.getState().loadFailed).toBe(true);
    expect(await controller.setEnabled(true)).toBe(false);
    expect(set).not.toHaveBeenCalled();
    expect(await controller.setEnabled(false)).toBe(true);
    expect(set).toHaveBeenCalledTimes(1);
    expect(set).toHaveBeenCalledWith(false);
    expect(controller.getState().enabled).toBe(false);
    expect(controller.getState().loadFailed).toBe(false);
    // And once the store holds false, another take-back is a no-op, not a
    // second write.
    expect(await controller.setEnabled(false)).toBe(false);
    expect(set).toHaveBeenCalledTimes(1);
  });

  it("a refused write leaves the mirror on the CONFIRMED base, so the same value can still be written", async () => {
    // Audit 3 F8: the refusal's published panel is the confirmed value under
    // either revert base, so only the ref mirror can witness where the
    // revert landed — a retry of the refused value reaches the daemon only
    // if the mirror went back to the confirmed value instead of the
    // optimistic one. The re-read the refusal scheduled is gated, so at the
    // moment of the retry nothing but the mirror can answer the guard.
    let releaseReread!: () => void;
    const reread = new Promise<DelegationReply>((resolve) => {
      releaseReread = () => resolve({ enabled: false, source: "file" });
    });
    let call = 0;
    const set = vi.fn(async () => {
      if (set.mock.calls.length === 1) throw new Error("store refused");
    });
    const controller = createDelegationController({
      get: vi.fn(async (): Promise<DelegationReply> =>
        call++ === 0 ? { enabled: false, source: "file" } : reread,
      ),
      set,
    });
    await controller.load();
    expect(await controller.setEnabled(true)).toBe(false);
    const retry = controller.setEnabled(true);
    expect(set).toHaveBeenCalledTimes(2);
    releaseReread();
    expect(await retry).toBe(true);
    await flush();
    // The retried write confirmed; the stale re-read (issued before it) was
    // discarded by the sequence guard, not allowed to overwrite it.
    expect(controller.getState().enabled).toBe(true);
  });
});
