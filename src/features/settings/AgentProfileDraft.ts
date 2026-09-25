/**
 * The agent-profile draft: what the form's fields hold in both modes, the
 * stored row converted into it, and every rule a save must satisfy before
 * the panel sends anything. The rules mirror `agent_profiles.rs` exactly —
 * the same caps, the same trim set, the same NFC name comparison — so a
 * draft the form accepts is a document the store admits. The form component
 * (`AgentProfileForm.tsx`) owns the fields; the panel owns the document.
 */
import type { AgentProfile, ProviderVocabulary, VocabularyFeature } from "../../types/ipc";

/** The profile store's caps, the daemon's own constants mirrored. */
const MAX_PROFILE_NAME_CHARS = 60;
const MAX_PROFILE_NOTE_BYTES = 2 * 1024;
const MAX_PROFILE_ICON_BYTES = 64;
/** `MAX_PROFILE_SPAWN_PROMPT_BYTES` in `agent_profiles.rs`. */
export const MAX_PROFILE_SPAWN_PROMPT_BYTES = 8 * 1024;
/** `MAX_PROFILE_FIELD_BYTES` in `agent_profiles.rs`: ids like the thinking option. */
const MAX_PROFILE_FIELD_BYTES = 128;
/** `AUTO_ACCEPT_FEATURE` in `provider_catalog.rs`: the tick every agent family
 *  reads, and the only feature key whose meaning the daemon wrote itself. */
export const AUTO_ACCEPT_FEATURE = "autoAccept";

/**
 * Trims exactly what Rust's `str::trim` trims — the Unicode `White_Space`
 * set — and not what ECMAScript's `trim()` trims: JavaScript leaves U+0085
 * and drops U+FEFF, Rust is the other way round. Comparisons and byte caps
 * here must use the daemon's rule, or the form's preflight diverges from the
 * store's canonical form.
 */
const RUST_WHITE_SPACE = new Set([
  "\u{0009}",
  "\u{000A}",
  "\u{000B}",
  "\u{000C}",
  "\u{000D}",
  "\u{0020}",
  "\u{0085}",
  "\u{00A0}",
  "\u{1680}",
  "\u{2000}",
  "\u{2001}",
  "\u{2002}",
  "\u{2003}",
  "\u{2004}",
  "\u{2005}",
  "\u{2006}",
  "\u{2007}",
  "\u{2008}",
  "\u{2009}",
  "\u{200A}",
  "\u{2028}",
  "\u{2029}",
  "\u{202F}",
  "\u{205F}",
  "\u{3000}",
]);

export function rustTrim(text: string): string {
  let start = 0;
  let end = text.length;
  while (start < end && RUST_WHITE_SPACE.has(text[start]!)) start += 1;
  while (end > start && RUST_WHITE_SPACE.has(text[end - 1]!)) end -= 1;
  return text.slice(start, end);
}

/** The daemon counts UTF-8 bytes (`String::len`), so the on-screen counter must too. */
export function utf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * The daemon counts a profile name in Unicode scalar values
 * (`str::chars().count()`), so the cap must count the same unit: iteration
 * yields whole code points, and one astral-plane character (emoji, CJK
 * extensions) is one — where UTF-16 code-unit counting would call it two and
 * refuse names the daemon accepts.
 */
function charCount(text: string): number {
  return [...text].length;
}

/**
 * The comparison form of a profile name: NFC-normalised and Rust-trimmed.
 * Two enabled profiles whose names agree here are refused, because a
 * creation resolves a profile **by name** — and canonically equivalent
 * spellings render identically in every list, so NFD and NFC spellings of
 * one word are one name, not a near-miss a creating agent stumbles over.
 * Each stored name stays as its owner typed it; only the comparison is
 * normalised.
 */
export function normalizeName(name: string): string {
  return rustTrim(name).normalize("NFC");
}

