/**
 * The only channel a plugin frame gets to ask the host for work. The frame is
 * checked by both origin and identity because either check alone can admit a
 * message from a different document.
 */
export interface PluginBridgeOptions {
  iframe: HTMLIFrameElement;
  pluginId: string;
  pluginOrigin: string;
  capabilities: readonly string[];
  timeoutMs?: number;
  /** Forward a granted method to `plugin_invoke`. Absent: the host cannot route yet. */
  route?: (method: string, payload: unknown) => Promise<unknown>;
}

export interface PluginBridge {
  /** Send a request to the frame and reject if it never replies. */
  invoke(method: string, payload?: unknown): Promise<unknown>;
  dispose(): void;
}

interface InvokeMessage {
  v: 1;
  id: string;
  kind: "invoke";
  method: string;
  payload?: unknown;
}

type ReplyMessage =
  | { v: 1; id: string; kind: "result"; value: unknown }
  | { v: 1; id: string; kind: "error"; message: string; code?: string };

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

const DEFAULT_TIMEOUT_MS = 10_000;

/** Attach the host side of the frame's versioned, capability-limited channel. */
export function createPluginBridge(options: PluginBridgeOptions): PluginBridge {
  const pluginOrigin = options.pluginOrigin.replace(/\/+$/, "");
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const pending = new Map<string, PendingRequest>();
  let nextId = 0;
  let disposed = false;

  function post(reply: ReplyMessage): void {
    options.iframe.contentWindow?.postMessage(reply, pluginOrigin);
  }

  function handleMessage(event: MessageEvent<unknown>): void {
    // Both checks are required: an attacker can send from a trusted origin in
    // another window, and a frame can navigate to a different origin.
    if (event.origin !== pluginOrigin || event.source !== options.iframe.contentWindow) return;

    const message = parseMessage(event.data);
    if (message === null) return;

    if (message.kind === "invoke") {
      const capability = message.method;
      if (!options.capabilities.includes(capability)) {
        post({
          v: 1,
          id: message.id,
          kind: "error",
          message: `Plugin capability "${capability}" was not requested in the manifest`,
        });
        return;
      }

      if (options.route === undefined) {
        post({
          v: 1,
          id: message.id,
          kind: "error",
          message: `The host cannot route plugin method "${message.method}" yet`,
        });
        return;
      }

      const requestId = message.id;
      void options.route(message.method, message.payload).then(
        (value) => {
          if (disposed) return;
          post({ v: 1, id: requestId, kind: "result", value });
        },
        (error: unknown) => {
          if (disposed) return;
          const reply: Extract<ReplyMessage, { kind: "error" }> = {
            v: 1,
            id: requestId,
            kind: "error",
            message: routeErrorMessage(error),
          };
          const code = routeErrorCode(error);
          if (code !== undefined) reply.code = code;
          post(reply);
        },
      );
      return;
    }

    const request = pending.get(message.id);
    if (request === undefined) return;
    pending.delete(message.id);
    clearTimeout(request.timer);
    if (message.kind === "result") request.resolve(message.value);
    else request.reject(new Error(message.message));
  }

  window.addEventListener("message", handleMessage);

  return {
    invoke(method, payload) {
      if (disposed) return Promise.reject(new Error("Plugin bridge is disposed"));
      const frame = options.iframe.contentWindow;
      if (frame === null) return Promise.reject(new Error("Plugin frame is unavailable"));

      const id = `${options.pluginId}-${++nextId}`;
      const message: InvokeMessage = { v: 1, id, kind: "invoke", method };
      if (payload !== undefined) message.payload = payload;

      return new Promise<unknown>((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`Plugin request "${method}" timed out`));
        }, timeoutMs);
        pending.set(id, { resolve, reject, timer });
        try {
          frame.postMessage(message, pluginOrigin);
        } catch (error) {
          clearTimeout(timer);
          pending.delete(id);
          reject(error instanceof Error ? error : new Error("Plugin request could not be sent"));
        }
      });
    },

    dispose() {
      if (disposed) return;
      disposed = true;
      window.removeEventListener("message", handleMessage);
      for (const [id, request] of pending) {
        clearTimeout(request.timer);
        request.reject(new Error(`Plugin request "${id}" was cancelled`));
      }
      pending.clear();
    },
  };
}

function parseMessage(value: unknown): InvokeMessage | ReplyMessage | null {
  if (!isRecord(value) || value.v !== 1 || typeof value.id !== "string" || value.id === "") {
    return null;
  }

  if (value.kind === "invoke" && typeof value.method === "string" && value.method !== "") {
    return value as unknown as InvokeMessage;
  }
  if (value.kind === "result" && "value" in value) {
    return value as unknown as ReplyMessage;
  }
  if (
    value.kind === "error" &&
    typeof value.message === "string" &&
    (value.code === undefined || typeof value.code === "string")
  ) {
    return value as unknown as ReplyMessage;
  }
  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Tauri rejects with `{ code, message }`, not always an Error. */
function routeErrorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message: unknown }).message;
    if (typeof message === "string" && message !== "") return message;
  }
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return "plugin invoke failed";
}

function routeErrorCode(error: unknown): string | undefined {
  if (isRecord(error) && typeof error.code === "string" && error.code !== "") {
    return error.code;
  }
  if (error instanceof Error) {
    const code = (error as Error & { code?: unknown }).code;
    return typeof code === "string" && code !== "" ? code : undefined;
  }
  return undefined;
}
