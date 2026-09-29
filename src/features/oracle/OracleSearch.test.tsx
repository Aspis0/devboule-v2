// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OracleSearch } from "./OracleSearch";

describe("Oracle search draft", () => {
  it("renders the parent-owned question text after the search surface is mounted again", () => {
    const markup = renderToStaticMarkup(
      <OracleSearch
        query="where is the workspace root resolved?"
        onQueryChange={() => undefined}
        searchState={{ status: "idle" }}
        submittedQuery={null}
        stats={null}
        indexIsEmpty={false}
        reranker={null}
        onSearch={() => undefined}
        onRetryReranker={() => undefined}
      />,
    );

    expect(markup).toContain('value="where is the workspace root resolved?"');
  });
});

// Enter during an IME composition is the candidate list's key: it must
// not reach the search, and the typed question stays untouched.
describe("Oracle search Enter handling", () => {
  let container: HTMLDivElement;
  let root: Root;

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it("leaves Enter to an open IME composition instead of searching", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    const onSearch = vi.fn();
    await act(async () => {
      root.render(
        <OracleSearch
          query="where is the workspace root resolved?"
          onQueryChange={() => undefined}
          searchState={{ status: "idle" }}
          submittedQuery={null}
          stats={null}
          indexIsEmpty={false}
          reranker={null}
          onSearch={onSearch}
          onRetryReranker={() => undefined}
        />,
      );
    });
    const field = container.querySelector<HTMLInputElement>(
      'input[aria-label="Ask Oracle a question"]',
    );
    if (field === null) throw new Error("oracle question input missing");

    await act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
          isComposing: true,
        }),
      );
    });
    expect(onSearch).not.toHaveBeenCalled();
    expect(field.value).toBe("where is the workspace root resolved?");

    // Older engines report the composition commit as keyCode 229 alone.
    await act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
          keyCode: 229,
        }),
      );
    });
    expect(onSearch).not.toHaveBeenCalled();
    expect(field.value).toBe("where is the workspace root resolved?");

    // Composition closed: the next Enter searches, as it always has.
    await act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
      );
    });
    expect(onSearch).toHaveBeenCalledTimes(1);
    expect(onSearch).toHaveBeenCalledWith("where is the workspace root resolved?");
  });
});
