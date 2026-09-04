import type { Channel } from "@tauri-apps/api/core";
import { isCommandError } from "../../lib/tauri";
import type {
  Session,
  SessionEvent,
  SessionSnapshot,
  PermissionRequest,
  UnverifiableTranscriptIntegrity,
} from "../../types/ipc";
import type { TerminalViewHandle } from "./createTerminalView";
import type { TerminalSessionRegistry } from "./terminalRegistry";

export type TerminalEvent = SessionEvent;
export type TerminalChannel = Channel<TerminalEvent>;

export type TerminalBanner =
  | {
      kind: "exited";
      code: number | null;
      lost: { frames: number; bytes: number } | null;
      trimmedBytes: number;
    }
  | { kind: "silent"; elapsedMs: number }
  /**
   * The session was reopened from a journal nobody closed orderly. The
   * counters preserve any loss measured before the daemon died.
   */
  | { kind: "recovered"; integrity: UnverifiableTranscriptIntegrity }
  | { kind: "journal_degraded"; lost: { frames: number; bytes: number } }
  | { kind: "closed" }
  | { kind: "error"; message: string }
  | null;

type PersistentTerminalBanner = Exclude<TerminalBanner, { kind: "silent" } | null>;

export interface TerminalSessionDeps {
  workspaceId: string | null;
  /** A Workspace tab may select a listed session instead of adopting by workspace. */
  sessionId?: string | null;
  host: HTMLElement;
  createView: (
    host: HTMLElement,
    options: { onData: (data: string) => void; onCtrlC: () => void },
  ) => Promise<TerminalViewHandle>;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  createChannel: (onEvent: (event: TerminalEvent) => void) => TerminalChannel;
  registry: TerminalSessionRegistry;
  onBanner: (banner: TerminalBanner) => void;
  onCtrlCArmed: (armed: boolean) => void;
  onExited?: (code: number | null) => void;
  onPermissionRequest?: (request: PermissionRequest) => void;
  setTimeout?: (callback: () => void, milliseconds: number) => number;
  clearTimeout?: (id: number) => void;
  scheduleFrame?: (callback: () => void) => number;
  cancelFrame?: (id: number) => void;
}

const RESIZE_DEBOUNCE_MS = 150;
const CTRL_C_ARM_MS = 3000;
const WRITE_FAIL_THRESHOLD = 2;

function errorMessage(error: unknown): string {
  if (isCommandError(error) && error.message.trim()) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message) return error.message;
  return "Unknown terminal error.";
}

function eventTypeName(event: unknown): string {
  if (typeof event !== "object" || event === null || !("type" in event)) return "unknown";
  const type = event.type;
  return typeof type === "string" && type.trim() ? type : "unknown";
}

/** An output with seq <= asOfSeq is already represented by the snapshot and must never be applied again. */
function isSnapshotCoveredOutput(seq: number, asOfSeq: number): boolean {
  return seq <= asOfSeq;
}

/**
 * Headless controller for one terminal session. It owns IPC ordering, output
 * batching, resize debounce, interrupt confirmation, and cleanup; React only
 * supplies the host and renders the small status banner.
 */
export class TerminalSession {
  private readonly setTimer: (callback: () => void, milliseconds: number) => number;
  private readonly clearTimer: (id: number) => void;
  private readonly scheduleFrame: (callback: () => void) => number;
  private readonly cancelFrame: (id: number) => void;

  private sessionId: string | null = null;
  private channel: TerminalChannel | null = null;
  private view: TerminalViewHandle | null = null;
  private started = false;
  private disposed = false;
  private exited = false;
  private lastSeenSeq: number | null = null;
  private attachPending = false;
  private applyingSnapshot = false;
  private snapshotReceived = false;
  private snapshotAsOfSeq: number | null = null;
  private backendTeardown: "detach" | "close" | null = null;
  private backendTeardownSent = false;

  private readonly pendingOutput: Array<{ seq: number; data: string }> = [];
  private readonly pendingSnapshotEvents: TerminalEvent[] = [];
  private readonly pendingInput: string[] = [];
  private outputFrame: number | null = null;
  private ctrlCArmed = false;
  private ctrlCTimer: number | null = null;
  private resizeTimer: number | null = null;
  private writeFailCount = 0;
  private silenceBannerVisible = false;
  private persistentBanner: PersistentTerminalBanner | null = null;
  private journalLoss: { frames: number; bytes: number } | null = null;

