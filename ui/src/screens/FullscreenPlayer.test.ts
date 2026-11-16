import { describe, expect, it, vi } from "vitest";
import { render } from "svelte/server";

// The player reads the window's height when it opens.
vi.stubGlobal("window", { innerHeight: 600, addEventListener() {}, removeEventListener() {} });
const { default: FullscreenPlayer } = await import("./FullscreenPlayer.svelte");

const html = (soft: boolean) => render(FullscreenPlayer, { props: { soft } }).body;
/** The opening tag of the player's root element. */
const root = (out: string) => out.slice(out.indexOf("<div"), out.indexOf(">", out.indexOf("<div")));

describe("the full-screen player", () => {
  it("fades its backdrop into the island's black inside Droplet Island", () => {
    const out = html(true);
    expect(out).toContain("soft-edges");
    expect(out).toContain("data-soft");
    // Under the fade: the island's black, not the grey, and nothing of the app below.
    expect(root(out)).not.toContain("bg-[#070709]");
    expect(root(out)).toContain("bg-black");
  });

  it("keeps its hard-edged backdrop as a window of its own", () => {
    const out = html(false);
    expect(out).not.toContain("soft-edges");
    expect(out).not.toContain("data-soft");
    expect(root(out)).toContain("bg-[#070709]");
  });
});
