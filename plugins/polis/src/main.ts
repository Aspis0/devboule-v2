import "pixi.js/unsafe-eval";
import { Application } from "pixi.js";
import fixtureCity from "./fixture-city.json";
import { CityRenderer } from "./renderer";
import type { City, CityFile } from "./model";

const canvas = getElement<HTMLCanvasElement>("scene");
const webglReadout = getElement<HTMLElement>("webgl");
const tauriReadout = getElement<HTMLElement>("tauri");
const bridgeReadout = getElement<HTMLElement>("bridge");
const cityReadout = getElement<HTMLElement>("city");
const detailsReadout = getElement<HTMLElement>("details");
const city = fixtureCity as City;
const TAURI_PROBE_TIMEOUT_MS = 4000;

renderCityStats(city);
const isolationMeasurement = measureTauriIsolation();
void isolationMeasurement.then((isolation) => reportIsolationOutcome(isolation));
requestBridgeProbe();
void startRenderer();

type IsolationOutcome =
  | { status: "object-absent" }
  | { status: "present-call-refused"; message: string }
  | { status: "present-call-timed-out"; message: string }
  | { status: "call-succeeded"; value: unknown };

type WindowWithTauriInternals = Window & {
  __TAURI_INTERNALS__?: {
    invoke(command: string, args?: unknown): Promise<unknown>;
  };
};

async function startRenderer(): Promise<void> {
  const probe = document.createElement("canvas");
  const gl = probe.getContext("webgl2", { antialias: true });
  if (gl === null) {
    webglReadout.textContent = "WebGL2: unavailable — no WebGL2 context was created";
  } else {
    const debugInfo = gl.getExtension("WEBGL_debug_renderer_info");
    const renderer = debugInfo
      ? gl.getParameter(debugInfo.UNMASKED_RENDERER_WEBGL)
      : "debug extension unavailable";
    webglReadout.textContent = `WebGL2: available | renderer: ${renderer}`;
  }

  const app = new Application();
  try {
    await app.init({
      canvas,
      preference: "webgl",
      antialias: true,
      backgroundAlpha: 0,
      resizeTo: window,
      resolution: Math.min(window.devicePixelRatio || 1, 2),
      autoDensity: true,
    });
  } catch (error) {
    webglReadout.textContent = `WebGL2: renderer failed to start — ${errorMessage(error)}`;
    return;
  }

  new CityRenderer({
    app,
    canvas,
    city,
    details: {
      setDetails: (file) => showFileDetails(file),
      clearDetails: () => clearFileDetails(),
    },
  });
}

async function measureTauriIsolation(): Promise<IsolationOutcome> {
  const internals = (window as WindowWithTauriInternals).__TAURI_INTERNALS__;
  if (internals === undefined) {
    const outcome: IsolationOutcome = { status: "object-absent" };
    renderIsolationOutcome(outcome);
    return outcome;
  }

  try {
    // The object is only a symptom. This real registered command tells us
    // whether the plugin can cross the IPC boundary that actually matters.
    const value = await invokePluginsListWithTimeout(internals);
    const outcome: IsolationOutcome = { status: "call-succeeded", value: serializableValue(value) };
    renderIsolationOutcome(outcome);
    return outcome;
  } catch (error) {
    if (isProbeTimeout(error)) {
      const outcome: IsolationOutcome = {
        status: "present-call-timed-out",
        message: errorMessage(error),
      };
      renderIsolationOutcome(outcome);
      return outcome;
    }
    const outcome: IsolationOutcome = {
      status: "present-call-refused",
      message: errorMessage(error),
    };
    renderIsolationOutcome(outcome);
    return outcome;
  }
}

