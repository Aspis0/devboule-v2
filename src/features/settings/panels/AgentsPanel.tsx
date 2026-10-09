import { useEffect, useMemo, useRef, useState } from "react";
import { agentProfilesGet, agentProfilesSet, providersList } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { isInstalled } from "../../../lib/providerPredicates";
import { ErrorText } from "../../../components/ErrorText";
import {
  DELEGATION_CAPABILITY,
  delegationController,
  useDelegationState,
  type DelegationController,
} from "../../../lib/delegation";
import { AgentProfileForm } from "../AgentProfileForm";
import { ProfileDialog } from "../profiles/ProfileDialog";
import { ProfileRow } from "../profiles/ProfileRow";
import { ByteCounter, SettingsDialog, SettingsRow, SettingsSection } from "../rows";
import {
  EMPTY_PROFILE_FORM_SEED,
  type ProfileFormSeed,
  applyIdleClose,
  enabledNameClash,
  profileDraftRefusal,
  profileFeaturesFromDraft,
  rustTrim,
  seedFromProfile,
  utf8Bytes,
} from "../AgentProfileDraft";
import type {
  AgentProfile,
  AgentProfilesDocument,
  DelegationReply,
  ProviderCatalog,
} from "../../../types/ipc";
import { useSettingsDaemon } from "../settingsDaemon";
import "../profiles.css";
/**
 * The handshake capability that gates the whole Agents section, spelled
 * exactly like the daemon's own name for it. A daemon that does not
 * advertise it cannot answer `agent_profiles_get`, so the section renders
 * nothing and no request is sent — the section is absent, not broken.
 */
const AGENT_PROFILES_CAPABILITY = "agent_profiles";

/**
 * The handshake capability that gates the provider-vocabulary query, spelled
 * exactly like the daemon's own name for it. A daemon that does not advertise
 * it cannot answer `provider_vocabulary_get`. The pickers fall back to free
 * text there, and the form says THAT reason — this daemon is older than this
 * app — in its own sentence.
 */
const PROVIDER_VOCABULARY_CAPABILITY = "provider_vocabulary";

const MAX_STANDING_INSTRUCTIONS_BYTES = 8 * 1024;
/**
 * `MAX_PROFILES` in `crates/devboule-daemon/src/agent_profiles.rs`. The 65th
 * creation is refused by the store, so the panel says so before the human
 * fills the form instead of offering a create it knows cannot be kept.
 */
const MAX_PROFILES = 64;

/**
 * Every write replaces the whole document, so each one travels on a deep
 * copy: a mutation made for one write must never sit under an earlier
 * write's revert base, and no render may alias the stored document.
 */
function cloneDocument(document: AgentProfilesDocument): AgentProfilesDocument {
  return JSON.parse(JSON.stringify(document)) as AgentProfilesDocument;
}

/**
 * The sentence for the one absence the delegation section can name without
 * asking anyone: the daemon predates `permission_delegation`, so there is no
 * store to ask and no request may be sent.
 */
const DELEGATION_UNAVAILABLE_TEXT =
  "This daemon is older than this app: it cannot keep the delegation switch.";

/**
 * The sentence for a reply that disagrees with itself: a source beside a
 * switch value it cannot hold.
 */
const DELEGATION_CONTRADICTION_LABEL =
  "The daemon's answer contradicts itself — this source cannot hold this switch value";

/**
 * The sentences the stored answer's `source` renders as, one row per value
 * and an arm for each switch reading. `default` is "never configured" — a
 * human said nothing yet, so the built-in on applies; `quarantined` is
 * neither that nor "off" — a human DID configure, and the file came back
 * damaged; `file` is the deliberate case.
 */
export const DELEGATION_SOURCE_LABELS: Record<
  DelegationReply["source"],
  { off: string; on: string }
> = {
  file: { off: "Off", on: "On" },
  default: { off: "Off (never configured)", on: "Never configured — on by default" },
  quarantined: {
    off: "Settings file was damaged — delegation reads off",
    on: DELEGATION_CONTRADICTION_LABEL,
  },
};

