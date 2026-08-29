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
});
