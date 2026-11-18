import { describe, expect, it } from "vitest";

import app from "../../App.svelte?raw";

describe("the embedded close button's place", () => {
  it("is 32 px, 34 px from the top and 24 px from the content panel's right edge, like the island's phone view", () => {
    expect(app).toContain('class="absolute top-[34px] right-6 z-20 flex h-8 items-center"');
    expect(app).toContain("grid h-8 w-8 cursor-pointer place-items-center");
  });

  it("keeps the history arrows just left of it embedded, and the window layout unchanged", () => {
    expect(app).toContain("embedded ? 'top-[34px] right-14 h-8' : 'top-1 right-[140px] h-10'");
  });
});
