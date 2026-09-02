import "pixi.js/unsafe-eval";
import { Application } from "pixi.js";
import { loadPolisArt } from "./artAssets";
import fixtureCity from "./fixture-city.json";
import {
  CITY_FETCH_TIMEOUT_MS,
  cityHudLabel,
  cityDegradationSuffix,
  formatCityFetchReadout,
  formatBackendFailureReadout,
  formatHandshakeReadout,
  formatWorkspaceRootReadout,
  invokeHost,
  loadCity,
  pendingCityState,
  SESSIONS_WATCH_TIMEOUT_MS,
  subscribeSessions,
} from "./hostBridge";
import { CityRenderer } from "./renderer";
import { renderAgentRoster } from "./roster";
import type { City, CityFile } from "./model";

const canvas = getElement<HTMLCanvasElement>("scene");
const webglReadout = getElement<HTMLElement>("webgl");
const tauriReadout = getElement<HTMLElement>("tauri");
const bridgeReadout = getElement<HTMLElement>("bridge");
const backendReadout = getElement<HTMLElement>("backend");
const cityReadout = getElement<HTMLElement>("city");
const agentReadout = getElement<HTMLElement>("agents");
const rosterReadout = getElement<HTMLElement>("roster");
const findingReadout = getElement<HTMLElement>("findings");
const detailsReadout = getElement<HTMLElement>("details");
const hudToggle = getElement<HTMLButtonElement>("hud-toggle");
const hudDetails = getElement<HTMLDivElement>("hud-details");
const fixture = fixtureCity as City;
const TAURI_PROBE_TIMEOUT_MS = 4000;

const cityPending = pendingCityState();
if (cityPending.status === "pending") renderPendingCity();
bindHudToggle();
const isolationMeasurement = measureTauriIsolation();
void isolationMeasurement.then((isolation) => reportIsolationOutcome(isolation));
const cityLoad = loadCity(invokeHost, fixture, CITY_FETCH_TIMEOUT_MS);
void reportBackend(cityLoad);
void startRenderer(cityLoad);

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

async function startRenderer(cityLoadResult: ReturnType<typeof loadCity>): Promise<void> {
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

  const bank = await loadPolisArt();
  const loadedCity = await cityLoadResult;
  let currentAgents = loadedCity.city.agents;
  let renderer: CityRenderer | null = null;
  let subscription: Awaited<ReturnType<typeof subscribeSessions>> | null = null;

  if (loadedCity.status === "host") {
    try {
      subscription = await subscribeSessions(
        invokeHost,
        (agents) => {
          currentAgents = agents;
          const nextCity = { ...loadedCity.city, agents };
          renderCityStats(nextCity);
          renderer?.refreshAgents(agents);
        },
        SESSIONS_WATCH_TIMEOUT_MS,
      );
    } catch (error) {
      renderSessionWatchFailure(error);
    }
  }

  const cityForRenderer = { ...loadedCity.city, agents: currentAgents };
  renderCityStats(cityForRenderer);

  renderer = new CityRenderer({
    app,
    canvas,
    city: cityForRenderer,
    details: {
      setDetails: (file) => showFileDetails(file),
      clearDetails: () => clearFileDetails(),
    },
    bank,
  });

  if (subscription !== null) {
    window.addEventListener("pagehide", () => subscription?.close(), { once: true });
  }
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

async function reportBackend(cityLoadResult: ReturnType<typeof loadCity>): Promise<void> {
  const loadedCity = await cityLoadResult;
  try {
    const value = await invokeHost("workspace.root");
    bridgeReadout.textContent = formatWorkspaceRootReadout(value);
    backendReadout.textContent = `${formatHandshakeReadout(value)} · ${formatCityFetchReadout(loadedCity)}`;
  } catch (error) {
    const backendFailure = formatBackendFailureReadout(error);
    bridgeReadout.textContent = `Bridge reply: ${backendFailure.replace(/^Backend: /, "")}`;
    backendReadout.textContent = `${backendFailure} · ${formatCityFetchReadout(loadedCity)}`;
  }
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
  const knownFileIds = new Set(value.files.map((file) => file.id));
  const placedAgents = value.agents.filter(
    (agent) => agent.fileId !== null && knownFileIds.has(agent.fileId),
  ).length;
  const rosterAgents = value.agents.length - placedAgents;
  const placedFindings = value.findings.filter((finding) =>
    knownFileIds.has(finding.fileId),
  ).length;
  const source = cityHudLabel(value);
  cityReadout.textContent = `${source} · ${value.files.length} files${cityDegradationSuffix(value)} · ${value.imports.length} directed roads`;
  agentReadout.textContent = `${value.dataSource === "host" ? "Agents host" : "Agents fixture"} · ${placedAgents} on buildings · ${rosterAgents} roster-only`;
  if (value.dataSource === "host") {
    renderAgentRoster(rosterReadout, value.agents, knownFileIds);
  } else {
    rosterReadout.textContent =
      rosterAgents === 0
        ? "Roster: empty"
        : `Roster: ${rosterAgents} session${rosterAgents === 1 ? "" : "s"} without a touched file (not drawn)`;
  }
  findingReadout.textContent = `Findings fixture · ${placedFindings} open · smoke / fire / inferno`;
}

function renderSessionWatchFailure(error: unknown): void {
  const code = errorCode(error);
  const state =
    code === "timeout" ? "timed out" : code === "malformed_sessions" ? "malformed" : "refused";
  rosterReadout.textContent = `Roster: live session feed ${state} — ${errorMessage(error)}`;
}

function renderPendingCity(): void {
  cityReadout.textContent = "City: measuring host city…";
  agentReadout.textContent = "Agents: host data pending";
  rosterReadout.textContent = "Roster: host data pending";
  findingReadout.textContent = "Findings: host data pending";
}

function showFileDetails(file: CityFile): void {
  detailsReadout.textContent = `${file.path} · ${file.lines.toLocaleString()} lines · district ${file.district}`;
}

function clearFileDetails(): void {
  detailsReadout.textContent = "Hover a building to inspect its file";
}

function bindHudToggle(): void {
  const setExpanded = (expanded: boolean): void => {
    hudDetails.hidden = !expanded;
    hudToggle.setAttribute("aria-expanded", String(expanded));
    hudToggle.textContent = expanded ? "Hide facts" : "Show facts";
  };

  setExpanded(false);
  hudToggle.addEventListener("click", () => setExpanded(hudDetails.hasAttribute("hidden")));
}

function getElement<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (element === null) throw new Error(`Missing #${id}`);
  return element as T;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "unknown error";
}

function errorCode(error: unknown): string | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) return undefined;
  const code = (error as { code?: unknown }).code;
  return typeof code === "string" ? code : undefined;
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
