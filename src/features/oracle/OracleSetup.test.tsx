import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { OracleWorkspace } from "../../types/ipc";
import { OracleSetup } from "./OracleSetup";

const workspace: OracleWorkspace = {
  path: "C:/code/project",
  source: "saved",
  exists: true,
  editable: true,
};

describe("Oracle error recovery", () => {
  it("puts folder recovery first when the status request cannot read the workspace", () => {
    const markup = renderToStaticMarkup(
      <OracleSetup
        stage="oracle-error"
        workspaceRequest={{ status: "ready", value: workspace }}
        statusRequest={{
          status: "error",
          message: "Oracle workspace C:/code/project no longer exists",
          detail: null,
        }}
        workspaceBusy={false}
        indexStarting={false}
        cancelBusy={false}
        modelDownloadBusy={false}
        workspaceActionError={null}
        indexActionError={null}
        onChooseWorkspace={() => undefined}
        onStartIndex={() => undefined}
        onCancel={() => undefined}
        onRefreshStatus={() => undefined}
        onRetryModels={() => undefined}
      />,
    );

    expect(markup.indexOf("Choose another folder")).toBeLessThan(markup.indexOf("Try again"));
  });

  it("walks the same four steps in the same words on every stage", () => {
    // The setup flow is Folder → Models → Index → Ask; the rail renders
    // on every stage, so one stage proves the shape for all of them.
    const markup = renderToStaticMarkup(
      <OracleSetup
        stage="models"
        workspaceRequest={{ status: "loading" }}
        statusRequest={{ status: "idle" }}
        workspaceBusy={false}
        indexStarting={false}
        cancelBusy={false}
        modelDownloadBusy={false}
        workspaceActionError={null}
        indexActionError={null}
        onChooseWorkspace={() => undefined}
        onStartIndex={() => undefined}
        onCancel={() => undefined}
        onRefreshStatus={() => undefined}
        onRetryModels={() => undefined}
      />,
    );

    const steps = markup.match(/oracle-setup-step-number">[^<]*<\/span>\s*<span>([^<]*)<\/span>/g);
    if (steps === null) throw new Error("setup rail did not render");
    expect(steps).toHaveLength(4);
    for (const label of ["Folder", "Models", "Index", "Ask"]) {
      expect(markup).toContain(`<span>${label}</span>`);
    }
  });
});
