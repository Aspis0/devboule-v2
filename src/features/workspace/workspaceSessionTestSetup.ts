import { sharedSessionController } from "./workspaceSessions";

// Fixture birth stamps keep membership setup from triggering identity-repair IPC.
export function openListedSessionsForTest(): void {
  const controller = sharedSessionController();
  const seed = () => {
    const state = controller.getState();
    if (state.loading || state.error !== null) return;
    release();
    const fixtures = state.sessions.filter((session) => session.state.type !== "ended");
    for (const session of fixtures)
      controller.open({ ...session, createdAtMs: session.createdAtMs ?? 1 });
    controller.select(fixtures[0]?.id ?? null);
  };
  const release = controller.subscribe(seed);
}
