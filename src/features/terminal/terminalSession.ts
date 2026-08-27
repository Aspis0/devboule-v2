import type { Channel } from '@tauri-apps/api/core';
import type { Session, SessionEvent } from '../../types/ipc';
import type { TerminalViewHandle } from './createTerminalView';
import type { TerminalSessionRegistry } from './terminalRegistry';

export type TerminalEvent = SessionEvent;
export type TerminalChannel = Channel<TerminalEvent>;

export type TerminalBanner =
  | { kind: 'exited'; code: number | null }
  | { kind: 'closed' }
  | { kind: 'error'; message: string }
  | null;

export interface TerminalSessionDeps {
  workspaceId: string | null;
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
  setTimeout?: (callback: () => void, milliseconds: number) => number;
  clearTimeout?: (id: number) => void;
  scheduleFrame?: (callback: () => void) => number;
  cancelFrame?: (id: number) => void;
}

const RESIZE_DEBOUNCE_MS = 150;
const CTRL_C_ARM_MS = 3000;
const WRITE_FAIL_THRESHOLD = 2;

function errorMessage(error: unknown): string {
  if (typeof error === 'string' && error.trim()) return error;
  if (error instanceof Error && error.message) return error.message;
  return 'Unknown terminal error.';
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
  private backendTeardown: 'detach' | 'close' | null = null;
  private backendTeardownSent = false;

  private readonly pendingOutput: string[] = [];
  private outputFrame: number | null = null;
  private ctrlCArmed = false;
  private ctrlCTimer: number | null = null;
  private resizeTimer: number | null = null;
  private writeFailCount = 0;

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

    const existing = this.deps.registry.get(this.deps.workspaceId);
    const adopted = existing !== null;
    let sessionId = existing?.sessionId ?? null;

    if (sessionId === null) {
      let session: Session;
      try {
        session = await this.deps.invoke<Session>('session_create', {
          workspace_id: this.deps.workspaceId,
          kind: 'terminal',
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

    let attachFailed = false;
    let attachError: unknown;
    this.attachPending = true;
    try {
      await this.deps.invoke<void>('session_attach', {
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
      this.disposeViewAndChannel();
      if (adopted && isMissingSessionError(attachError) && !this.disposed) {
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
      void this.writeToPty('\x03');
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
    const sessionId = this.sessionId;
    try {
      await this.deps.invoke<void>('session_send', { id: sessionId, text: data });
      this.writeFailCount = 0;
    } catch {
      if (this.disposed) return;
      this.writeFailCount += 1;
      if (this.writeFailCount >= WRITE_FAIL_THRESHOLD) {
        this.showError('Could not send input to the terminal.');
      }
    }
  }

  /** Detach the view, channel, and backend subscription, leaving the session alive. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;

    this.disposeLocal();
    this.backendTeardown = 'detach';
    this.requestBackendTeardown();
  }

  /** Explicitly close the runtime-owned session and detach this view. */
  close(): void {
    if (this.disposed) return;
    this.disposed = true;

    this.disposeLocal();
    this.backendTeardown = 'close';
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
    this.disposeViewAndChannel();
  }

  private handleViewData(data: string): void {
    void this.writeToPty(data);
  }

  private handleEvent(event: TerminalEvent): void {
    if (this.disposed) return;

    if (event.type === 'exit') {
      this.markExited(event.code);
      return;
    }

    if (this.lastSeenSeq !== null && event.seq <= this.lastSeenSeq) return;
    this.lastSeenSeq = event.seq;
    if (this.sessionId !== null) {
      this.deps.registry.updateCursor(this.deps.workspaceId, this.sessionId, event.seq);
    }
    this.pendingOutput.push(event.data);
    this.scheduleOutputFlush();
  }

  private scheduleOutputFlush(): void {
    if (this.outputFrame !== null || this.view === null) return;
    this.outputFrame = this.scheduleFrame(() => {
      this.outputFrame = null;
      const view = this.view;
      if (view === null || this.pendingOutput.length === 0) return;
      const output = this.pendingOutput.splice(0).join('');
      view.write(output);
    });
  }

  private markExited(code: number | null): void {
    if (this.exited) return;
    this.exited = true;
    const sessionId = this.sessionId;
    if (sessionId !== null) this.deps.registry.remove(this.deps.workspaceId, sessionId);
    this.sessionId = null;
    this.deps.onBanner({ kind: 'exited', code });
    this.deps.onExited?.(code);
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
    if (this.disposed || this.exited || this.sessionId === null || this.view === null) return;
    const view = this.view;
    const fitted = view.fit();
    const cols = view.cols();
    const rows = view.rows();
    if (!fitted || cols <= 0 || rows <= 0) return;

    void this.deps
      .invoke<void>('session_resize', { id: this.sessionId, cols, rows })
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
    if (!this.disposed) this.deps.onBanner({ kind: 'error', message });
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
    if (teardown === 'close') {
      this.deps.registry.remove(this.deps.workspaceId, sessionId);
      void this.deps.invoke<void>('session_close', { id: sessionId }).catch(() => undefined);
      return;
    }

    void this.deps.invoke<void>('session_detach', { id: sessionId }).catch(() => undefined);
  }
}

function isMissingSessionError(error: unknown): boolean {
  return errorMessage(error).toLowerCase().includes('no session with that id');
}
