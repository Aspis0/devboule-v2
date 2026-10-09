import type { SessionModel } from "../../types/ipc";

/** A model's label: the provider that serves it, then the model. One model name
 * can be served by several providers, so the provider is part of what is read. */
export function modelLabel(model: Pick<SessionModel, "name" | "providerId">): string {
  return model.providerId === undefined || model.providerId === ""
    ? model.name
    : `${model.providerId} · ${model.name}`;
}
