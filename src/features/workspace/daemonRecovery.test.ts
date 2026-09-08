import { describe, expect, it, vi } from "vitest";
import type { DaemonRecoveryDeps } from "./daemonRecovery";
import type { DaemonStatus } from "../../types/ipc";
import { createDaemonRecovery } from "./daemonRecovery";
const unresponsive = (message = "the daemon stopped answering"): DaemonStatus => ({
  state: "unresponsive",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 1,
  clients: 1,
  capabilities: ["typed_permissions"],
  message,
});

const connected = (): DaemonStatus => ({
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 1,
  clients: 1,
  capabilities: ["typed_permissions"],
  message: null,
});

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

function createDeps() {
  const restart = vi.fn(async () => undefined);
  const ask = vi.fn(
    async (
      _message: string,
      _options?: {
        title?: string;
        kind?: "info" | "warning" | "error";
        okLabel?: string;
        cancelLabel?: string;
      },
    ) => true,
  );
  const deps: DaemonRecoveryDeps = { restart, ask };
  return { deps, restart, ask };
}

async function flush(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("daemon recovery decision", () => {
  it("with no roster injected, an unresponsive status asks instead of restarting", async () => {
    // Direction of failure: a missing injection must produce an unnecessary
    // question, never a silent destructive restart.
    const restart = vi.fn(async () => undefined);
    const ask = vi.fn(async () => false);
    const recovery = createDaemonRecovery({ restart, ask });

    recovery.onStatus(unresponsive());
    await flush();

    expect(ask).toHaveBeenCalledTimes(1);
    expect(restart).not.toHaveBeenCalled();
  });

  it("restarts straight away when nothing is live, without asking, once per episode", async () => {
    const { deps, restart, ask } = createDeps();
    const recovery = createDaemonRecovery(deps);
    recovery.setRoster(false);

    recovery.onStatus(unresponsive());
    await flush();
    recovery.onStatus(unresponsive());
    recovery.onStatus(unresponsive());
    await flush();

    expect(restart).toHaveBeenCalledTimes(1);
    expect(ask).not.toHaveBeenCalled();
  });

  it("asks first when a session is live and restarts only on confirm", async () => {
    const { deps, restart, ask } = createDeps();
    const recovery = createDaemonRecovery(deps);
    recovery.setRoster(true);

    recovery.onStatus(unresponsive());
    await flush();

    expect(ask).toHaveBeenCalledTimes(1);
    // The wording must say plainly what is lost and what is kept.
    const message = String(ask.mock.calls[0]?.[0]);
    expect(message).toContain("stop");
    expect(message).toContain("conversations are kept");
    expect(restart).toHaveBeenCalledTimes(1);
  });

  it("does not re-ask while the dialog is still open", async () => {
    const gate = createDeferred<boolean>();
    const { deps, restart, ask } = createDeps();
    ask.mockImplementation(() => gate.promise);
    const recovery = createDaemonRecovery(deps);
    recovery.setRoster(true);

    recovery.onStatus(unresponsive());
    recovery.onStatus(unresponsive());
    recovery.onStatus(unresponsive());
    expect(ask).toHaveBeenCalledTimes(1);

    gate.resolve(true);
    await flush();
    expect(restart).toHaveBeenCalledTimes(1);
  });

  it("a decline restarts nothing and is not re-asked on the next unresponsive status", async () => {
    const { deps, restart, ask } = createDeps();
    ask.mockResolvedValue(false);
    const recovery = createDaemonRecovery(deps);
    recovery.setRoster(true);

    recovery.onStatus(unresponsive());
    await flush();
    recovery.onStatus(unresponsive());
    await flush();

    expect(ask).toHaveBeenCalledTimes(1);
    expect(restart).not.toHaveBeenCalled();
  });

  it("recovery resets the episode, so a later hang asks again", async () => {
    const { deps, restart, ask } = createDeps();
    ask.mockResolvedValue(false);
    const recovery = createDaemonRecovery(deps);
    recovery.setRoster(true);

    recovery.onStatus(unresponsive());
    await flush();
    expect(ask).toHaveBeenCalledTimes(1);

    recovery.onStatus(connected());
    recovery.onStatus(unresponsive());
    await flush();

    expect(ask).toHaveBeenCalledTimes(2);
    expect(restart).not.toHaveBeenCalled();
  });

  it("a stale roster causes neither a second dialog nor a second restart", async () => {
    // The roster is frozen while the daemon is wedged: the session keeps
    // reporting live (or unchanged) across every poll.
    const { deps, restart, ask } = createDeps();
    const recovery = createDaemonRecovery(deps);
    recovery.setRoster(true);

    recovery.onStatus(unresponsive());
    await flush();
    recovery.onStatus(unresponsive());
    recovery.onStatus(unresponsive());
    recovery.onStatus(unresponsive());
    await flush();

    expect(ask).toHaveBeenCalledTimes(1);
    expect(restart).toHaveBeenCalledTimes(1);
  });

  describe("restart attempt failure note", () => {
    it("says an attempt was made when the silent restart rejects, and keeps it across polls", async () => {
      const restart = vi.fn(async () => {
        throw new Error("daemon identity changed");
      });
      const recovery = createDaemonRecovery({ restart });
      recovery.setRoster(false);

      recovery.onStatus(unresponsive());
      await flush();
      expect(recovery.note()).toBe("a restart was attempted, but it did not complete");

      // The next unresponsive poll must not flicker the note away.
      recovery.onStatus(unresponsive());
      await flush();
      expect(recovery.note()).toBe("a restart was attempted, but it did not complete");
    });

    it("says an attempt was made when a confirmed restart rejects", async () => {
      const restart = vi.fn(async () => {
        throw new Error("daemon identity changed");
      });
      const recovery = createDaemonRecovery({
        restart,
        ask: async () => true,
      });

      recovery.onStatus(unresponsive());
      await flush();
      expect(recovery.note()).toBe("a restart was attempted, but it did not complete");
    });

    it("holds no note before any attempt", () => {
      const recovery = createDaemonRecovery();
      expect(recovery.note()).toBeNull();
    });

    it("the note disappears when the daemon recovers, like the episode", async () => {
      const restart = vi.fn(async () => {
        throw new Error("daemon identity changed");
      });
      const recovery = createDaemonRecovery({ restart });
      recovery.setRoster(false);

      recovery.onStatus(unresponsive());
      await flush();
      expect(recovery.note()).not.toBeNull();

      recovery.onStatus(connected());
      expect(recovery.note()).toBeNull();

      // A later episode starts clean: a hang that is never restarted shows no
      // stale attempt-failed sentence.
      recovery.onStatus(unresponsive());
      expect(recovery.note()).toBeNull();
    });
  });
});