  constructor(private readonly deps: TerminalSessionDeps) {
    this.setTimer =
      deps.setTimeout ?? ((callback, milliseconds) => window.setTimeout(callback, milliseconds));
    this.clearTimer = deps.clearTimeout ?? ((id) => window.clearTimeout(id));
    this.scheduleFrame =
      deps.scheduleFrame ?? ((callback) => window.requestAnimationFrame(callback));
    this.cancelFrame = deps.cancelFrame ?? ((id) => window.cancelAnimationFrame(id));
  }

  /** Adopt or create, attach, and size the session. Every backend rejection is handled here. */
  async start(): Promise<void> {
    if (this.disposed || this.started) return;
    this.started = true;

    const requestedSessionId = this.deps.sessionId ?? null;
    const existing =
      requestedSessionId === null ? this.deps.registry.get(this.deps.workspaceId) : null;
    const adopted = requestedSessionId === null && existing !== null;
    let sessionId = requestedSessionId ?? existing?.sessionId ?? null;

    if (sessionId === null) {
      try {
        const listed = await this.deps.invoke<Session[]>("sessions_list");
        const restorable = pickRestorable(listed, this.deps.workspaceId);
        if (restorable !== null) {
          sessionId = restorable.id;
          this.deps.registry.register(this.deps.workspaceId, sessionId);
        }
      } catch {
        // Listing is best-effort. Create still works if the journal is down.
      }
    }

    if (sessionId === null) {
      let session: Session;
      try {
        session = await this.deps.invoke<Session>("session_create", {
          workspace_id: this.deps.workspaceId,
          kind: "terminal",
        });
      } catch (error: unknown) {
        this.showError(`Could not create the terminal session: ${errorMessage(error)}`);
        return;
      }
      sessionId = session.id;
      this.deps.registry.register(this.deps.workspaceId, sessionId);
    }

    this.sessionId = sessionId;
    if (this.disposed) {
      this.requestBackendTeardown();
      return;
    }

    let view: TerminalViewHandle;
    try {
      view = await this.deps.createView(this.deps.host, {
        onData: (data) => this.handleViewData(data),
        onCtrlC: () => this.requestCtrlC(),
      });
    } catch (error: unknown) {
      this.showError(`Could not open the terminal view: ${errorMessage(error)}`);
      this.requestBackendTeardown();
      return;
    }

    if (this.disposed) {
      view.dispose();
      return;
    }
    this.view = view;

    let channel: TerminalChannel;
    try {
      channel = this.deps.createChannel((event) => this.handleEvent(event));
    } catch (error: unknown) {
      this.showError(`Could not open the terminal stream: ${errorMessage(error)}`);
      this.disposeViewAndChannel();
      return;
    }
    this.channel = channel;
    this.applyingSnapshot = true;
    this.snapshotReceived = false;

    let attachFailed = false;
    let attachError: unknown;
    this.attachPending = true;
    try {
      await this.deps.invoke<void>("session_attach", {
        id: sessionId,
        // A new xterm host needs the retained scrollback replayed in full.
        // The registry cursor is bookkeeping for a future resume path.
        from_cursor: null,
        ch: channel,
      });
    } catch (error: unknown) {
      attachFailed = true;
      attachError = error;
    } finally {
      this.attachPending = false;
      this.requestBackendTeardown();
    }

    if (attachFailed) {
      this.clearSnapshotState();
      this.disposeViewAndChannel();
      if (adopted && isMissingSessionError(attachError) && !this.disposed) {
        this.resetSessionIdentity();
        this.deps.registry.remove(this.deps.workspaceId, sessionId);
        this.sessionId = null;
        this.started = false;
        await this.start();
        return;
      }
      this.showError(`Could not attach to the terminal: ${errorMessage(attachError)}`);
      return;
    }

    if (this.disposed || this.exited) return;
    // The host may have just become visible. Let ResizeObserver/layout settle
    // before fitting; doResize also ignores zero-sized hosts defensively.
    this.requestResize();
  }

