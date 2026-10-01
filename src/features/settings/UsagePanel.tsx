import { useEffect, useState } from "react";
import { ErrorText } from "../../components/ErrorText";
import { providerEmptySentence } from "../../lib/providerEmptySentence";
import { usePlanUsage } from "../../lib/planUsageStore";
import { useTrackedRequest } from "../../lib/trackedRequest";
import { providersList } from "../../lib/tauri";
import type { PlanUsage, PlanWindow, ProviderCatalog, ProviderInfo } from "../../types/ipc";
import {
  planCreditsCopy,
  planFrameHasContent,
  planWindowBarPercent,
  planWindowKey,
  planWindowLabel,
  planWindowMeta,
} from "../workspace/contextUsageView";

const LOADING_COPY = "Listing the configured providers…";

/**
 * Plan usage travels for the daemon's Claude and Codex roads only: its plan
 * cache maps the Claude and Codex session kinds to a frame and Pi, Acp and
 * Terminal to none (crates/devboule-daemon/src/plan_usage_cache.rs; frames
 * are built in claude_view_rate_limit.rs and codex_view.rs). The catalog's
 * `protocol` names the road a provider launches chat on — one of "acp",
 * "stream-json", "pi-rpc" or "codex-app-server" — so that field is the whole
 * fact here, never a second, hand-kept list of provider ids.
 */
function reportsPlanUsage(provider: ProviderInfo): boolean {
  return provider.protocol === "stream-json" || provider.protocol === "codex-app-server";
}

/** The quiet line under a provider heading with no reading stored. A
    protocol the catalog omitted (an older daemon sends no field) is no
    answer: it takes the neutral line, never a claim about what the provider
    can report. */
function noReadingCopy(provider: ProviderInfo): string {
  if (provider.protocol != null && !reportsPlanUsage(provider)) {
    return "This provider does not report plan usage.";
  }
  if (provider.enabled === false) {
    return "No plan reading yet — the provider is switched off, so no session can send one.";
  }
  return "No plan reading yet — one appears here when a session of this provider sends it.";
}

/** One window of a plan reading: label, percent and reset, then the bar. A
    field the frame did not carry renders nothing — never a stand-in zero. */
function PlanWindowRow({ planWindow, nowMs }: { planWindow: PlanWindow; nowMs: number }) {
  const meta = planWindowMeta(planWindow, nowMs);
  const barPercent = planWindowBarPercent(planWindow);
  return (
    <div className="plan-window">
      <div className="settings-card settings-value-row">
        <span className="settings-card-title">{planWindowLabel(planWindow.durationMins)}</span>
        {meta !== null ? <span className="settings-card-value">{meta}</span> : null}
      </div>
      {barPercent !== null ? (
        <div className="plan-window-bar" aria-hidden="true">
          <div className="plan-window-fill" style={{ width: `${barPercent}%` }} />
        </div>
      ) : null}
    </div>
  );
}

function PlanReadingRows({ plan, nowMs }: { plan: PlanUsage; nowMs: number }) {
  const credits = planCreditsCopy(plan.credits);
  return (
    <div className="settings-stack settings-stack-spaced">
      {plan.planLabel !== undefined ? (
        <div className="settings-card settings-value-row">
          <span className="settings-card-title">Plan</span>
          <span className="settings-card-value">{plan.planLabel}</span>
        </div>
      ) : null}
      {plan.windows.map((planWindow, index) => (
        <PlanWindowRow
          key={planWindowKey(index, planWindow)}
          planWindow={planWindow}
          nowMs={nowMs}
        />
      ))}
      {credits !== null ? (
        <div className="settings-card settings-value-row">
          <span className="settings-card-title">{credits.title}</span>
          <span className="settings-card-value">{credits.value}</span>
        </div>
      ) : null}
    </div>
  );
}

/** One provider's group: its name, then its latest plan reading or the quiet
    line that says why there is none. `usePlanUsage` re-renders only this
    group when the provider's next frame lands. */
function ProviderPlanGroup({ provider, nowMs }: { provider: ProviderInfo; nowMs: number }) {
  const plan = usePlanUsage(provider.id);
  return (
    <section aria-label={`Plan usage: ${provider.id}`}>
      <h3 className="settings-subheading">{provider.id}</h3>
      {plan === null || !planFrameHasContent(plan) ? (
        <p className="settings-page-empty">{noReadingCopy(provider)}</p>
      ) : (
        <PlanReadingRows plan={plan} nowMs={nowMs} />
      )}
    </section>
  );
}

/**
 * The page's readings are the plan store's live-only frame, never restored
 * from replay, so the panel stays empty until the first live reading arrives.
 */
export function UsagePanel() {
  // The reset labels count down, so the clock must move while the page sits
  // open — a render that never happens would freeze "resets in".
  const [nowMs, setNowMs] = useState(() => Date.now());
  const catalogRequest = useTrackedRequest<ProviderCatalog>(
    providersList,
    { status: "loading" },
    true,
  );
  const { state: catalogState, run: listProviders } = catalogRequest;

  useEffect(() => {
    const timer = globalThis.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => globalThis.clearInterval(timer);
  }, []);

  const installed =
    catalogState.status === "ready"
      ? catalogState.value.providers.filter((provider) => provider.installed !== false)
      : null;
  const unreadableDirs = catalogState.status === "ready" ? catalogState.value.unreadableDirs : 0;

  return (
    <div id="settings-panel-usage">
      {catalogState.status === "error" ? (
        <div role="alert">
          <ErrorText
            sentence={catalogState.message}
            detail={catalogState.detail}
            id="settings-usage-error"
          />
          <button type="button" className="settings-device-action" onClick={() => listProviders()}>
            Retry
          </button>
        </div>
      ) : installed === null ? (
        <div role="status">{LOADING_COPY}</div>
      ) : installed.length === 0 ? (
        <p className="settings-page-empty" role="status">
          {providerEmptySentence(unreadableDirs)}
        </p>
      ) : (
        installed.map((provider) => (
          <ProviderPlanGroup key={provider.id} provider={provider} nowMs={nowMs} />
        ))
      )}
    </div>
  );
}