/**
 * True when enabling (or saving with the tick on) would put two enabled
 * profiles on one name. The daemon refuses such a document; this is the same
 * rule, applied before a write is sent.
 */
export function enabledNameClash(
  profiles: readonly { id: string; name: string; enabledForAgents: boolean }[],
  excludingId: string | null,
  name: string,
): boolean {
  const key = normalizeName(name);
  return profiles.some(
    (profile) =>
      profile.id !== excludingId && profile.enabledForAgents && normalizeName(profile.name) === key,
  );
}

/**
 * What the profile form's fields hold — in both modes. It is the seed the
 * form starts from, the draft an edit reports up, and what a save applies.
 * The feature values travel as one map keyed the way the provider declares
 * them, and the form writes only the keys the provider offered: a stored value
 * the provider no longer offers is dropped on save, silently, as Paseo's
 * `pruneFeatureValues` does. That replaced an earlier rule that kept unknown
 * keys visible and removable, and the reason it went is this: once a control
 * exists per offered feature, an unoffered key has no control, so keeping it
 * alive would leave a value on the profile that the form cannot even show.
 * The daemon prunes on the same rule at the store, so no child is ever created
 * from a key nothing delivers.
 */
export interface ProfileFormSeed {
  name: string;
  /** "" saves none; the daemon stores null. */
  icon: string;
  note: string;
  spawnPrompt: string;
  provider: string;
  model: string;
  modeId: string;
  /** "" means none; the daemon stores null. */
  thinkingOptionId: string;
  /**
   * The profile's feature values, keyed as the provider declares them: a
   * toggle's `true`, a select's chosen option id. Absent means the control is
   * off or unset, which is what a tick written as `false` means to the daemon
   * too — the two spellings are one state, and the map holds only the one of
   * them that asks for something.
   */
  features: Record<string, boolean | string>;
  /** The stored tool overlay, verbatim; the peer tick and removals edit it. */
  overlay: string[];
  enabledForAgents: boolean;
  /**
   * The features the provider offered for this draft's model, as the vocabulary
   * reply answered them — the list the form drew its controls from. It rides on
   * the draft rather than being re-read at save because the answer lives in the
   * form's fetch, and a save must prune by **the same list that was drawn**: a
   * second read could answer differently between the keystroke and the click, and
   * a key dropped by one and never shown by the other is a value that left a
   * profile without anyone being able to see it go.
   *
   * `null` is "no answer in hand" — the query failed, the daemon predates the
   * axis, or an ACP read has not landed — and the save reads it as prune
   * nothing. `[]` is the answer "this provider offers nothing", which prunes
   * everything: the two are different facts and only one of them deletes.
   */
  offeredFeatures: VocabularyFeature[] | null;
}

/** The seed a new profile starts from: empty fields, no provider chosen yet. */
export const EMPTY_PROFILE_FORM_SEED: ProfileFormSeed = {
  name: "",
  icon: "",
  note: "",
  spawnPrompt: "",
  provider: "",
  model: "",
  modeId: "",
  thinkingOptionId: "",
  features: {},
  overlay: [],
  enabledForAgents: false,
  offeredFeatures: null,
};

/**
 * The stored row a form edits, as the fields hold it. `features` is the
 * profile's map read as values: the daemon's own tick is the JSON boolean
 * `true` and nothing else (`profile_delivery.rs`), a select's value is the
 * option id the agent declared, and anything else a document holds came from
 * an older build — it is carried unchanged so the human's next save can let the
 * provider's list decide its fate, rather than this reader guessing one.
 * The overlay is carried verbatim, and the peer tick is on whenever the peer
 * tools are in it — however the overlay came to hold them — so an ordinary
 * edit can never silently drop a restriction.
 */
