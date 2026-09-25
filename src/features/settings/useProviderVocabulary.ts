/**
 * The profile form's one question to the daemon: what does this provider offer?
 *
 * Extracted from `AgentProfileForm` because the ask has a lifecycle of its own,
 * and that lifecycle is the only place in the settings UI that has to wait for
 * something the daemon cannot answer immediately. An ACP provider's feature list
 * is learned by **starting** it, so the first ask can only say "being read now";
 * the form that stops there leaves a human staring at "Checking…" for the rest of
 * the editor's life, with no feature editable until they close it and reopen it.
 * So this hook re-asks until the answer is a list or a failure, and it stops the
 * moment it is either.
 *
 * Three rules are what make the re-asking safe to hang off a Settings panel, and
 * each has a comment where it is enforced below:
 *
 * - **The question is asked for the provider and a settled model.** A model
 *   field is free text half the time, and one ask per keystroke would be one
 *   provider process per character. A select settles on change, a text field on
 *   blur (`settleModel`); feature declarations remain provider-scoped and the
 *   form applies the current model gate locally.
 * - **A stale reply is dropped, never adopted.** Two guards, one for a newer ask
 *   and one for a reply that names a different provider: what an old question
 *   learned must not dress the current one's controls.
 * - **A failed ask is said once and not retried.** The vocabulary read is not
 *   retried by the app, so a provider that is simply not installed does not turn
 *   the panel into a loop; the daemon's own `absent`-after-a-finished-read is
 *   what the feature rows render as a failure.
 */
import { useEffect, useRef, useState } from "react";
import { providerVocabularyGet } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import type { ProviderInfo, ProviderVocabulary } from "../../types/ipc";
import { ACP_MODE_SUGGESTION } from "./AgentProfileVocabulary";

/** How often the form re-asks while an ACP provider's feature list is being
 *  read. Two seconds: the read is a provider process start, which Paseo budgets
 *  at 90 s, so a faster poll is a dozen identical round-trips against a worker
 *  that cannot finish sooner, and a slower one leaves the human watching
 *  "Checking…" after the answer already exists. */
export const VOCABULARY_POLL_MS = 2000;