  /** The first Ctrl+C arms; a second press within three seconds sends ETX. */
  requestCtrlC(): void {
    if (this.disposed || this.exited) return;

    if (this.ctrlCArmed) {
      this.disarmCtrlC();
      void this.writeToPty("\x03");
      return;
    }

    this.ctrlCArmed = true;
    this.deps.onCtrlCArmed(true);
    this.ctrlCTimer = this.setTimer(() => {
      this.ctrlCTimer = null;
      this.disarmCtrlC();
    }, CTRL_C_ARM_MS);
  }

  /** Schedule one backend resize after a burst of host-size changes. */
  requestResize(): void {
    if (this.disposed) return;
    if (this.resizeTimer !== null) this.clearTimer(this.resizeTimer);
    this.resizeTimer = this.setTimer(() => {
      this.resizeTimer = null;
      this.doResize();
    }, RESIZE_DEBOUNCE_MS);
  }

  /**
   * Public for the headless tests and for xterm's onData path. A failed write
   * is reported after two failures; it is intentionally not retried blindly.
   * A session_send may have flushed bytes before reporting a flush error, so an
   * automatic retry could duplicate user input. This gives failed writes a
   * safe failure path instead of a tight retry loop.
   */
  async writeToPty(data: string): Promise<void> {
    if (this.disposed || this.exited || this.sessionId === null) return;
    if (this.applyingSnapshot) {
      this.pendingInput.push(data);
      return;
    }
    await this.sendToPty(data);
  }

  private async sendToPty(data: string): Promise<void> {
    if (this.disposed || this.exited || this.sessionId === null) return;
    const sessionId = this.sessionId;
    try {
      await this.deps.invoke<void>("session_send", { id: sessionId, text: data });
      this.writeFailCount = 0;
    } catch {
      if (this.disposed) return;
      this.writeFailCount += 1;
      if (this.writeFailCount >= WRITE_FAIL_THRESHOLD) {
        this.showError("Could not send input to the terminal.");
      }
    }
  }

  /** Detach the view, channel, and backend subscription, leaving the session alive. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;

    this.disposeLocal();
    this.backendTeardown = "detach";
    this.requestBackendTeardown();
  }

  /** Explicitly close the runtime-owned session and detach this view. */
  close(): void {
    if (this.disposed) return;
    this.disposed = true;

    this.disposeLocal();
    this.backendTeardown = "close";
    this.requestBackendTeardown();
  }

  private disposeLocal(): void {
    if (this.ctrlCTimer !== null) {
      this.clearTimer(this.ctrlCTimer);
      this.ctrlCTimer = null;
    }
    this.ctrlCArmed = false;

    if (this.resizeTimer !== null) {
      this.clearTimer(this.resizeTimer);
      this.resizeTimer = null;
    }

    if (this.outputFrame !== null) {
      this.cancelFrame(this.outputFrame);
      this.outputFrame = null;
    }
    this.pendingOutput.length = 0;
    this.clearSnapshotState();
    this.disposeViewAndChannel();
  }

  private handleViewData(data: string): void {
    void this.writeToPty(data);
  }

  private handleEvent(event: TerminalEvent): void {
    if (this.disposed) return;
    if (this.applyingSnapshot && event.type !== "snapshot") {
      this.pendingSnapshotEvents.push(event);
      return;
    }
    if (this.exited) return;

    if (event.type === "snapshot") {
      this.beginSnapshot(event);
      return;
    }

    this.processEvent(event);
  }

  private processEvent(event: Exclude<TerminalEvent, SessionSnapshot>): void {
    if (this.disposed || this.exited) return;

    switch (event.type) {
      case "exit":
        this.markExited(event.code);
        break;
      case "recovered":
        this.markRecovered(event.integrity);
        break;
      case "silent":
        this.silenceBannerVisible = true;
        this.deps.onBanner({ kind: "silent", elapsedMs: event.elapsedMs });
        break;
      case "journal_degraded":
        this.markJournalDegraded(event.droppedFrames, event.droppedBytes);
        break;
      case "output":
        this.processOutput(event);
        break;
      case "agent_message":
      case "agent_tool_call":
      case "agent_tool_update":
      case "agent_finished":
      case "agent_error":
      case "agent_stderr":
        // ACP sessions use these same daemon channels; the terminal view has
        // no agent transcript renderer yet, so it safely ignores them.
        break;
      case "permission_request":
        this.deps.onPermissionRequest?.(event);
        break;
      default: {
        const unknownEvent: never = event;
        this.showError(
          `The daemon sent an unknown terminal event type: ${eventTypeName(unknownEvent)}.`,
        );
        return;
      }
    }
  }

