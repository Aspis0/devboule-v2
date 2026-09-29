import { describe, expect, it } from "vitest";
import { terminalKeyPolicy } from "./terminalKeyPolicy";

const nothingSelected = () => false;
const somethingSelected = () => true;

describe("terminalKeyPolicy", () => {
  it("passes ordinary keys through", () => {
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: false, key: "a" }, nothingSelected, false),
    ).toBe("pass");
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: false, key: "c" }, somethingSelected, false),
    ).toBe("pass");
  });

  it("maps Ctrl+Shift+C to copy", () => {
    expect(
      terminalKeyPolicy(
        { type: "keydown", ctrlKey: true, shiftKey: true, key: "C" },
        nothingSelected,
        false,
      ),
    ).toBe("copy");
    expect(
      terminalKeyPolicy(
        { type: "keyup", ctrlKey: true, shiftKey: true, key: "C" },
        nothingSelected,
        false,
      ),
    ).toBe("swallow");
  });

  it("leaves Ctrl+Shift+V to the browser's own paste road", () => {
    expect(
      terminalKeyPolicy(
        { type: "keydown", ctrlKey: true, shiftKey: true, key: "V" },
        nothingSelected,
        false,
      ),
    ).toBe("pass");
    expect(
      terminalKeyPolicy(
        { type: "keyup", ctrlKey: true, shiftKey: true, key: "V" },
        nothingSelected,
        false,
      ),
    ).toBe("pass");
  });

  it("copies on plain Ctrl+C while text is selected (Windows and Linux)", () => {
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: true, key: "c" }, somethingSelected, false),
    ).toBe("copy");
  });

  it("arms the two-step interrupt on plain Ctrl+C with no selection", () => {
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: true, key: "c" }, nothingSelected, false),
    ).toBe("interrupt");
    expect(
      terminalKeyPolicy({ type: "keyup", ctrlKey: true, key: "c" }, nothingSelected, false),
    ).toBe("swallow");
  });

  it("keeps plain Ctrl+C as the interrupt on macOS whatever is selected", () => {
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: true, key: "c" }, somethingSelected, true),
    ).toBe("interrupt");
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: true, key: "c" }, nothingSelected, true),
    ).toBe("interrupt");
  });

  it("leaves Command keys on macOS's own copy/paste road", () => {
    expect(
      terminalKeyPolicy(
        { type: "keydown", ctrlKey: false, metaKey: true, key: "c" },
        somethingSelected,
        true,
      ),
    ).toBe("pass");
    expect(
      terminalKeyPolicy(
        { type: "keydown", ctrlKey: true, metaKey: true, key: "c" },
        somethingSelected,
        true,
      ),
    ).toBe("pass");
    expect(
      terminalKeyPolicy(
        { type: "keydown", ctrlKey: false, metaKey: true, key: "v" },
        nothingSelected,
        true,
      ),
    ).toBe("pass");
    expect(
      terminalKeyPolicy(
        { type: "keyup", ctrlKey: false, metaKey: true, key: "c" },
        somethingSelected,
        true,
      ),
    ).toBe("pass");
  });

  it("passes AltGr and non-Ctrl variants through", () => {
    expect(
      terminalKeyPolicy(
        { type: "keydown", ctrlKey: true, altKey: true, key: "c" },
        somethingSelected,
        false,
      ),
    ).toBe("pass");
    expect(
      terminalKeyPolicy(
        { type: "keydown", ctrlKey: true, altKey: true, shiftKey: true, key: "C" },
        nothingSelected,
        false,
      ),
    ).toBe("pass");
  });
});
