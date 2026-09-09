import { create } from "zustand";
import { pluginInstall, pluginsList, pluginsRescan, reasonFromCause } from "../lib/tauri";
import type { DesignDocument, DesignHost, DesignMessage } from "../features/design/designHost";
import type { PluginInventory } from "../types/ipc";
import type { SurfaceKey } from "../types/surface";

export interface DesignArtifact {
  html?: string;
  error?: string;
}

export interface DesignGenerationState {
  // The assistant message this run is writing into. It is the run's identity: every guard
  // compares against it, so there is deliberately no second field that could disagree.
  assistantId: string;
  controller: AbortController;
}

export interface DesignSessionState {
  host: DesignHost | null;
  document: DesignDocument | null;
  messages: DesignMessage[];
  latestArtifact: DesignArtifact | null;
  generation: DesignGenerationState | null;
}

function emptyDesignSession(host: DesignHost | null = null): DesignSessionState {
  return {
    host,
    document: null,
    messages: [],
    latestArtifact: null,
    generation: null,
  };
}

// `status === "done"` is load-bearing, not defensive. A message still `working` can carry a
// half-streamed fence, and this value is what the canvas renders, what a preview elsewhere
// mirrors, and what the render critic measures. Measuring incomplete markup would produce
// findings that vanish when the turn finishes, and a check whose findings come and go is one
// people learn to ignore. This is the only definition: the surface reads it rather than
// computing its own, because two definitions can disagree about what the current artifact is.
function latestArtifact(messages: readonly DesignMessage[]): DesignArtifact | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (
      message?.role === "assistant" &&
      message.status === "done" &&
      (message.artifactHtml !== undefined || message.artifactError !== undefined)
    ) {
      return { html: message.artifactHtml, error: message.artifactError };
    }
  }
  return null;
}

type DesignMessagesUpdate =
  | readonly DesignMessage[]
  | ((messages: readonly DesignMessage[]) => readonly DesignMessage[]);

export interface InstalledSkill {
  id: string;
  name: string;
  author: string;
  description: string;
}

interface AppState {
  activeSurface: SurfaceKey;
  selectSurface: (surface: SurfaceKey) => void;

  installedSkills: InstalledSkill[];
  installSkill: (skill: InstalledSkill) => void;

  /**
   * The live Design session is shared because its surface is intentionally
   * mounted only while Design is selected.
   */
  designSession: DesignSessionState;
  setDesignHost: (host: DesignHost) => void;
  setDesignDocument: (
    host: DesignHost,
    document: DesignDocument,
    messages: readonly DesignMessage[],
  ) => void;
  setDesignMessages: (host: DesignHost, update: DesignMessagesUpdate) => void;
  setDesignGeneration: (host: DesignHost, generation: DesignGenerationState | null) => void;
  clearDesignSession: (host?: DesignHost) => void;

  /**
   * What discovery last reported. `null` means nobody has asked yet, which is
   * not the same as "nothing is installed" and must not be drawn as if it were.
   */
  plugins: PluginInventory | null;
  /** The id of the plugin whose install is in flight, if any. */
  installing: string | null;
  /** Why the last install did not happen. Cleared by success, refresh, or dismissal. */
  installError: string | null;
  dismissInstallError: () => void;

  refreshPlugins: (again?: boolean) => Promise<void>;
  installPlugin: (id: string, source: string) => Promise<boolean>;
}

/**
 * One inventory for the whole app.
 *
 * Both the navigation and the Polis surface need to know whether Polis is
 * installed. Two independent fetches would be two answers that can disagree —
 * the crescent still offering a `+` for something the surface already shows as
 * installed — so there is one, here.
 */
export const useAppStore = create<AppState>((set) => ({
  activeSurface: "workspace",
  selectSurface: (activeSurface) => set({ activeSurface }),

  designSession: emptyDesignSession(),
  setDesignHost: (host) =>
    set((state) =>
      state.designSession.host === host ? state : { designSession: emptyDesignSession(host) },
    ),
  setDesignDocument: (host, document, messages) =>
    set((state) => {
      if (state.designSession.host !== host) return state;
      const nextMessages = [...messages];
      return {
        designSession: {
          ...state.designSession,
          document,
          messages: nextMessages,
          latestArtifact: latestArtifact(nextMessages),
        },
      };
    }),
  setDesignMessages: (host, update) =>
    set((state) => {
      if (state.designSession.host !== host) return state;
      const nextMessages = [
        ...(typeof update === "function" ? update(state.designSession.messages) : update),
      ];
      return {
        designSession: {
          ...state.designSession,
          messages: nextMessages,
          latestArtifact: latestArtifact(nextMessages),
        },
      };
    }),
  setDesignGeneration: (host, generation) =>
    set((state) =>
      state.designSession.host !== host
        ? state
        : { designSession: { ...state.designSession, generation } },
    ),
  clearDesignSession: (host) =>
    set((state) =>
      host !== undefined && state.designSession.host !== host
        ? state
        : { designSession: emptyDesignSession() },
    ),
  installedSkills: [],
  installSkill: (skill) =>
    set((state) =>
      state.installedSkills.some((installed) => installed.id === skill.id)
        ? state
        : { installedSkills: [...state.installedSkills, skill] },
    ),

  plugins: null,
  installing: null,
  installError: null,
  dismissInstallError: () => set({ installError: null }),

  refreshPlugins: async (again = false) => {
    try {
      const inventory = again ? await pluginsRescan() : await pluginsList();
      set((state) => ({
        plugins: inventory,
        // A successful rescan is the acknowledgement that an install now
        // exists on disk. Do not leave its old failure over a verified plugin.
        installError: inventory.plugins.some((plugin) => plugin.ready) ? null : state.installError,
      }));
    } catch (cause) {
      // The command reports "I could not look" inside the inventory, so a
      // rejection means the app did not answer at all. Same shape either way:
      // one thing for the interface to render, and never silence.
      set({ plugins: { root: "", plugins: [], problem: reasonFromCause(cause) } });
    }
  },

  installPlugin: async (id, source) => {
    set({ installing: id, installError: null });
    try {
      // The command verifies before it puts anything in place, so a plugin that
      // arrives here is one that passed; there is no half-installed state to
      // render.
      set({ plugins: await pluginInstall(id, source), installing: null });
      return true;
    } catch (cause) {
      set({ installing: null, installError: reasonFromCause(cause) });
      return false;
    }
  },
}));