export function seedFromProfile(profile: AgentProfile): ProfileFormSeed {
  const overlay = profile.toolOverlay ?? [];
  // `features` is skipped on the wire when empty, and profiles older builds
  // wrote never carried the key: absent **is** the empty map. Reading it as
  // if it were always there threw on a live legacy profile and blanked the
  // app, so the default lives here, at the one reader.
  const features = profile.features ?? {};
  const seeded: Record<string, boolean | string> = {};
  for (const [key, value] of Object.entries(features)) {
    if (typeof value === "boolean" || typeof value === "string") {
      seeded[key] = value;
    }
  }
  return {
    name: profile.name,
    icon: profile.icon ?? "",
    note: profile.note,
    spawnPrompt: profile.spawnPrompt ?? "",
    provider: profile.provider,
    model: profile.model,
    modeId: profile.modeId,
    thinkingOptionId: profile.thinkingOptionId ?? "",
    features: seeded,
    overlay: [...overlay],
    enabledForAgents: profile.enabledForAgents,
    // The stored map travels; the offered list is what the form's fetch
    // answers, and an edit that never opens one keeps the profile's own keys.
    offeredFeatures: null,
  };
}

/**
 * Every cap a profile write must fit, checked on the draft before the panel
 * sends anything: the daemon applies exactly these shapes in
 * `agent_profiles.rs`, and the spawn prompt is capped **after** trimming
 * there (`check_profile`), so the same Rust-trimmed bytes are counted here —
 * whitespace the daemon will trim must not refuse a prompt it would store.
 * Refusals are sized in the daemon's own units and nothing is ever
 * truncated.
 */
