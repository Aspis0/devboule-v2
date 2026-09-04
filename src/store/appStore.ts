import { create } from "zustand";
import { pluginInstall, pluginsList, pluginsRescan, reasonFromCause } from "../lib/tauri";
import type { PluginInventory } from "../types/ipc";
import type { SurfaceKey } from "../types/surface";

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