function renderIsolationOutcome(outcome: IsolationOutcome): void {
  tauriReadout.classList.remove("prominent");
  if (outcome.status === "object-absent") {
    tauriReadout.textContent = "Tauri IPC: object is absent — isolated";
  } else if (outcome.status === "present-call-refused") {
    tauriReadout.textContent = `Tauri IPC: object is present but the call was refused or threw — isolated in the way that matters — ${outcome.message}`;
  } else if (outcome.status === "present-call-timed-out") {
    tauriReadout.textContent = `Tauri IPC: object is present, call never answered within ${TAURI_PROBE_TIMEOUT_MS / 1000} seconds — isolated; the call hangs rather than failing`;
  } else {
    tauriReadout.classList.add("prominent");
    tauriReadout.textContent = `Tauri IPC: call SUCCEEDED and returned data — NOT isolated — ${formatValue(outcome.value)}`;
  }
}

function invokePluginsListWithTimeout(
  internals: NonNullable<WindowWithTauriInternals["__TAURI_INTERNALS__"]>,
): Promise<unknown> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = window.setTimeout(() => {
      settled = true;
      const error = new Error(
        `plugins_list did not answer within ${TAURI_PROBE_TIMEOUT_MS / 1000} seconds; the call hangs rather than failing`,
      );
      error.name = "TauriProbeTimeoutError";
      reject(error);
    }, TAURI_PROBE_TIMEOUT_MS);

    Promise.resolve()
      .then(() => internals.invoke("plugins_list"))
      .then(
        (value) => {
          if (settled) return;
          settled = true;
          window.clearTimeout(timer);
          resolve(value);
        },
        (error: unknown) => {
          if (settled) return;
          settled = true;
          window.clearTimeout(timer);
          reject(error);
        },
      );
  });
}

function isProbeTimeout(error: unknown): boolean {
  return error instanceof Error && error.name === "TauriProbeTimeoutError";
}

function requestBridgeProbe(): void {
  const requestId = `polis-${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`}`;
  let replyReceived = false;
  window.addEventListener("message", (event) => {
    if (replyReceived || event.source !== window.parent) return;
    const message = event.data;
    if (
      message === null ||
      typeof message !== "object" ||
      message.v !== 1 ||
      message.id !== requestId ||
      (message.kind !== "error" && message.kind !== "result")
    ) {
      return;
    }

    replyReceived = true;
    if (message.kind === "error") {
      const expected =
        message.message === 'The host cannot route plugin method "oracle.search" yet';
      bridgeReadout.textContent = expected
        ? `Bridge reply: refusal (expected) — ${message.message}`
        : `Bridge reply: error — ${message.message}`;
    } else {
      bridgeReadout.textContent = `Bridge reply: result — ${JSON.stringify(message.value)}`;
    }
  });

  window.parent.postMessage(
    {
      v: 1,
      id: requestId,
      kind: "invoke",
      method: "oracle.search",
      payload: { isolation: { status: "measurement-pending" } },
    },
    "*",
  );
  window.setTimeout(() => {
    if (!replyReceived)
      bridgeReadout.textContent = "Bridge reply: no reply (silence after 4 seconds)";
  }, 4000);
}

function reportIsolationOutcome(isolation: IsolationOutcome): void {
  const requestId = `polis-isolation-${globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`}`;
  window.parent.postMessage(
    {
      v: 1,
      id: requestId,
      kind: "invoke",
      method: "oracle.search",
      payload: { isolation },
    },
    "*",
  );
}

function renderCityStats(value: City): void {
  cityReadout.textContent = `Fixture city · ${value.files.length} files · ${value.imports.length} directed roads`;
}

function showFileDetails(file: CityFile): void {
  detailsReadout.textContent = `${file.path} · ${file.lines.toLocaleString()} lines · district ${file.district}`;
}

function clearFileDetails(): void {
  detailsReadout.textContent = "Hover a building to inspect its file";
}

function getElement<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (element === null) throw new Error(`Missing #${id}`);
  return element as T;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "unknown error";
}

function serializableValue(value: unknown): unknown {
  try {
    return JSON.parse(JSON.stringify(value)) as unknown;
  } catch {
    return String(value);
  }
}

function formatValue(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}
