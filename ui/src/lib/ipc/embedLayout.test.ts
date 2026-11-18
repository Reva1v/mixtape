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

describe("the embedded search header", () => {
  it("starts 24 px down, like its left side", () => {
    expect(rule(".in-island-main .search-head")).toContain("padding-top: 24px");
  });

});

describe("the embedded header rows with buttons", () => {
  // Close: 24 px in, 32 px wide; arrows: 56 px in, two 32 px buttons and a 2 px gap; 8 px of air. The rows already
  // have the 24 px side padding, so the margin is the rest.
  const SIDE = 24;
  it("end left of the close button", () => {
    expect(rule(".in-island-main :is(.search-head, div.page-head)")).toContain(`margin-inline-end: ${24 + 32 + 8 - SIDE}px`);
  });

  it("end left of the history arrows when they show", () => {
    expect(rule(".in-island-main[data-history] :is(.search-head, div.page-head)")).toContain(
      `margin-inline-end: ${56 + 66 + 8 - SIDE}px`,
    );
    expect(app).toContain('data-history={embedded && (router.canBack || router.canForward) ? "" : undefined}');
  });

  it("cover every page whose header row has buttons", () => {
    for (const page of ["Library", "Stats", "Downloads"]) {
      const src = readFileSync(new URL(`../../screens/${page}.svelte`, import.meta.url), "utf8");
      expect(src, page).toMatch(/<div class="page-head /);
    }
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
    expect(rule(".in-island-main .search-head")).toContain("padding-top: 24px");
  });

  it("apply only inside the island's main", () => {
    expect(app).toContain("{embedded ? 'in-island-main' : ''}");
  });
});
