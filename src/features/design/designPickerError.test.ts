import { describe, expect, it } from "vitest";
import { lastErrorText } from "./designPickerError";

const message = (role: string, text: string) => ({ role, text }) as never;

describe("the picker's last error line", () => {
  it("answers the most recent error item's text", () => {
    const items = [message("error", "first"), message("user", "hi"), message("error", "second")];
    expect(lastErrorText(items)).toBe("second");
  });

  it("answers null when the transcript holds no error, without a fixed scan order", () => {
    expect(lastErrorText([message("user", "hi"), message("assistant", "yo")])).toBeNull();
    expect(lastErrorText([])).toBeNull();
  });
});
