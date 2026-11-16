import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import MiniPlayer from "./MiniPlayer.svelte";
import { t } from "$lib/i18n";

const html = (compact: boolean) => render(MiniPlayer, { props: { compact } }).body;
const label = (s: string) => `aria-label="${s}"`;

describe("the bottom player bar", () => {
  it("renders inside Droplet Island (compact), without the window-sized extras", () => {
    const out = html(true);
    expect(out).toContain("data-bar");
    expect(out).toContain("data-compact");
    expect(out).toContain(label(t.player.queue));
    expect(out).not.toContain(label(t.tray.miniPlayer));
    expect(out).not.toContain(label(t.player.fullscreen));
    // Shuffle and repeat go on the narrowest frames, by container width.
    expect(out).toContain("@max-[460px]:hidden");
  });

  it("lays the compact bar out in three columns: title left, transport centred, capsule right", () => {
    const out = html(true);
    expect(out).toContain("compact-grid");
    expect(out).toContain("data-transport");
    expect(out).toContain("justify-self-end");
    // Without a track the left column is still there, so the transport stays in the middle one.
    const grid = out.slice(out.indexOf("compact-grid"));
    expect(grid.indexOf("<div></div>")).toBeGreaterThan(-1);
    expect(grid.indexOf("<div></div>")).toBeLessThan(grid.indexOf("data-transport"));
  });

  it("renders outside the island too, with every control", () => {
    const out = html(false);
    expect(out).toContain("data-bar");
    expect(out).not.toContain("data-compact");
    expect(out).toContain(label(t.tray.miniPlayer));
    expect(out).toContain(label(t.player.fullscreen));
    expect(out).not.toContain("@max-[460px]:hidden");
    expect(out).not.toContain("compact-grid");
  });
});
