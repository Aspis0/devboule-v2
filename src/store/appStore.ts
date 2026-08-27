import { create } from "zustand";
import type { SurfaceKey } from "../types/surface";

interface AppState {
  activeSurface: SurfaceKey;
  selectSurface: (surface: SurfaceKey) => void;
}

export const useAppStore = create<AppState>((set) => ({
  activeSurface: "workspace",
  selectSurface: (activeSurface) => set({ activeSurface }),
}));
