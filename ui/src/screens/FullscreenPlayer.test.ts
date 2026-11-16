import { describe, expect, it, vi } from "vitest";
import playerSource from "./FullscreenPlayer.svelte?raw";
import { render } from "svelte/server";
import { t } from "$lib/i18n";

// The player reads the window's height when it opens.
vi.stubGlobal("window", { innerHeight: 600, addEventListener() {}, removeEventListener() {} });
const { default: FullscreenPlayer } = await import("./FullscreenPlayer.svelte");

const html = (soft: boolean) => render(FullscreenPlayer, { props: { island: soft } }).body;
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

  it("has a close button back to the island's Home inside Droplet Island only", () => {
    const inIsland = html(true);
    expect(inIsland).toContain("data-island-close");
    expect(inIsland).toContain(`aria-label="${t.island.back}"`);
    // The collapse button's twin: same capsule, mirrored to the right.
    const tag = inIsland.slice(inIsland.lastIndexOf("<button", inIsland.indexOf("data-island-close")), inIsland.indexOf(">", inIsland.indexOf("data-island-close")));
    expect(tag).toContain("top-4 right-4");
    // 32 px: whole device pixels at the island's 0.875 scale, like the collapse button there.
    expect(tag).toContain("size-8");
    expect(inIsland.match(/top-4 left-4 z-10 grid size-8/)).not.toBeNull();
    expect(tag).toContain("rounded-full bg-white/10");
    expect(html(false)).not.toContain("data-island-close");
    // As a window of its own the collapse button keeps its size.
    expect(html(false)).toContain("top-4 left-4 z-10 grid size-9");
  });

  it("breathes its edges slowly, out of step, and holds them still with reduced motion", () => {
    const src = playerSource;
    const css = src.slice(src.indexOf("<style>"));
    // Narrow fades: 24–32 px at the sides, 36–44 px at the bottom.
    expect(css).toMatch(/@keyframes breathe-side\s*\{\s*from\s*\{\s*--side: 24px;\s*\}\s*to\s*\{\s*--side: 32px;/);
    expect(css).toMatch(/@keyframes breathe-foot\s*\{\s*from\s*\{\s*--foot: 36px;\s*\}\s*to\s*\{\s*--foot: 44px;/);
    // Long, different periods, so the sides and the bottom never pulse together.
    expect(css).toContain("breathe-side 10s ease-in-out infinite alternate");
    expect(css).toContain("breathe-foot 13s ease-in-out -4s infinite alternate");
    expect(css).toMatch(/\.soft-edges\.still\s*\{\s*animation-play-state: paused;/);
    expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\)\s*\{\s*\.soft-edges\s*\{\s*animation: none;/);
  });
});
