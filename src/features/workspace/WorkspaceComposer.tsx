import { memo, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import type { ChangeEvent, KeyboardEvent as ReactKeyboardEvent } from 'react';
import {
  MOCK_PROVIDER_MANIFESTS,
  type MockEffortLevel,
  type MockProviderManifest,
} from './mockData';

export function getProviderManifest(providerId: string): MockProviderManifest {
  return MOCK_PROVIDER_MANIFESTS.find((provider) => provider.id === providerId) ?? MOCK_PROVIDER_MANIFESTS[0];
}

interface WorkspaceComposerProps {
  streaming: boolean;
  providerId: string;
  onProviderChange: (providerId: string) => void;
  onSend: (text: string) => void;
}

export const WorkspaceComposer = memo(function WorkspaceComposer({
  streaming,
  providerId,
  onProviderChange,
  onSend,
}: WorkspaceComposerProps) {
  const provider = useMemo(() => getProviderManifest(providerId), [providerId]);
  const [input, setInput] = useState('');
  const [modelId, setModelId] = useState(provider.defaults.modelId);
  const [modeState, setModeState] = useState<Record<string, boolean>>({});
  const [effort, setEffort] = useState<MockEffortLevel | null>(provider.defaults.effort);
  const [modesOpen, setModesOpen] = useState(false);
  const modesWrapRef = useRef<HTMLDivElement | null>(null);
  const modesToggleRef = useRef<HTMLButtonElement | null>(null);
  const firstModeToggleRef = useRef<HTMLButtonElement | null>(null);
  const modesMenuId = useId();

  const selectedModel = useMemo(
    () => provider.models.find((model) => model.id === modelId) ?? provider.models[0],
    [modelId, provider],
  );
  const effortLevels = useMemo(
    () => provider.effortLevels.filter((level) => selectedModel?.thinkingLevels.includes(level)),
    [provider, selectedModel],
  );
  const effectiveModelId = selectedModel?.id ?? '';
  const effectiveEffort = effort && effortLevels.includes(effort)
    ? effort
    : provider.defaults.effort && effortLevels.includes(provider.defaults.effort)
      ? provider.defaults.effort
      : effortLevels[0] ?? null;
  const effectiveModes = useMemo(
    () => provider.modes.reduce<Record<string, boolean>>((modes, mode) => {
      modes[mode.id] = modeState[mode.id] ?? provider.defaults.modes[mode.id] ?? false;
      return modes;
    }, {}),
    [modeState, provider],
  );

  const activeModes = useMemo(
    () => provider.modes.filter((mode) => effectiveModes[mode.id]),
    [provider, effectiveModes],
  );
  const modesSummary = activeModes.length === 0
    ? 'Modes'
    : activeModes.length === 1
      ? activeModes[0].label
      : `${activeModes[0].label} +${activeModes.length - 1}`;
  const modesAriaLabel = activeModes.length === 0
    ? 'Modes'
    : `Modes: ${activeModes.map((mode) => mode.label).join(', ')}`;

  useEffect(() => {
    setModelId(provider.defaults.modelId);
    setModeState(provider.defaults.modes);
    setEffort(provider.defaults.effort);
    setModesOpen(false);
  }, [provider]);

  const sendInput = useCallback(() => {
    const text = input.trim();
    if (!text) return;
    onSend(text);
    setInput('');
  }, [input, onSend]);

  const handleComposerKeyDown = useCallback((event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      sendInput();
    }
  }, [sendInput]);

  const handleProviderChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    onProviderChange(event.target.value);
  }, [onProviderChange]);

  const handleModelChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    const nextModel = provider.models.find((model) => model.id === event.target.value);
    if (!nextModel) return;

    setModelId(nextModel.id);
    const nextEffortLevels = provider.effortLevels.filter((level) => nextModel.thinkingLevels.includes(level));
    setEffort((currentEffort) => currentEffort && nextEffortLevels.includes(currentEffort)
      ? currentEffort
      : nextEffortLevels[0] ?? null);
  }, [provider]);

  const handleEffortChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    const nextEffort = event.target.value as MockEffortLevel;
    if (effortLevels.includes(nextEffort)) setEffort(nextEffort);
  }, [effortLevels]);

  const toggleMode = useCallback((modeId: string) => {
    setModeState((currentModes) => ({
      ...currentModes,
      [modeId]: !currentModes[modeId],
    }));
  }, []);

  const handleModesToggleKeyDown = useCallback((event: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    if (modesOpen) {
      firstModeToggleRef.current?.focus();
      return;
    }
    setModesOpen(true);
    requestAnimationFrame(() => firstModeToggleRef.current?.focus());
  }, [modesOpen]);

  useEffect(() => {
    if (!modesOpen) return;
    const handlePointerDown = (event: PointerEvent) => {
      if (modesWrapRef.current && event.target instanceof Node && !modesWrapRef.current.contains(event.target)) {
        setModesOpen(false);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setModesOpen(false);
        modesToggleRef.current?.focus();
      }
    };
    document.addEventListener('pointerdown', handlePointerDown);
    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown);
      document.removeEventListener('keydown', handleKeyDown);
    };
  }, [modesOpen]);

  return (
    <div className="workspace-composer-wrap">
      <div className="workspace-composer">
        <textarea
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={handleComposerKeyDown}
          placeholder="Steer the running turn, or start a new one…"
          rows={2}
          aria-label="Workspace message"
        />
        <div className="workspace-composer-footer">
          <div className="workspace-composer-control-row">
            <label className="workspace-composer-select-wrap workspace-provider-select-wrap">
              <span className="sr-only">Provider for this session</span>
              <select
                className="workspace-composer-select workspace-provider-select"
                value={provider.id}
                onChange={handleProviderChange}
                aria-label="Provider for this session"
              >
                {MOCK_PROVIDER_MANIFESTS.map((manifest) => (
                  <option value={manifest.id} key={manifest.id}>{manifest.name}</option>
                ))}
              </select>
            </label>
            <label className="workspace-composer-select-wrap workspace-model-select-wrap">
              <span className="sr-only">Model</span>
              <select
                className="workspace-composer-select workspace-model-select"
                value={effectiveModelId}
                onChange={handleModelChange}
                aria-label="Model"
                disabled={provider.models.length === 0}
              >
                {provider.models.map((model) => (
                  <option value={model.id} key={model.id}>{model.label}</option>
                ))}
              </select>
            </label>
            {effortLevels.length > 0 ? (
              <label className="workspace-effort-control">
                <span className="workspace-effort-label">Thinking</span>
                <select
                  value={effectiveEffort ?? ''}
                  onChange={handleEffortChange}
                  aria-label="Thinking effort"
                >
                  {effortLevels.map((level) => <option value={level} key={level}>{level}</option>)}
                </select>
              </label>
            ) : null}
            <div className="workspace-modes-wrap" ref={modesWrapRef}>
              <button
                type="button"
                className="workspace-modes-toggle"
                ref={modesToggleRef}
                onClick={() => setModesOpen((open) => !open)}
                onKeyDown={handleModesToggleKeyDown}
                aria-expanded={modesOpen}
                aria-controls={modesOpen ? modesMenuId : undefined}
                aria-label={modesAriaLabel}
              >
                <span className="workspace-modes-summary">{modesSummary}</span>
              </button>
              {modesOpen ? (
                <div className="workspace-modes-menu" id={modesMenuId}>
                  <div className="workspace-composer-mode-group" role="group" aria-label={`Modes for ${provider.name}`}>
                    {provider.modes.map((mode, index) => (
                      <button
                        type="button"
                        className={`workspace-mode-toggle${effectiveModes[mode.id] ? ' workspace-mode-toggle-active' : ''}`}
                        key={mode.id}
                        ref={index === 0 ? firstModeToggleRef : undefined}
                        onClick={() => toggleMode(mode.id)}
                        aria-pressed={effectiveModes[mode.id]}
                        title={mode.description}
                      >
                        {mode.label}
                      </button>
                    ))}
                    {provider.modes.length === 0 ? (
                      <span className="workspace-no-modes">No modes for this provider</span>
                    ) : null}
                  </div>
                </div>
              ) : null}
            </div>
            {streaming ? <span className="workspace-steer-hint">goes into the running turn</span> : null}
            <button
              type="button"
              className="workspace-primary-action workspace-send-action"
              onClick={sendInput}
              disabled={!input.trim()}
            >
              Send
            </button>
          </div>
        </div>
      </div>
    </div>
  );
});
