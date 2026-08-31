import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { PluginSurface } from "./PluginSurface";

describe("PluginSurface", () => {
  it("loads an HTML entry document in a foreign-origin iframe", () => {
    const markup = renderToStaticMarkup(
      <PluginSurface
        pluginId="polis"
        entry="ui/index.html"
        assetOrigin="http://plugin.localhost/"
        capabilities={["oracle.search"]}
      />,
    );

    expect(markup).toContain('src="http://plugin.localhost/polis/ui/index.html"');
    expect(markup).toContain('sandbox="allow-scripts allow-same-origin"');
  });

  it("renders a readable failure instead of an iframe for a missing entry", () => {
    const markup = renderToStaticMarkup(
      <PluginSurface
        pluginId="polis"
        entry={null}
        assetOrigin="http://plugin.localhost"
        capabilities={[]}
      />,
    );

    expect(markup).toContain("did not declare a UI entry path");
    expect(markup).not.toContain("<iframe");
  });
});