  private processOutput(event: Extract<TerminalEvent, { type: "output" }>): void {
    if (this.snapshotAsOfSeq !== null && isSnapshotCoveredOutput(event.seq, this.snapshotAsOfSeq)) {
      return;
    }
    if (this.lastSeenSeq !== null && event.seq <= this.lastSeenSeq) return;
    this.lastSeenSeq = event.seq;
    if (this.silenceBannerVisible) {
      this.silenceBannerVisible = false;
      this.deps.onBanner(this.persistentBanner);
    }
    if (this.sessionId !== null) {
      this.deps.registry.updateCursor(this.deps.workspaceId, this.sessionId, event.seq);
    }
    this.pendingOutput.push(event);
    if (!this.applyingSnapshot) this.scheduleOutputFlush();
  }

  private beginSnapshot(snapshot: SessionSnapshot): void {
    if (this.snapshotReceived || this.view === null) return;
    this.snapshotReceived = true;
    const view = this.view;
    view.applySnapshot(snapshot, () => this.finishSnapshotWrite(snapshot));
  }

  /**
   * This callback is reached only after the view's xterm write callback has
   * parsed the snapshot and its state restoration sequence. The sequence
   * boundary is therefore committed here, never when the event is received.
   */
  private finishSnapshotWrite(snapshot: SessionSnapshot): void {
    if (this.disposed || this.view === null) return;

    this.snapshotAsOfSeq = snapshot.asOfSeq;
    this.lastSeenSeq = Math.max(this.lastSeenSeq ?? snapshot.asOfSeq, snapshot.asOfSeq);
    if (this.sessionId !== null) {
      this.deps.registry.updateCursor(this.deps.workspaceId, this.sessionId, snapshot.asOfSeq);
    }
    this.releaseSnapshotEvents(snapshot.asOfSeq);
  }

  /** Release only post-snapshot output, waiting for each xterm write. */
  private releaseSnapshotEvents(asOfSeq: number): void {
    if (this.disposed || this.view === null) return;

    const queuedEvents = this.pendingSnapshotEvents.splice(0);
    for (const event of queuedEvents) {
      if (event.type === "snapshot") continue;
      if (event.type === "output" && isSnapshotCoveredOutput(event.seq, asOfSeq)) continue;
      this.processEvent(event);
    }

    if (this.pendingOutput.length > 0) {
      const output = this.pendingOutput
        .splice(0)
        .map((event) => event.data)
        .join("");
      this.view.write(output, () => this.releaseSnapshotEvents(asOfSeq));
      return;
    }

    this.applyingSnapshot = false;
    this.releasePendingInput();
    this.requestResize();
  }

  private releasePendingInput(): void {
    const input = this.pendingInput.splice(0);
    for (const data of input) void this.sendToPty(data);
  }

  private clearSnapshotState(): void {
    this.applyingSnapshot = false;
    this.snapshotReceived = false;
    this.snapshotAsOfSeq = null;
    this.pendingSnapshotEvents.length = 0;
    this.pendingInput.length = 0;
  }

  /** Reset sequence state before this controller adopts a different session identity. */
  private resetSessionIdentity(): void {
    this.lastSeenSeq = null;
    this.clearSnapshotState();
    this.journalLoss = null;
  }

  private scheduleOutputFlush(): void {
    if (this.outputFrame !== null || this.view === null) return;
    this.outputFrame = this.scheduleFrame(() => {
      this.outputFrame = null;
      const view = this.view;
      if (view === null || this.pendingOutput.length === 0) return;
      const output = this.pendingOutput
        .splice(0)
        .map((event) => event.data)
        .join("");
      view.write(output);
    });
  }

