import type { DesignDocument, DesignInitialState } from "./designHost";

/**
 * The document defaults the Design surface reads on every render. They used
 * to live inside the demo host's fixture module, which made them look like
 * mock content; they are not — the agent host spreads them into every real
 * document it loads, and the surface reads them (composer context prefix
 * and placeholders, canvas initial state, grounding default).
 *
 * Values are copied verbatim from what the surface showed before the demo
 * host was removed, so this transplant changes no visible behaviour.
 *
 * Note: `draftPlaceholder` names a fixture layer ("Index header"). It is
 * dead data kept only because the document shape requires it: the surface
 * derives the composer placeholder from the actual selection instead (see
 * the `draftPlaceholder` call site in DesignSurface), so this string never
 * reaches the user.
 */
export interface DesignDocumentDefaults {
  contextPrefix: DesignDocument["contextPrefix"];
  draftPlaceholder: DesignDocument["draftPlaceholder"];
  noContextPlaceholder: DesignDocument["noContextPlaceholder"];
  initialState: DesignInitialState;
  grounded: DesignDocument["grounded"];
}

export function createDesignDocumentDefaults(): DesignDocumentDefaults {
  return {
    contextPrefix: "Editing",
    draftPlaceholder: "Describe the change to Index header…",
    noContextPlaceholder: "Describe what to generate…",
    initialState: {
      zoom: 1,
      saved: false,
      draft: "",
      hiddenLayerIds: [],
    },
    grounded: true,
  };
}
