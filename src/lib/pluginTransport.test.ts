import { describe, expect, it } from "vitest";
import { describePluginTransport, PLUGIN_ORIGINS, probePluginTransport } from "./pluginTransport";

describe("probePluginTransport", () => {
  it("reports the origin that actually served the module", async () => {
    const tried: string[] = [];
    const transport = await probePluginTransport(async (url) => {
      tried.push(url);
      return { pluginTransportWorks: true };
    });
    expect(transport.works).toBe(true);
    expect(transport.origin).toBe(PLUGIN_ORIGINS[0]);
    expect(tried).toEqual([`${PLUGIN_ORIGINS[0]}/__selftest.js`]);
    expect(describePluginTransport(transport)).toContain("loads from");
  });

  it("falls through to the other platform's origin instead of guessing", async () => {
    const transport = await probePluginTransport(async (url) => {
      if (url.startsWith(PLUGIN_ORIGINS[0])) throw new Error("not this platform");
      return { pluginTransportWorks: true };
    });
    expect(transport.works).toBe(true);
    expect(transport.origin).toBe(PLUGIN_ORIGINS[1]);
  });

  it("keeps the browser's own words when every origin fails", async () => {
    const transport = await probePluginTransport(async () => {
      throw new Error("blocked by Content Security Policy");
    });
    expect(transport.works).toBe(false);
    expect(transport.reason).toContain("Content Security Policy");
    expect(describePluginTransport(transport)).toContain("cannot load");
  });

  it("treats a module without the expected export as a failure", async () => {
    // A handler that serves the wrong file, or an HTML error page typed as
    // JavaScript, would otherwise look like success.
    const transport = await probePluginTransport(async () => ({ somethingElse: 1 }));
    expect(transport.works).toBe(false);
    expect(transport.reason).toContain("without the expected export");
  });
});
