/**
 * One Chrome DevTools Protocol client for the app's WebView2 window.
 *
 * WebView2 speaks CDP when the app is started with
 * `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`. Two
 * callers drive it — `scripts/webview-probe.mjs` by hand and
 * `scripts/e2e/smoke.mjs` in CI — so the target lookup, the request/answer
 * pairing and the event stream live here once. No dependencies: Node has had a
 * global WebSocket since 22.
 */

/**
 * Find the page target serving the app.
 *
 * The debug port lists every page the browser process owns, so the target is
 * matched rather than assumed to be first. Cross-origin plugin frames surface
 * as their own targets, typed `page` or `iframe` depending on the WebView2
 * build; the URL match is what picks the document. Returns null when nothing
 * answers before the deadline — the app may still be starting, so callers
 * decide whether that is a failure.
 */
export async function findPageTarget({ port, urlMatches, timeoutMs }) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`);
      const targets = await response.json();
      const page = targets.find(
        (target) =>
          (target.type === "page" || target.type === "iframe") &&
          target.webSocketDebuggerUrl &&
          urlMatches.some((match) => String(target.url).includes(match)),
      );
      if (page) return page;
    } catch {
      // The port is not open until the WebView is created; keep waiting.
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  return null;
}

/**
 * One WebSocket to one target: commands are answered by id, events are handed
 * to whoever registered for their method.
 */
export class CdpSession {
  #socket;
  #nextId = 1;
  #pending = new Map();
  #listeners = new Map();

  constructor(socket) {
    this.#socket = socket;
    socket.addEventListener("message", (event) => this.#dispatch(JSON.parse(event.data)));
    socket.addEventListener("close", () => this.#abandon("the WebView closed the connection"));
    socket.addEventListener("error", () => this.#abandon("the WebView connection failed"));
  }

  static open(webSocketDebuggerUrl) {
    return new Promise((resolve, reject) => {
      const socket = new WebSocket(webSocketDebuggerUrl);
      socket.addEventListener("open", () => resolve(new CdpSession(socket)), { once: true });
      socket.addEventListener(
        "error",
        () => reject(new Error(`could not open ${webSocketDebuggerUrl}`)),
        {
          once: true,
        },
      );
    });
  }

  /** Subscribe to a CDP event by method name. */
  on(method, handler) {
    const handlers = this.#listeners.get(method) ?? new Set();
    handlers.add(handler);
    this.#listeners.set(method, handlers);
  }

  /** Send one command and resolve with its result. */
  send(method, params = {}, timeoutMs = 30_000) {
    return new Promise((resolve, reject) => {
      const id = this.#nextId++;
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`the WebView did not answer within ${Math.round(timeoutMs / 1000)}s`));
      }, timeoutMs);
      this.#pending.set(id, { resolve, reject, timer, method });
      this.#socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.#socket.close();
    this.#abandon("the WebView connection was closed");
  }

  #dispatch(message) {
    if (message.id !== undefined) {
      const pending = this.#pending.get(message.id);
      if (pending === undefined) return;
      clearTimeout(pending.timer);
      this.#pending.delete(message.id);
      if (message.error) pending.reject(new Error(`${pending.method}: ${message.error.message}`));
      else pending.resolve(message.result);
      return;
    }
    if (message.method === undefined) return;
    for (const handler of this.#listeners.get(message.method) ?? []) handler(message.params);
  }

  #abandon(reason) {
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error(reason));
    }
    this.#pending.clear();
  }
}

/**
 * Evaluate an expression in the page and return its value.
 *
 * `awaitPromise` is on because callers hand in async work; `userGesture` is on
 * because probes call app code, which is not a user gesture, and without it
 * some APIs refuse in ways that look like feature absence.
 */
export function evaluate(session, expression, timeoutMs = 30_000) {
  return session
    .send(
      "Runtime.evaluate",
      { expression, awaitPromise: true, returnByValue: true, userGesture: true },
      timeoutMs,
    )
    .then(({ result, exceptionDetails }) => {
      if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? "threw");
      return result.value;
    });
}

/**
 * Capture the window as a PNG. Two commands in order, not one: a
 * `Page.captureScreenshot` on a domain that was never enabled answers with an
 * error instead of a picture, and the error looks like a broken port.
 */
export async function captureScreenshot(session, timeoutMs = 30_000) {
  await session.send("Page.enable", {}, timeoutMs);
  const { data } = await session.send("Page.captureScreenshot", { format: "png" }, timeoutMs);
  return Buffer.from(data, "base64");
}
