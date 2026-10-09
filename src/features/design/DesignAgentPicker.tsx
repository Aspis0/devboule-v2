import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { DesignAgentSession } from "./designHost";
import type { AgentSessionState } from "../../lib/agentSession";
import type { ProviderInfo } from "../../types/ipc";
import { menuPlacement } from "../../components/PickerChip";
import { useMenuOpen } from "../../lib/menuOpen";
import { scrollRowIntoView } from "../../lib/scrollRowIntoView";
import { isImeComposition } from "../../lib/imeComposition";
import { useProviderConsent } from "../workspace/useProviderConsent";
import { requiresConsent } from "../workspace/workspaceSessions";
import { modelLabel as modelDisplayLabel } from "../workspace/modelLabel";
import { confirmedEffort, manifestModel } from "./designMessageModel";

interface DesignAgentPickerProps {
  providers: readonly ProviderInfo[];
  providersLoading: boolean;
  selectedProviderId: string | null;
  unavailableProviderId: string | null;
  busy: boolean;
  agentSession: DesignAgentSession | null;
  agentState: AgentSessionState | null;
  onProviderSelect: (provider: ProviderInfo) => void;
  onModelSelect: (modelId: string) => void;
  onEffortSelect: (effort: string) => void;
}

export function DesignAgentPicker({
  providers,
  providersLoading,
  selectedProviderId,
  unavailableProviderId,
  busy,
  agentSession,
  agentState,
  onProviderSelect,
  onModelSelect,
  onEffortSelect,
}: DesignAgentPickerProps) {
  const [providerPickerOpen, setProviderPickerOpen] = useState(false);
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const providerButtonRef = useRef<HTMLButtonElement>(null);
  const providerPickerWrapRef = useRef<HTMLDivElement>(null);
  const providerMenuRef = useRef<HTMLDivElement>(null);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  const consentRestoreProviderIdRef = useRef<string | null>(null);
  const manifest = agentState?.manifest ?? null;
  const currentModel = manifestModel(manifest);
  const sessionClosed = agentState?.status === "closed";
  const sessionErrored = agentState?.status === "error";
  const sessionUnavailable = sessionClosed || sessionErrored;
  // `fail()` in agentSession.ts records the message only as an error item in the
  // transcript; the state carries no dedicated error field, so read it back here.
  let sessionErrorText: string | null = null;
  if (agentState !== null) {
    for (let index = agentState.items.length - 1; index >= 0; index -= 1) {
      const item = agentState.items[index];
      if (item.role === "error") {
        sessionErrorText = item.text;
        break;
      }
    }
  }

  const modelLabel = sessionClosed
    ? "Session closed"
    : sessionErrored
      ? "Session error"
      : ((currentModel !== null ? modelDisplayLabel(currentModel) : undefined) ??
        manifest?.currentModelId ??
        (agentState === null ? "No agent running" : "No model selected"));
  const modelButtonLabel = modelLabel === "No model selected" ? "No model" : modelLabel;
  const selectedProvider = providers.find((provider) => provider.id === selectedProviderId) ?? null;
  const providerFallback =
    unavailableProviderId === null
      ? providersLoading
        ? "Loading agents…"
        : "Choose agent"
      : `Unavailable: ${unavailableProviderId}`;
  const providerLabel = selectedProvider?.id ?? manifest?.providerId ?? providerFallback;
  const efforts = currentModel?.efforts ?? [];
  const selectedEffortLabel = efforts.find(
    (effort) => effort.id === confirmedEffort(currentModel),
  )?.label;
  const pendingSwitch =
    agentState?.pendingSwitch !== null && agentState?.pendingSwitch !== undefined;
  const providerButtonDisabled = busy;
  const modelButtonDisabled =
    agentSession === null ||
    sessionUnavailable ||
    manifest === null ||
    manifest.models.length === 0;
  const modelButtonUnavailableLabel =
    busy && agentSession === null
      ? "Starting the agent session; its models will appear shortly."
      : agentSession === null
        ? "Start a generation to see the models offered by this agent."
        : sessionClosed
          ? "The agent session has closed; start a generation to reconnect."
          : sessionErrored
            ? `Session error: ${sessionErrorText ?? "The agent reported an unknown error."} The session is still open; start a generation to continue.`
            : manifest === null
              ? "The agent is running; waiting for its model list."
              : // Interim reading until the wire carries the distinction (models as
                // Option<Vec<_>>, absent vs empty): an empty list WITH a current model is
                // self-contradictory — an agent offering no models cannot have a current
                // one — and matches the daemon's model-switch completion fallback, which
                // publishes a manifest naming the switched-to model with models: []. So
                // only an empty list with NO current model counts as evidence of absence.
                manifest.currentModelId !== undefined
                ? "The agent is running; its model list is not known yet."
                : "This agent offered no models.";

  // A picker whose button is disabled must not keep an open flag: a session can close and a
  // later one can open, and the stale flag would reopen the menu with no user action.
  useEffect(() => {
    if (providerButtonDisabled) setProviderPickerOpen(false);
  }, [providerButtonDisabled]);
  useEffect(() => {
    if (modelButtonDisabled) setModelPickerOpen(false);
  }, [modelButtonDisabled]);

  const handleConsentConfirmed = useCallback(
    (provider: ProviderInfo) => {
      onProviderSelect(provider);
      setProviderPickerOpen(false);
    },
    [onProviderSelect],
  );
  const {
    pending: consentProvider,
    request: requestConsent,
    confirm: confirmConsent,
    cancel: cancelConsent,
    inFlight: consentInFlight,
    commandLine: consentCommandLine,
  } = useProviderConsent({ onConfirmed: handleConsentConfirmed });

  useEffect(() => {
    if (!providerPickerOpen && consentProvider !== null) cancelConsent();
  }, [cancelConsent, consentProvider, providerPickerOpen]);

  const dismissProviderPicker = useCallback(() => {
    if (consentProvider !== null) {
      cancelConsent();
      return;
    }
    setProviderPickerOpen(false);
  }, [cancelConsent, consentProvider]);

  // The provider choice (and its consent gate) dismisses when the band
  // opens, like every other menu: the band is the outside press.
  useMenuOpen(providerPickerOpen && !providerButtonDisabled, dismissProviderPicker);

  // Measured against the card that clips the shell, not the window. The keys
  // are what can move the trigger itself: open, the busy close, and the
  // label states that can re-wrap the control strip — the shell is absolute,
  // so nothing inside it moves it. A decline clears the cap to the fallback.
  useLayoutEffect(() => {
    if (!providerPickerOpen || providerButtonDisabled) return;
    const trigger = providerButtonRef.current;
    const menu = providerMenuRef.current;
    if (trigger === null || menu === null) return;
    const apply = () => {
      const placed = menuPlacement(trigger, menu, trigger.closest(".surface-card"));
      if (!placed.below && placed.maxHeight > 0) {
        menu.style.maxHeight = `${placed.maxHeight}px`;
      } else if (menu.style.maxHeight !== "") {
        menu.style.maxHeight = "";
      }
    };
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [
    providerButtonDisabled,
    providerPickerOpen,
    providers.length,
    providersLoading,
    unavailableProviderId,
  ]);

  useEffect(() => {
    if (consentProvider !== null) {
      consentConfirmRef.current?.focus();
      return;
    }
    const trigger = consentRestoreRef.current;
    const providerId = consentRestoreProviderIdRef.current;
    const restoredOption =
      providerId === null
        ? null
        : [
            ...(providerPickerWrapRef.current?.querySelectorAll<HTMLButtonElement>(
              '[role="option"]',
            ) ?? []),
          ].find((option) => option.dataset.providerId === providerId);
    if (restoredOption !== undefined && restoredOption !== null) {
      // preventScroll keeps focus()'s own ancestor walk out of it; the
      // option's focus handler scrolls the list.
      restoredOption.focus({ preventScroll: true });
    } else if (trigger?.isConnected) {
      trigger.focus();
    } else if (trigger !== null || providerId !== null) {
      providerButtonRef.current?.focus();
    }
    if (trigger !== null || providerId !== null) {
      consentRestoreRef.current = null;
      consentRestoreProviderIdRef.current = null;
    }
  }, [consentProvider]);

  useEffect(() => {
    if (!providerPickerOpen) return;
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (isImeComposition(event)) return;
      if (event.key !== "Escape") return;
      event.preventDefault();
      dismissProviderPicker();
    };
    const onMouseDown = (event: globalThis.MouseEvent): void => {
      const root = providerPickerWrapRef.current;
      if (root !== null && event.target instanceof Node && !root.contains(event.target)) {
        dismissProviderPicker();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("mousedown", onMouseDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("mousedown", onMouseDown);
    };
  }, [dismissProviderPicker, providerPickerOpen]);

  return (
    <>
      <div className="design-agent-picker-wrap" ref={providerPickerWrapRef}>
        <button
          ref={providerButtonRef}
          className="design-provider-button"
          type="button"
          // A session keeps the agent it was opened with. While a generation is idle,
          // let the user choose a new provider; that choice closes this session and a
          // later generation opens a fresh one for the selected agent.
          aria-label={
            providerButtonDisabled
              ? `Choose provider: ${providerLabel}. A generation is running; wait for it to finish to change the agent.`
              : `Choose provider: ${providerLabel}`
          }
          title={
            providerButtonDisabled
              ? "A generation is running; wait for it to finish to change the agent."
              : undefined
          }
          aria-expanded={providerButtonDisabled ? undefined : providerPickerOpen}
          aria-controls={providerButtonDisabled ? undefined : "design-provider-picker"}
          disabled={providerButtonDisabled}
          onClick={() => {
            if (consentProvider !== null) return;
            setProviderPickerOpen((open) => !open);
            setModelPickerOpen(false);
          }}
        >
          <span className="design-provider-dot" aria-hidden="true" />
          {providerLabel}
          {providerButtonDisabled ? null : " ▾"}
        </button>
        {providerPickerOpen && !providerButtonDisabled ? (
          <div
            id="design-provider-picker"
            ref={providerMenuRef}
            className={`design-agent-picker${pendingSwitch ? " design-agent-picker-pending" : ""}`}
            role={consentProvider === null ? "listbox" : "group"}
            aria-label={consentProvider === null ? "Choose provider" : "Confirm provider"}
          >
            {consentProvider !== null ? (
              <>
                <div className="design-agent-picker-label">Confirm provider</div>
                {/*
                Focus moves to Confirm as soon as this card appears, so a screen reader
                announces that button and whatever describes it — and nothing else. The
                description therefore has to carry the command itself: approving a
                package download while hearing only the word "Confirm" is not consent.
              */}
                <p className="design-agent-picker-notice" id="design-consent-notice">
                  Approve this command to download and run third-party code:
                </p>
                <code className="design-agent-picker-command" id="design-consent-command">
                  {consentCommandLine}
                </code>
                <div className="design-agent-picker-actions">
                  <button
                    type="button"
                    className="design-agent-picker-secondary"
                    onClick={cancelConsent}
                  >
                    Cancel
                  </button>
                  <button
                    ref={consentConfirmRef}
                    type="button"
                    className="design-agent-picker-primary"
                    aria-describedby="design-consent-notice design-consent-command"
                    onClick={confirmConsent}
                    disabled={consentInFlight}
                  >
                    Confirm
                  </button>
                </div>
              </>
            ) : providersLoading ? (
              <>
                <div className="design-agent-picker-label">Choose agent</div>
                <div className="design-agent-picker-status">Loading agents…</div>
              </>
            ) : providers.length === 0 ? (
              <>
                <div className="design-agent-picker-label">Choose agent</div>
                <div className="design-agent-picker-status">No chat-capable agents found.</div>
              </>
            ) : (
              <>
                <div className="design-agent-picker-label">Choose agent</div>
                <div className="design-agent-picker-options">
                  {providers.map((providerOption) => (
                    <button
                      type="button"
                      role="option"
                      aria-selected={providerOption.id === selectedProviderId}
                      data-provider-id={providerOption.id}
                      className="design-agent-picker-option"
                      key={providerOption.id}
                      onClick={(event) => {
                        if (requiresConsent(providerOption)) {
                          consentRestoreRef.current = event.currentTarget;
                          consentRestoreProviderIdRef.current = providerOption.id;
                          requestConsent(providerOption);
                          return;
                        }
                        onProviderSelect(providerOption);
                        setProviderPickerOpen(false);
                      }}
                      onFocus={(event) => {
                        // The options list is the scrollport: reveal the
                        // focused row in it; the page keeps its own scroll.
                        const list = event.currentTarget.parentElement;
                        if (list !== null) scrollRowIntoView(list, event.currentTarget);
                      }}
                    >
                      {providerOption.id}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        ) : null}
      </div>
      <div className="design-agent-picker-wrap">
        <button
          className="design-provider-button"
          type="button"
          aria-label={modelButtonDisabled ? modelButtonUnavailableLabel : `Model: ${modelLabel}`}
          title={modelButtonDisabled ? modelButtonUnavailableLabel : undefined}
          aria-expanded={modelButtonDisabled ? undefined : modelPickerOpen}
          aria-controls={modelButtonDisabled ? undefined : "design-model-picker"}
          disabled={modelButtonDisabled}
          onClick={() => {
            setModelPickerOpen((open) => !open);
            setProviderPickerOpen(false);
          }}
        >
          <span className="design-provider-dot" aria-hidden="true" />
          {modelButtonLabel}
          {modelButtonDisabled ? null : " ▾"}
        </button>
        {modelButtonDisabled ? null : modelPickerOpen ? (
          <div
            id="design-model-picker"
            className={`design-agent-picker${pendingSwitch ? " design-agent-picker-pending" : ""}`}
            role="group"
            aria-label="Choose model"
            aria-busy={pendingSwitch}
          >
            <>
              <div className="design-agent-picker-label">
                {manifest.providerId ?? providerLabel}
              </div>
              {manifest.models.length > 1 ? (
                <select
                  aria-label="Model"
                  title={
                    (currentModel !== null ? modelDisplayLabel(currentModel) : undefined) ??
                    manifest?.currentModelId ??
                    undefined
                  }
                  value={manifest.currentModelId ?? ""}
                  disabled={pendingSwitch}
                  onChange={(event) => onModelSelect(event.target.value)}
                >
                  {manifest.models.map((model) => (
                    <option key={model.modelId} value={model.modelId}>
                      {modelDisplayLabel(model)}
                    </option>
                  ))}
                </select>
              ) : (
                <span className="design-agent-picker-model-name">
                  {modelDisplayLabel(manifest.models[0])}
                </span>
              )}
              {efforts.length > 0 ? (
                <label className="design-agent-picker-effort">
                  <span>Thinking effort</span>
                  <select
                    aria-label="Thinking effort"
                    title={selectedEffortLabel ?? currentModel?.currentEffort}
                    value={confirmedEffort(currentModel)}
                    disabled={pendingSwitch}
                    onChange={(event) => onEffortSelect(event.target.value)}
                  >
                    {efforts.map((effort) => (
                      <option key={effort.id} value={effort.id}>
                        {effort.label}
                      </option>
                    ))}
                  </select>
                </label>
              ) : null}
            </>
          </div>
        ) : null}
      </div>
    </>
  );
}