/**
 * The sentence a source value OUTSIDE the closed vocabulary renders as. A
 * `Record` indexed with an unknown key yields `undefined`, and
 * `undefined !== null` would render an empty status line — a line that says
 * nothing, when unknown must be present.
 */
const DELEGATION_SOURCE_UNKNOWN_LABEL =
  "Delegation's stored answer came from a source this app cannot name";

/**
 * The status line while the stored answer has not landed. The switch holds
 * no value yet — it must not read as a definite off with no sentence saying
 * otherwise.
 */
const DELEGATION_PENDING_LABEL = "Reading the stored answer…";

/**
 * The status line when the stored answer never arrived. A failed load is
 * terminal: the switch stays empty, and the empty state is named — never
 * styled into a definite off.
 */
const DELEGATION_UNREADABLE_LABEL =
  "The stored answer could not be read — the switch holds no value, not an off";

function delegationSourceSentence(source: string, enabled: boolean): string {
  return Object.hasOwn(DELEGATION_SOURCE_LABELS, source)
    ? DELEGATION_SOURCE_LABELS[source as DelegationReply["source"]][enabled ? "on" : "off"]
    : DELEGATION_SOURCE_UNKNOWN_LABEL;
}

/**
 * Settings → Agents, beside the profiles: the one consent surface for
 * delegated answering. One toggle row — the label, the switch on the right,
 * the stored answer's one-line status under it.
 */
export function DelegationSetting({
  controller = delegationController,
}: {
  /** Injectable so tests get a fresh controller, like the tauri seams. */
  controller?: DelegationController;
}) {
  const daemon = useSettingsDaemon();
  const delegationSupported = daemon.capabilities.includes(DELEGATION_CAPABILITY);
  const delegation = useDelegationState(controller);

  useEffect(() => {
    if (!delegationSupported || daemon.state !== "connected") return;
    void controller.load();
  }, [controller, delegationSupported, daemon.state, daemon.instanceId]);

  if (!delegationSupported) {
    return (
      <span className="device-copy agent-delegation-unavailable" role="note">
        {DELEGATION_UNAVAILABLE_TEXT}
      </span>
    );
  }

  const { enabled, reply, loadFailed, error, retryLoad } = delegation;
  const statusLine =
    enabled === null
      ? loadFailed
        ? DELEGATION_UNREADABLE_LABEL
        : DELEGATION_PENDING_LABEL
      : reply === null
        ? null
        : delegationSourceSentence(reply.source, enabled);

  return (
    <section className="agent-delegation" aria-label="Answer for created children">
      <SettingsRow
        title="Let agents answer their children's cards"
        description={
          statusLine === null ? undefined : (
            <span className="agent-delegation-source">{statusLine}</span>
          )
        }
        control={
          <input
            type="checkbox"
            role="switch"
            aria-label="Let agents answer their children's cards"
            checked={enabled === true}
            aria-checked={enabled === null ? "mixed" : enabled ? "true" : "false"}
            ref={(el) => {
              if (el !== null) el.indeterminate = enabled === null;
            }}
            className={enabled === null ? "agent-delegation-switch-unknown" : undefined}
            disabled={enabled === null}
            onChange={(event) => void controller.setEnabled(event.target.checked)}
          />
        }
      />
      {error === null ? null : (
        <p role="alert" className="device-error">
          <ErrorText
            sentence={error.sentence}
            detail={error.detail}
            id="settings-delegation-error"
          />
        </p>
      )}
      {loadFailed ? (
        <button type="button" className="settings-device-action" onClick={retryLoad}>
          Retry
        </button>
      ) : null}
    </section>
  );
}

/**
 * Settings → Agents: the daemon's agent-profile store. Standing instructions
 * are one row with an Edit button, the profiles are rows under an "Agent
 * profiles" label with a "+", and the delegation switch closes the page.
 * Long editors live in dialogs; the page itself is rows, never paragraphs.
 */