export function profileDraftRefusal(draft: ProfileFormSeed): string | null {
  const trimmedChars = charCount(rustTrim(draft.name));
  if (trimmedChars === 0) {
    return `A profile name is 1 to ${MAX_PROFILE_NAME_CHARS} characters.`;
  }
  if (trimmedChars > MAX_PROFILE_NAME_CHARS) {
    return `This name is ${trimmedChars} characters, over the ${MAX_PROFILE_NAME_CHARS}-character cap. Nothing was saved and nothing was truncated.`;
  }
  const noteBytes = utf8Bytes(draft.note);
  if (noteBytes > MAX_PROFILE_NOTE_BYTES) {
    return `This note is ${noteBytes} bytes, over the ${MAX_PROFILE_NOTE_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  const spawnBytes = utf8Bytes(rustTrim(draft.spawnPrompt));
  if (spawnBytes > MAX_PROFILE_SPAWN_PROMPT_BYTES) {
    return `This spawn prompt is ${spawnBytes} bytes, over the ${MAX_PROFILE_SPAWN_PROMPT_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  if (rustTrim(draft.model) === "") {
    return "Choose or type a model for the profile.";
  }
  if (rustTrim(draft.modeId) === "") {
    return "Choose or type a mode for the profile.";
  }
  const thinkingBytes = utf8Bytes(rustTrim(draft.thinkingOptionId));
  if (thinkingBytes > MAX_PROFILE_FIELD_BYTES) {
    return `The thinking option id is ${thinkingBytes} bytes, over the ${MAX_PROFILE_FIELD_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  const iconBytes = utf8Bytes(rustTrim(draft.icon));
  if (iconBytes > MAX_PROFILE_ICON_BYTES) {
    return `The icon is ${iconBytes} bytes, over the ${MAX_PROFILE_ICON_BYTES}-byte cap. Nothing was saved and nothing was truncated.`;
  }
  return null;
}

/**
 * The features a form may draw and save: the provider's own list, narrowed to
 * what the profile's current model carries.
 *
 * The whole of "read from the provider, not hard-coded in the form" lives in
 * this function. Nothing here names a provider, a model or a feature: the
 * daemon's answer says which controls exist, which models each one rides, and
 * what a choice is called. A model outside a gated row drops that row from the
 * form — which is also what saves the human from a stale value, since what is
 * not drawn is not written.
 *
 * `null` is the answer-less case, and the two ways to get it are one fact to the
 * caller: no axis at all (a daemon older than the field), or an axis that did not
 * answer (`none` is an answer — an empty list; `absent` is not). The form passes
 * the result straight to `profileFeaturesFromDraft`, so the reader of one value
 * decides both what is drawn and whether stale keys may go.
 */
export function offeredFeatures(
  vocabulary: ProviderVocabulary | null,
  model: string,
): VocabularyFeature[] | null {
  const axis = vocabulary?.features;
  if (!axis) {
    return null;
  }
  if (axis.state === "absent") {
    // Includes the read that is still running: no controls are drawn, and
    // nothing stored is judged away. `featuresAreProbing` says which of the two
    // `absent` answers this is, and the form shows the difference.
    return null;
  }
  const chosen = rustTrim(model);
  return axis.items
    .filter((feature) => (feature.models ? feature.models.some((id) => id === chosen) : true))
    .map((feature) =>
      // The select arms are the only ones with a choice list, and a `select`
      // with no options is a widget that cannot be drawn: the daemon's own
      // builder refuses to make one, so this only guards a malformed frame.
      feature.type === "select" && (feature.options?.length ?? 0) > 0
        ? feature
        : feature.type === "toggle"
          ? feature
          : null,
    )
    .filter((feature): feature is VocabularyFeature => feature !== null);
}

/** Whether the provider's answer says its list is still being read: the ACP
 *  cold start, which the form labels "checking…" instead of drawing nothing in
 *  silence. */
export function featuresAreProbing(vocabulary: ProviderVocabulary | null): boolean {
  return vocabulary?.features?.state === "absent" && vocabulary.features.probing === true;
}

/** Whether the read was **made and could not be answered**: the axis is
 *  `absent` and the daemon is not still reading. The two `absent` answers are
 *  the difference between "the provider has nothing" and "nobody could ask",
 *  and the form says them in different sentences — and a daemon older than the
 *  field is neither, which is why this reads the axis rather than its absence. */
export function featuresAskFailed(vocabulary: ProviderVocabulary | null): boolean {
  const axis = vocabulary?.features;
  return axis !== undefined && axis !== null && axis.state === "absent" && axis.probing !== true;
}

/**
 * The stored `features` map the draft saves.
 *
 * With an answer in hand (`offered` is the drawn list), exactly those keys are
 * written, in the provider's own order, each with a value its control could
 * produce: a toggle writes its key when it is on and nothing when it is off —
 * the rule `autoAccept` already followed, now every toggle's, because `false`
 * and absent are one state to the daemon and storing both spellings would be two
 * ways to say one thing. A select writes its chosen option id, and nothing when
 * the human chose the empty option. **A stored key the provider no longer offers
 * is therefore dropped here, silently, on save** — Paseo's
 * `pruneFeatureValues`, and the reason the form no longer shows removable
 * read-only rows: with one control per offered feature an unoffered key has no
 * control, and keeping it alive would store a value the form cannot display.
 *
 * With no answer (`offered === null`: the query failed, the daemon predates the
 * axis, or an ACP read has not landed) nothing may be judged away, so the
 * stored values pass through and only a human's own edit changes them. Pruning
 * against an unknown list would delete a provider-authored value the daemon has
 * simply not learned to name — the difference between "offers nothing" and
 * "nobody could ask" that the whole vocabulary surface is built on.
 *
 * `Object.fromEntries` defines own properties, so even a stored key named
 * `__proto__` stays data.
 */
export function profileFeaturesFromDraft(
  draft: ProfileFormSeed,
  offered: readonly VocabularyFeature[] | null,
): Record<string, unknown> {
  if (offered === null) {
    return { ...draft.features };
  }
  const saved: Record<string, unknown> = {};
  for (const feature of offered) {
    const value = draft.features[feature.id];
    if (value === undefined || value === false || value === "") {
      continue;
    }
    saved[feature.id] = value;
  }
  return saved;
}