export function useProviderVocabulary({
  providerId,
  model,
  providers,
  supported,
  onAcpWithoutModes,
  onSettledModel,
}: {
  providerId: string;
  /** The model field as it stands, settled or not. A reply is trusted only while
   *  it answered this exact value, so a moved-on model cannot inherit the old
   *  model's list. */
  model: string;
  providers: readonly ProviderInfo[];
  /** Whether the daemon advertised `provider_vocabulary`. An older one would
   *  refuse the request, so it is never sent and the form keeps its free-text
   *  fallback. */
  supported: boolean;
  /** The catalog, read for the one question "is this provider an ACP agent".
   *  Consulted through a ref, not an effect dependency: see `providersRef`. */

  /** The one prefill the reply is allowed to make: an ACP agent that declares no
   *  modes runs in "default". */
  onAcpWithoutModes: () => void;
  /** Whether the model field's current value has finished changing. The form's
   *  own `setModel` moves the field; this hook is told about it by `settleModel`,
   *  and the two must agree or a reply answers a model nobody holds — which the
   *  form reads as "no answer in hand" and draws no controls for. */
  onSettledModel: (next: string) => void;
}) {
  const [vocabulary, setVocabulary] = useState<ProviderVocabulary | null>(null);
  const [vocabularyError, setVocabularyError] = useState<ErrorSentence | null>(null);
  // The model the current reply was asked about, kept beside it rather than
  // read out of it: the wire answer names a provider, not the question.
  const [answeredModel, setAnsweredModel] = useState("");
  // Bumped by every ask, so a reply can be recognised as overtaken.
  const seqRef = useRef(0);
  // Bumped only when the *question* changes (provider, settled model, unmount).
  // Guarding the poll on `seqRef` instead was a real bug, found by the poll's own
  // test: every ask bumps the sequence, so the first interval tick invalidated its
  // own chain and the form stopped asking after one retry — a permanent
  // "Checking…", which is the defect this file exists to close.
  const chainRef = useRef(0);
  const pollRef = useRef<number | null>(null);
  // The model as it settled, not as the render last saw it: the interval's
  // callback fires long after the render that armed it.
  const settledModelRef = useRef(model);
  // The catalog, likewise read through a ref. It is only consulted to decide
  // whether *this* provider speaks ACP before a mode prefill is offered, and the
  // array is rebuilt by most parents — naming it in the effect's dependencies
  // made the effect re-run on every render it caused, which is an ask, a clear
  // and a render without end. A caller measured that as 25 timed-out tests.
  const providersRef = useRef(providers);
  providersRef.current = providers;

  function stopPoll() {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }

  function ask(modelForAsk: string) {
    if (!supported || providerId === "") {
      return;
    }
    const seq = ++seqRef.current;
    const chain = chainRef.current;
    void providerVocabularyGet(providerId, modelForAsk, false)
      .then((reply) => {
        // A newer ask owns the form: this reply is stale whatever it names.
        if (seqRef.current !== seq) return;
        // And a reply for another provider is stale by its own admission.
        if (reply.provider !== providerId) return;
        // Adopted as it arrived. A reply missing an axis is malformed and has its
        // own rendered sentence; it must not throw here, because throwing would
        // report a successful query as failed and discard the axis that landed.
        setVocabulary(reply);
        setAnsweredModel(modelForAsk);
        setVocabularyError(null);
        if (reply.modes?.state === "absent") {
          const info = providersRef.current.find((provider) => provider.id === providerId);
          if (info?.protocol === "acp") {
            onAcpWithoutModes();
          }
        }
        if (reply.features?.state !== "absent" || reply.features.probing !== true) {
          // The read answered, or there was nothing to read: stop asking. The
          // prefill above runs on *both* roads — an early return from this arm is
          // how a provider that answers at all lost its mode suggestion.
          stopPoll();
        } else if (pollRef.current === null) {
          // Still being read: this reply is the current truth, and the question
          // gets asked again on the interval until it is not.
          pollRef.current = window.setInterval(() => {
            if (chainRef.current !== chain) {
              stopPoll();
              return;
            }
            ask(settledModelRef.current);
          }, VOCABULARY_POLL_MS);
        }
      })
      .catch((cause: unknown) => {
        if (seqRef.current !== seq) return;
        stopPoll();
        setVocabularyError(errorSentence(cause));
      });
  }

  // Read through a ref so the interval and the effect below call the *current*
  // ask: a closure captured by a timer would otherwise ask the old provider's
  // question, and the dependency list would have to name a function that changes
  // every render to be honest about it.
  const askRef = useRef(ask);
  askRef.current = ask;

  useEffect(() => {
    if (!supported || providerId === "") return;
    chainRef.current += 1;
    setVocabulary(null);
    setVocabularyError(null);
    askRef.current(settledModelRef.current);
    return () => {
      // Unmount ends the question, not just the interval: bumping the chain
      // first makes a tick already in flight stop itself rather than running one
      // more ask into a torn-down frame — which happy-dom reports as a destroyed
      // task manager, and which no assertion would otherwise catch.
      chainRef.current += 1;
      stopPoll();
    };
    // Keyed on the provider and the daemon's support, deliberately not on
    // `model`: a model re-ask goes through `settleModel`, which fires once per
    // real choice rather than once per character.
  }, [providerId, supported]);

  /** A model choice the human has *finished* making: a select's change, or a
   *  free-text field's blur. This is the only road that re-asks, and the reason
   *  it is not the field's every keystroke is what the question costs: for an ACP
   *  provider an ask is a provider process, so typing `claude-opus-5` one
   *  character at a time would start ten agents to answer a question the human
   *  had not finished asking. */
  function settleModel(next: string, askNow = true) {
    if (next === settledModelRef.current) {
      return;
    }
    settledModelRef.current = next;
    onSettledModel(next);
    // The question changed, so the old answer's poll is retired with it.
    chainRef.current += 1;
    stopPoll();
    // Keep the last reply for the model and mode axes while the new feature
    // answer is pending; provider declarations are independent of the model.
    if (askNow) askRef.current(next);
  }

  // A reply for another provider is not this provider's answer, even in the
  // window before the effect clears it.
  const current = vocabulary !== null && vocabulary.provider === providerId;
  // The model-specific answer is used only for mode suggestions, which may
  // depend on which model the provider session reported.
  const answeredFor = current && model === answeredModel;
  const vocabularyAnswered = answeredFor ? vocabulary : null;
  return {
    vocabularyCurrent: current ? vocabulary : null,
    vocabularyAnswered,
    vocabularyError,
    /** The axes are read from the latest reply for this provider. */
    vocabularyKnown: current || vocabularyError !== null,
    settleModel,
    /** Precomputed so the form and its tests read one thing. */
    modeSuggestion: ACP_MODE_SUGGESTION,
  };
}