export function AgentProfilesPanel() {
  const daemon = useSettingsDaemon();
  const agentProfilesSupported = daemon.capabilities.includes(AGENT_PROFILES_CAPABILITY);
  const providerVocabularySupported = daemon.capabilities.includes(PROVIDER_VOCABULARY_CAPABILITY);
  const [document, setDocument] = useState<AgentProfilesDocument | null>(null);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [busy, setBusy] = useState(false);
  // Synchronous mirror of `document` — what a second rapid write reads and
  // what a rejection reverts onto, never a render closure.
  const documentRef = useRef<AgentProfilesDocument | null>(null);
  // Monotonic write sequence: only the newest write owns the UI when it settles.
  const seqRef = useRef(0);
  // How many writes are currently inside `persist` — sent, not yet settled
  // by their read-back or their revert.
  const writesInFlightRef = useRef(0);
  // Which dialog is open, panel-wide: at most one profile dialog, plus the
  // standing-instructions editor beside it.
  const [dialog, setDialog] = useState<{ mode: "create" } | { mode: "edit"; id: string } | null>(
    null,
  );
  const [standingOpen, setStandingOpen] = useState(false);
  const [standingError, setStandingError] = useState<ErrorSentence | null>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const standingReturnFocusRef = useRef<HTMLElement | null>(null);
  const listRef = useRef<HTMLOListElement>(null);
  const [pendingFocus, setPendingFocus] = useState<
    | { opener: HTMLElement }
    | { rowIndex: number }
    | { trashProfileId: string }
    | { listNow: true }
    | null
  >(null);

  useEffect(() => {
    if (pendingFocus === null) return;
    setPendingFocus(null);
    if ("opener" in pendingFocus) {
      pendingFocus.opener.focus();
    } else if ("rowIndex" in pendingFocus) {
      const rows = listRef.current?.querySelectorAll(".agent-profile-row");
      const pencil = rows
        ?.item(Math.min(pendingFocus.rowIndex, (rows?.length ?? 1) - 1))
        ?.querySelector<HTMLButtonElement>('button[aria-label^="Edit "]');
      if (pencil) pencil.focus();
      else listRef.current?.focus();
    } else if ("listNow" in pendingFocus) {
      listRef.current?.focus();
    } else {
      const row = Array.from(listRef.current?.querySelectorAll(".agent-profile-row") ?? []).find(
        (candidate) => candidate.getAttribute("data-profile-id") === pendingFocus.trashProfileId,
      );
      row?.querySelector<HTMLButtonElement>('button[aria-label^="Delete "]')?.focus();
    }
  }, [pendingFocus]);
  const [deleteArmedId, setDeleteArmedId] = useState<string | null>(null);
  // The dialogs' unsaved drafts. They live HERE, not in the forms' own
  // state: the discard confirm unmounts the form, and a draft kept locally
  // would remount blank after Keep editing; the edit draft additionally
  // survives its row being removed and reverted by an in-flight delete.
  // Closing a dialog abandons its draft.
  const [createDraft, setCreateDraft] = useState<ProfileFormSeed | null>(null);
  const [editorDraft, setEditorDraft] = useState<(ProfileFormSeed & { id: string }) | null>(null);
  const [catalog, setCatalog] = useState<ProviderCatalog | null>(null);
  const [catalogError, setCatalogError] = useState<ErrorSentence | null>(null);
  // The standing-instructions draft. Null means the editor shows the
  // document; the first keystroke sets it. Released only by its own writer's
  // confirmation — while still exactly what was sent — or by a fresh load.
  const [standingDraft, setStandingDraft] = useState<string | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [loadNonce, setLoadNonce] = useState(0);

  useEffect(() => {
    if (!agentProfilesSupported) return;
    let cancelled = false;
    const seqAtFetch = seqRef.current;
    const writeWasInFlight = writesInFlightRef.current > 0;
    void agentProfilesGet()
      .then((reply) => {
        if (cancelled) return;
        if (seqRef.current !== seqAtFetch) return;
        if (writeWasInFlight) return;
        documentRef.current = reply.document;
        setDocument(reply.document);
        setError(null);
        setLoadFailed(false);
        setStandingDraft(null);
        setEditorDraft(null);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (documentRef.current === null) setLoadFailed(true);
        setError(errorSentence(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [agentProfilesSupported, loadNonce]);

  useEffect(() => {
    if (!agentProfilesSupported) return;
    let cancelled = false;
    void providersList()
      .then((listed) => {
        if (!cancelled) setCatalog(listed);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setCatalogError(errorSentence(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [agentProfilesSupported]);

  const installedProviders = useMemo(
    () =>
      (catalog?.providers ?? []).filter(
        (provider) => isInstalled(provider) && provider.enabled !== false,
      ),
    [catalog],
  );

  if (!agentProfilesSupported) return null;
  const loading = document === null;
  const profiles = document?.profiles ?? [];
  const standingValue = standingDraft ?? document?.standingInstructions ?? "";
  const standingBytes = utf8Bytes(standingValue);

  /**
   * The write discipline — one rule for all writers (editor, delete, move,
   * create, standing instructions), stated here once because it is the one
   * place every writer passes through. Three clauses:
   *
   * 1. One write owns the panel from its optimistic swap until it settles:
   *    a confirmation PLUS its read-back, or a refusal's revert. `busy`
   *    spans that whole stretch, so no second write can start inside it.
   * 2. A document fetch adopts its reply only when NO write overlapped it.
   * 3. What the human typed but has not saved is not a write. Drafts live
   *    above the write plane; no write that did not carry their text
   *    releases them.
   */
  async function persist(next: AgentProfilesDocument): Promise<boolean> {
    const previous = documentRef.current;
    const seq = ++seqRef.current;
    setBusy(true);
    setError(null);
    documentRef.current = next;
    setDocument(next);
    writesInFlightRef.current += 1;
    try {
      await agentProfilesSet(next);
      try {
        const reply = await agentProfilesGet();
        if (seqRef.current === seq) {
          documentRef.current = reply.document;
          setDocument(reply.document);
        }
      } catch (cause: unknown) {
        if (seqRef.current === seq) setError(errorSentence(cause));
      }
      const confirmed = seq === seqRef.current;
      if (confirmed) setBusy(false);
      return confirmed;
    } catch (cause) {
      if (seq !== seqRef.current) return false;
      documentRef.current = previous;
      setDocument(previous);
      setError(errorSentence(cause));
      setBusy(false);
      return false;
    } finally {
      writesInFlightRef.current -= 1;
    }
  }

  function retryLoad() {
    setError(null);
    setLoadFailed(false);
    setLoadNonce((nonce) => nonce + 1);
  }

  function closeDialog() {
    setDialog(null);
    setEditorDraft(null);
    setCreateDraft(null);
    const opener = returnFocusRef.current;
    if (
      opener !== null &&
      opener.isConnected &&
      !(opener instanceof HTMLButtonElement && opener.disabled)
    ) {
      returnFocusRef.current = null;
      setPendingFocus({ opener });
    } else {
      setPendingFocus({ listNow: true });
    }
  }

  function closeStanding() {
    setStandingOpen(false);
    setStandingError(null);
    const opener = standingReturnFocusRef.current;
    standingReturnFocusRef.current = null;
    if (opener !== null && opener.isConnected) opener.focus();
  }

  function openCreateDialog(opener: HTMLElement) {
    returnFocusRef.current = opener;
    setError(null);
    setDeleteArmedId(null);
    setDialog({ mode: "create" });
  }

  function openEditDialog(id: string, opener: HTMLElement) {
    const current = documentRef.current;
    const row = current?.profiles.find((profile) => profile.id === id);
    if (row !== undefined) setEditorDraft({ id, ...seedFromProfile(row) });
    returnFocusRef.current = opener;
    setError(null);
    setDeleteArmedId(null);
    setDialog({ mode: "edit", id });
  }

  function openStanding(opener: HTMLElement) {
    standingReturnFocusRef.current = opener;
    setStandingError(null);
    setStandingOpen(true);
  }

  function move(id: string, delta: -1 | 1) {
    const current = documentRef.current;
    if (current === null) return;
    const from = current.profiles.findIndex((profile) => profile.id === id);
    const to = from + delta;
    if (from < 0 || to < 0 || to >= current.profiles.length) return;
    const updated = cloneDocument(current);
    const [row] = updated.profiles.splice(from, 1);
    updated.profiles.splice(to, 0, row);
    void persist(updated);
  }

  function remove(id: string) {
    const current = documentRef.current;
    if (current === null) return;
    const index = current.profiles.findIndex((profile) => profile.id === id);
    const removedId = current.profiles[index]?.id ?? "";
    const updated = cloneDocument(current);
    updated.profiles = updated.profiles.filter((profile) => profile.id !== id);
    setDeleteArmedId(null);
    void persist(updated).then((confirmed) => {
      setPendingFocus(confirmed ? { rowIndex: index } : { trashProfileId: removedId });
    });
  }

  function saveProfileFields(id: string, draft: ProfileFormSeed) {
    const current = documentRef.current;
    if (current === null) return;
    const refusal = profileDraftRefusal(draft);
    if (refusal !== null) {
      setError({ sentence: refusal, detail: null });
      return;
    }
    const trimmedName = rustTrim(draft.name);
    if (draft.enabledForAgents && enabledNameClash(current.profiles, id, trimmedName)) {
      setError({
        sentence: `The name '${trimmedName}' is already used by a profile enabled for agents; a creation resolves a profile by name, so they could not be told apart.`,
        detail: null,
      });
      return;
    }
    const updated = cloneDocument(current);
    const row = updated.profiles.find((profile) => profile.id === id);
    if (row === undefined) return;
    row.name = rustTrim(draft.name);
    row.icon = rustTrim(draft.icon) === "" ? null : rustTrim(draft.icon);
    row.note = draft.note;
    const spawn = rustTrim(draft.spawnPrompt);
    if (spawn === "") {
      delete row.spawnPrompt;
    } else {
      row.spawnPrompt = spawn;
    }
    row.provider = draft.provider;
    row.model = rustTrim(draft.model);
    // The serving provider travels beside the bare id; absent is what older
    // builds wrote, and those still read.
    const serving = rustTrim(draft.modelProvider ?? "");
    row.modelProvider = serving === "" ? null : serving;
    row.modeId = rustTrim(draft.modeId);
    const thinking = rustTrim(draft.thinkingOptionId);
    row.thinkingOptionId = thinking === "" ? null : thinking;
    row.features = profileFeaturesFromDraft(draft, draft.offeredFeatures);
    row.enabledForAgents = draft.enabledForAgents;
    row.toolOverlay = [...draft.overlay];
    applyIdleClose(row, draft);
    void persist(updated).then((confirmed) => {
      if (confirmed) closeDialog();
    });
  }

  function createProfile(draft: ProfileFormSeed) {
    const current = documentRef.current;
    if (current === null) return;
    if (current.profiles.length >= MAX_PROFILES) {
      setError({
        sentence: `The store already holds ${MAX_PROFILES} profiles, the maximum the daemon allows: delete one before creating another.`,
        detail: null,
      });
      return;
    }
    const refusal = profileDraftRefusal(draft);
    if (refusal !== null) {
      setError({ sentence: refusal, detail: null });
      return;
    }
    const trimmedName = rustTrim(draft.name);
    if (draft.enabledForAgents && enabledNameClash(current.profiles, null, trimmedName)) {
      setError({
        sentence: `The name '${trimmedName}' is already used by a profile enabled for agents; a creation resolves a profile by name, so they could not be told apart.`,
        detail: null,
      });
      return;
    }
    const icon = rustTrim(draft.icon);
    const serving = rustTrim(draft.modelProvider ?? "");
    const profile: AgentProfile = {
      id: "",
      name: rustTrim(draft.name),
      icon: icon === "" ? null : icon,
      note: draft.note,
      provider: draft.provider,
      model: rustTrim(draft.model),
      modelProvider: serving === "" ? null : serving,
      modeId: rustTrim(draft.modeId),
      thinkingOptionId:
        rustTrim(draft.thinkingOptionId) === "" ? null : rustTrim(draft.thinkingOptionId),
      features: profileFeaturesFromDraft(draft, draft.offeredFeatures),
      toolOverlay: [...draft.overlay],
      enabledForAgents: draft.enabledForAgents,
    };
    const spawn = rustTrim(draft.spawnPrompt);
    if (spawn !== "") {
      profile.spawnPrompt = spawn;
    }
    applyIdleClose(profile, draft);
    const updated = cloneDocument(current);
    updated.profiles = [...updated.profiles, profile];
    void persist(updated).then((confirmed) => {
      if (confirmed) closeDialog();
    });
  }

  function saveStandingInstructions() {
    const current = documentRef.current;
    if (current === null) return;
    const bytes = utf8Bytes(standingValue);
    if (bytes > MAX_STANDING_INSTRUCTIONS_BYTES) {
      setStandingError({
        sentence: `The standing instructions are ${bytes} bytes, over the ${MAX_STANDING_INSTRUCTIONS_BYTES}-byte cap. Nothing was saved and nothing was truncated.`,
        detail: null,
      });
      return;
    }
    const sent = standingValue;
    const updated = cloneDocument(current);
    updated.standingInstructions = sent;
    void persist(updated).then((confirmed) => {
      if (confirmed) {
        // The store now holds `sent`. The draft is released only while it is
        // still exactly what was sent: keystrokes typed while the write was
        // in flight are newer than the store and survive it.
        setStandingDraft((draft) => (draft === sent ? null : draft));
        closeStanding();
      }
    });
  }

  const dialogTarget =
    dialog !== null && dialog.mode === "edit" && document !== null
      ? (document.profiles.find((profile) => profile.id === dialog.id) ?? null)
      : null;

  const atCap = profiles.length >= MAX_PROFILES;

  return (
    <div id="settings-panel-agents">
      <div inert={dialog !== null || standingOpen}>
        <div className="settings-stack settings-stack-spaced agent-profiles">
          {error === null || dialog !== null || standingOpen ? null : (
            <p role="alert" className="device-error">
              <ErrorText
                sentence={error.sentence}
                detail={error.detail}
                id="settings-agents-error"
              />
            </p>
          )}
          {loading && !loadFailed ? <div role="status">Loading agent profiles…</div> : null}
          {loadFailed ? (
            <button type="button" className="settings-device-action" onClick={retryLoad}>
              Retry
            </button>
          ) : null}
          <SettingsRow
            title="Standing instructions"
            control={
              <button
                type="button"
                className="settings-device-action"
                disabled={loading}
                onClick={(event) => openStanding(event.currentTarget)}
              >
                Edit
              </button>
            }
          />
          <SettingsSection
            label="Agent profiles"
            action={
              <button
                type="button"
                className="settings-device-action"
                aria-label="New profile"
                aria-haspopup="dialog"
                disabled={busy || loading || atCap}
                onClick={(event) => openCreateDialog(event.currentTarget)}
              >
                +
              </button>
            }
          >
            {document !== null && profiles.length === 0 ? (
              <span className="device-copy">No profiles yet.</span>
            ) : null}
            {atCap ? (
              <span className="device-field-hint" role="status">
                The store holds the maximum of {MAX_PROFILES} profiles.
              </span>
            ) : null}
            <ol
              className="agent-profile-list"
              ref={listRef}
              tabIndex={-1}
              aria-label="Agent profiles"
            >
              {profiles.map((profile, index) => (
                <li className="agent-profile-row" key={profile.id} data-profile-id={profile.id}>
                  <ProfileRow
                    profile={profile}
                    isFirst={index === 0}
                    isLast={index === profiles.length - 1}
                    busy={busy}
                    loading={loading}
                    deleteArmed={deleteArmedId === profile.id}
                    onMove={move}
                    onEdit={openEditDialog}
                    onDeleteArm={(id) => setDeleteArmedId(id)}
                    onDeleteCancel={() => setDeleteArmedId(null)}
                    onDeleteConfirm={remove}
                  />
                </li>
              ))}
            </ol>
          </SettingsSection>
        </div>
        <DelegationSetting />
      </div>
      <SettingsDialog
        open={standingOpen && document !== null}
        title="Standing instructions"
        busy={busy}
        dirty={standingDraft !== null}
        onClose={closeStanding}
      >
        {({ requestClose }) => (
          <div className="agent-inline-editor">
            <label className="device-field">
              Rules every agent receives with its first task
              <textarea
                aria-label="Standing instructions for every agent"
                value={standingValue}
                disabled={busy}
                onChange={(event) => setStandingDraft(event.target.value)}
                rows={6}
              />
              <ByteCounter bytes={standingBytes} cap={MAX_STANDING_INSTRUCTIONS_BYTES} />
            </label>
            {standingError === null ? null : (
              <p role="alert" className="device-error">
                <ErrorText
                  sentence={standingError.sentence}
                  detail={standingError.detail}
                  id="settings-standing-error"
                />
              </p>
            )}
            <div className="device-actions profile-form-actions">
              <button
                type="button"
                className="settings-device-action"
                disabled={busy || loading}
                onClick={saveStandingInstructions}
              >
                Save standing instructions
              </button>
              <button type="button" className="settings-device-action" onClick={requestClose}>
                Cancel
              </button>
            </div>
          </div>
        )}
      </SettingsDialog>
      <ProfileDialog
        open={
          dialog !== null &&
          document !== null &&
          (dialog.mode === "create" || dialogTarget !== null)
        }
        title={
          dialog === null
            ? "Profiles"
            : dialog.mode === "create"
              ? "New profile"
              : `Edit profile — ${dialogTarget?.name}`
        }
        busy={busy}
        onClose={closeDialog}
      >
        {({ requestClose, markDirty }) =>
          dialog === null ? null : dialog.mode === "create" ? (
            <AgentProfileForm
              mode="create"
              hideHeading
              seed={createDraft ?? EMPTY_PROFILE_FORM_SEED}
              providers={installedProviders}
              catalogLoading={catalog === null && catalogError === null}
              catalogError={catalogError}
              vocabularySupported={providerVocabularySupported}
              busy={busy}
              onCreate={createProfile}
              onSeedChange={setCreateDraft}
              onDirty={markDirty}
              formError={error}
              onPairRefusal={(sentence) => setError({ sentence, detail: null })}
              onCancel={requestClose}
            />
          ) : (
            <AgentProfileForm
              mode="edit"
              hideHeading
              seed={
                editorDraft?.id === dialog.id
                  ? editorDraft
                  : dialogTarget !== null
                    ? seedFromProfile(dialogTarget)
                    : EMPTY_PROFILE_FORM_SEED
              }
              providers={installedProviders}
              catalogLoading={catalog === null && catalogError === null}
              catalogError={catalogError}
              vocabularySupported={providerVocabularySupported}
              busy={busy}
              onCreate={createProfile}
              onSaveSeed={(draft) => saveProfileFields(dialog.id, draft)}
              onSeedChange={(draft) => setEditorDraft({ id: dialog.id, ...draft })}
              onDirty={markDirty}
              formError={error}
              onPairRefusal={(sentence) => setError({ sentence, detail: null })}
              onCancel={requestClose}
            />
          )
        }
      </ProfileDialog>
    </div>
  );
}
