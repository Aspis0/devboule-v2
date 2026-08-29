import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
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
