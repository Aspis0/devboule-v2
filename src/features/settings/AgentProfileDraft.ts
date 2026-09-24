/**
 * The agent-profile draft: what the form's fields hold in both modes, the
 * stored row converted into it, and every rule a save must satisfy before
 * the panel sends anything. The rules mirror `agent_profiles.rs` exactly —
 * the same caps, the same trim set, the same NFC name comparison — so a
 * draft the form accepts is a document the store admits. The form component
 * (`AgentProfileForm.tsx`) owns the fields; the panel owns the document.
 */
import type { AgentProfile } from "../../types/ipc";

/** The profile store's caps, the daemon's own constants mirrored. */
const MAX_PROFILE_NAME_CHARS = 60;
const MAX_PROFILE_NOTE_BYTES = 2 * 1024;
const MAX_PROFILE_ICON_BYTES = 64;
/** `MAX_PROFILE_SPAWN_PROMPT_BYTES` in `agent_profiles.rs`. */
export const MAX_PROFILE_SPAWN_PROMPT_BYTES = 8 * 1024;
/** `MAX_PROFILE_FIELD_BYTES` in `agent_profiles.rs`: ids like the thinking option. */
const MAX_PROFILE_FIELD_BYTES = 128;
/** `AUTO_ACCEPT_FEATURE` in `provider_catalog.rs`: the one feature the daemon interprets. */
const AUTO_ACCEPT_FEATURE = "autoAccept";

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
 * Stored features beyond the daemon's own tick and the stored tool overlay
 * travel **as saved**: the form can remove entries but never edit or invent
 * them, because the spawn path delivers none of them and an editable
 * key/value pair would promise a pass-through that never happens.
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
  /** The one feature the daemon interprets and delivers, read the way the daemon reads it. */
  autoAccept: boolean;
  /** Stored features beyond the tick, as saved — read-only rows, each removable. */
  storedFeatures: { key: string; value: unknown }[];
  /** The stored tool overlay, verbatim; the peer tick and removals edit it. */
  overlay: string[];
  enabledForAgents: boolean;
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
  autoAccept: false,
  storedFeatures: [],
  overlay: [],
  enabledForAgents: false,
};

/**
 * The stored row a form edits, as the fields hold it. `autoAccept` is the
 * JSON boolean `true` and nothing else — the daemon's own reading
 * (`profile_delivery.rs`) — and it is the ONLY feature the checkbox writes;
 * every stored key beyond it keeps its saved value untouched. The overlay is
 * carried verbatim, and the peer tick is on whenever the peer tools are in
 * it — however the overlay came to hold them — so an ordinary edit can never
 * silently drop a restriction.
 */
export function seedFromProfile(profile: AgentProfile): ProfileFormSeed {
  const overlay = profile.toolOverlay ?? [];
  return {
    name: profile.name,
    icon: profile.icon ?? "",
    note: profile.note,
    spawnPrompt: profile.spawnPrompt ?? "",
    provider: profile.provider,
    model: profile.model,
    modeId: profile.modeId,
    thinkingOptionId: profile.thinkingOptionId ?? "",
    autoAccept: profile.features.autoAccept === true,
    storedFeatures: Object.entries(profile.features)
      .filter(([key]) => key !== AUTO_ACCEPT_FEATURE)
      .map(([key, value]) => ({ key, value })),
    overlay: [...overlay],
    enabledForAgents: profile.enabledForAgents,
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
 * The stored `features` map the draft saves: the tick the daemon interprets,
 * written from the checkbox and from nowhere else, plus every kept stored
 * key with its saved value untouched. `Object.fromEntries` defines own
 * properties, so even a stored key named `__proto__` stays data.
 */
export function profileFeaturesFromDraft(draft: ProfileFormSeed): Record<string, unknown> {
  return Object.fromEntries([
    ...(draft.autoAccept ? [[AUTO_ACCEPT_FEATURE, true as unknown]] : []),
    ...draft.storedFeatures.map((feature) => [feature.key, feature.value]),
  ]);
}
