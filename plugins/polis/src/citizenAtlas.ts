import { Container, Graphics } from "pixi.js";
import type { Texture } from "pixi.js";
import { defaultTunic, drawCitizen, type CitizenDrawOpts, type CitizenType } from "./kitcd/people";
import type { CityAgentState } from "./model";
import { BuildingTextureAtlas, type TextureSource } from "./buildingAtlas";
import { MAX_ZOOM } from "./camera";

/** Eight baked phase steps are enough to preserve the v1 cadence without
 * retaining one mutable Graphics tree per agent. */
export const CITIZEN_PHASE_STEPS = 8;
export type CitizenPhaseStep = 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7;

/** Placement scale from the v1 AgentLayer: people.ts is authored at ~23px tall. */
export const CITIZEN_FIGURE_SCALE = 0.55;
/** Citizen art is small; bake it at the highest interactive scale and let the
 * renderer reduce it at ordinary zooms instead of magnifying a tiny texture.
 *
 * The zoom is only half of it. The application renders at
 * `min(devicePixelRatio, 2)`, so at maximum zoom a citizen occupies
 * `MAX_ZOOM * devicePixelRatio` physical pixels per authored unit. Baking at
 * MAX_ZOOM alone therefore handed the GPU a texture that had to be magnified by
 * the pixel ratio — 25% on this machine — which is exactly the softness you see
 * when you get close. Memory grows with the square of this number, so it is
 * capped at 2 the same way the renderer caps its own resolution. */
export const CITIZEN_BAKE_RESOLUTION = MAX_ZOOM * Math.min(globalThis.devicePixelRatio ?? 1, 2);

const WALK_PHASE_INCREMENT = 0.6;
const ACTION_PHASE_INCREMENT = 0.5;

/** Provider identity is structural: the tunic is the source type's own colour. */
export function citizenTypeForProvider(provider: string): CitizenType {
  switch (provider.trim().toLowerCase()) {
    case "claude":
      return "noble";
    case "codex":
      return "builder";
    case "grok":
      return "foreigner";
    case "pi":
      return "watercarrier";
    case "copilot":
      return "priest";
    default:
      return "citizen";
  }
}

export interface CitizenDrawParams extends Pick<
  CitizenDrawOpts,
  "moving" | "phase" | "actionPhase"
> {}

/**
 * Translate renderer state into the existing people.ts contract. Working
 * agents keep the role action active while standing; silent, finished, and
 * idle are still and pass zero action phase, leaving their existing badges as
 * reinforcement.
 */
export function drawParamsForCitizen(
  state: CityAgentState,
  step: CitizenPhaseStep,
): CitizenDrawParams {
  const moving = state === "working";
  return {
    moving,
    phase: moving ? step * WALK_PHASE_INCREMENT : 0,
    actionPhase: state === "working" ? step * ACTION_PHASE_INCREMENT : 0,
  };
}

export function citizenVariantKey(
  type: CitizenType,
  state: CityAgentState,
  step: CitizenPhaseStep,
): string {
  return `citizen:${type}:${state}:s${step}`;
}

export interface CitizenVariant {
  texture: Texture;
  frame: { x: number; y: number; width: number; height: number };
  radius: number;
  type: CitizenType;
  state: CityAgentState;
  step: CitizenPhaseStep;
}

/**
 * Bakes the faithful people.ts drawing into the same owner/cache as buildings.
 * The cache key deliberately has no agent id: all agents sharing a structural
 * type, state, and quantised phase reuse one GPU texture and one short-lived
 * source Graphics tree.
 */
export class CitizenTextureAtlas {
  private readonly variants = new Map<string, CitizenVariant>();

  constructor(private readonly atlas: BuildingTextureAtlas) {}

  get size(): number {
    return this.variants.size;
  }

  has(type: CitizenType, state: CityAgentState, step: CitizenPhaseStep): boolean {
    return this.atlas.hasSprite(citizenVariantKey(type, state, step));
  }

  get(
    renderer: TextureSource,
    type: CitizenType,
    state: CityAgentState,
    step: CitizenPhaseStep,
  ): CitizenVariant {
    const key = citizenVariantKey(type, state, step);
    const hit = this.variants.get(key);
    if (hit !== undefined) return hit;

    const baked = this.atlas.getSprite(
      renderer,
      key,
      () => {
        const body = new Container();
        const graphic = new Graphics();
        const params = drawParamsForCitizen(state, step);
        drawCitizen(graphic, type, {
          ...params,
          tunic: defaultTunic(type),
        });
        body.addChild(graphic);
        body.scale.set(CITIZEN_FIGURE_SCALE);
        return body;
      },
      CITIZEN_BAKE_RESOLUTION,
    );
    const variant = {
      texture: baked.texture,
      frame: baked.frame,
      radius: Math.hypot(baked.frame.width, baked.frame.height) / 2 + 8,
      type,
      state,
      step,
    } satisfies CitizenVariant;
    this.variants.set(key, variant);
    return variant;
  }

  destroy(): void {
    // BuildingTextureAtlas owns and destroys the shared textures. This view is
    // only a cheap index of the citizen keys requested by this layer.
    this.variants.clear();
  }
}
