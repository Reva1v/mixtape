import { describe, expect, it } from "vitest";

import app from "../../App.svelte?raw";
// The css plugin rewrites a css ?raw import, so the source file is read from disk (no node types here).
// @ts-expect-error node:fs has no type definitions in this project
const { readFileSync } = (await import("node:fs")) as { readFileSync: (path: URL, enc: string) => string };
const css = readFileSync(new URL("../../app.css", import.meta.url), "utf8");

const rule = (selector: string) => {
  const flat = css.replace(/\s+/g, " ");
  const at = flat.indexOf(`${selector} {`);
  expect(at, selector).toBeGreaterThan(-1);
  return flat.slice(at, flat.indexOf("}", at));
};

describe("the embedded close button's place", () => {
  it("is 32 px, 24 px from the top and 24 px from the content panel's right edge, like the island's phone view", () => {
    expect(app).toContain('class="absolute top-6 right-6 z-20 flex h-8 items-center"');
    expect(app).toContain("grid h-8 w-8 cursor-pointer place-items-center");
  });

  it("keeps the history arrows just left of it embedded, and the window layout unchanged", () => {
    expect(app).toContain("embedded ? 'top-6 right-14 h-8' : 'top-1 right-[140px] h-10'");
  });
});

describe("the embedded pages' paddings", () => {
  it("have 24 px sides, scrollers included", () => {
    expect(rule(".in-island-main :is(.px-8, .px-7)")).toContain("padding-inline: 24px");
    expect(rule(".in-island-main .scroll-px-8")).toContain("scroll-padding-inline: 24px");
  });

  it("start the page header 22 px below the panel top, with no leftover shift", () => {
    expect(rule(".in-island-main :is(.pt-9, .pt-10)")).toContain("padding-top: 22px");
    expect(css).not.toContain("margin-top: -4px");
  });

  it("put the header row's middle line at 40 px", () => {
    expect(rule(".in-island-main .page-head")).toContain("min-height: 36px");
    expect(rule(".in-island-main h1.page-head")).toContain("line-height: 36px");
    expect(rule(".in-island-main .search-head")).toContain("padding-top: 16px");
  });

  it("apply only inside the island's main", () => {
    expect(app).toContain("{embedded ? 'in-island-main' : ''}");
  });
});