  private markExited(code: number | null): void {
    if (this.exited) return;
    this.exited = true;
    this.silenceBannerVisible = false;
    const sessionId = this.sessionId;
    if (sessionId !== null) this.deps.registry.remove(this.deps.workspaceId, sessionId);
    this.sessionId = null;
    this.persistentBanner = {
      kind: "exited",
      code,
      lost: this.journalLoss,
      trimmedBytes: 0,
    };
    this.deps.onBanner(this.persistentBanner);
    this.deps.onExited?.(code);
  }

  private markRecovered(integrity: UnverifiableTranscriptIntegrity): void {
    if (this.exited) return;
    this.exited = true;
    this.silenceBannerVisible = false;
    const sessionId = this.sessionId;
    if (sessionId !== null) this.deps.registry.remove(this.deps.workspaceId, sessionId);
    this.sessionId = null;
    this.persistentBanner = { kind: "recovered", integrity };
    this.deps.onBanner(this.persistentBanner);
    this.deps.onExited?.(null);
  }

  private markJournalDegraded(frames: number, bytes: number): void {
    if (this.exited) return;
    this.silenceBannerVisible = false;
    const previous = this.journalLoss;
    this.journalLoss = {
      frames: Math.max(previous?.frames ?? 0, frames),
      bytes: Math.max(previous?.bytes ?? 0, bytes),
    };
    this.persistentBanner = { kind: "journal_degraded", lost: this.journalLoss };
    this.deps.onBanner(this.persistentBanner);
  }

  private disarmCtrlC(): void {
    if (this.ctrlCTimer !== null) {
      this.clearTimer(this.ctrlCTimer);
      this.ctrlCTimer = null;
    }
    if (!this.ctrlCArmed) return;
    this.ctrlCArmed = false;
    this.deps.onCtrlCArmed(false);
  }

  private doResize(): void {
    if (
      this.disposed ||
      this.exited ||
      this.applyingSnapshot ||
      this.sessionId === null ||
      this.view === null
    )
      return;
    const view = this.view;
    const fitted = view.fit();
    const cols = view.cols();
    const rows = view.rows();
    if (!fitted || cols <= 0 || rows <= 0) return;

    void this.deps
      .invoke<void>("session_resize", { id: this.sessionId, cols, rows })
      .catch(() => undefined);
  }

  private disposeViewAndChannel(): void {
    if (this.channel !== null) {
      this.channel.onmessage = () => undefined;
      this.channel = null;
    }
    if (this.view !== null) {
      this.view.dispose();
      this.view = null;
    }
  }

  private showError(message: string): void {
    if (!this.disposed) {
      this.silenceBannerVisible = false;
      this.persistentBanner = { kind: "error", message };
      this.deps.onBanner(this.persistentBanner);
    }
  }

  private requestBackendTeardown(): void {
    if (
      this.backendTeardown === null ||
      this.backendTeardownSent ||
      this.attachPending ||
      this.sessionId === null
    ) {
      return;
    }

    const sessionId = this.sessionId;
    const teardown = this.backendTeardown;
    this.backendTeardownSent = true;
    if (teardown === "close") {
      this.deps.registry.remove(this.deps.workspaceId, sessionId);
      void this.deps.invoke<void>("session_close", { id: sessionId }).catch(() => undefined);
      return;
    }

    void this.deps.invoke<void>("session_detach", { id: sessionId }).catch(() => undefined);
  }
}

function isMissingSessionError(error: unknown): boolean {
  if (isCommandError(error)) return error.code === "session_not_found";
  return errorMessage(error).toLowerCase().includes("no session with that id");
}

function pickRestorable(sessions: Session[], workspaceId: string | null): Session | null {
  const same = sessions.filter(
    (session) => session.kind === "terminal" && (session.workspaceId ?? null) === workspaceId,
  );
  const live = same.filter((session) => session.state.type === "live");
  if (live.length > 0) return live[live.length - 1] ?? null;
  const recovered = same.filter((session) => session.state.type === "recovered");
  if (recovered.length > 0) return recovered[recovered.length - 1] ?? null;
  return null;
}
