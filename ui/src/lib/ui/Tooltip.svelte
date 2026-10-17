<script lang="ts">
  // App-wide tooltips. Any element with `data-tip="…"` gets one; `data-tip-pos`
  // picks the side (top by default, right, bottom). Shown after a short hover delay;
  // moving between tipped elements while one is open swaps instantly.
  import { pop } from "$lib/motion";

  type Pos = "top" | "right" | "bottom";

  const DELAY = 450;
  const WARM = 300;

  let text = $state("");
  let pos = $state<Pos>("top");
  let anchor = $state<DOMRect | null>(null);
  let el = $state<HTMLDivElement>();
  let at = $state({ x: 0, y: 0 });

  let current: HTMLElement | null = null;
  let timer = 0;
  let lastHidden = 0;

  function tipOf(target: EventTarget | null): HTMLElement | null {
    return target instanceof Element ? (target.closest("[data-tip]") as HTMLElement | null) : null;
  }

  function show(node: HTMLElement) {
    const tip = node.dataset.tip;
    if (!tip) return;
    text = tip;
    pos = (node.dataset.tipPos as Pos) ?? "top";
    anchor = node.getBoundingClientRect();
  }

  function hide() {
    clearTimeout(timer);
    if (anchor) lastHidden = performance.now();
    anchor = null;
    current = null;
  }

  function over(e: PointerEvent) {
    const node = tipOf(e.target);
    if (node === current) return;
    clearTimeout(timer);
    current = node;
    if (!node) {
      hide();
      return;
    }
    const warm = anchor !== null || performance.now() - lastHidden < WARM;
    if (warm) show(node);
    else timer = window.setTimeout(() => current === node && show(node), DELAY);
  }

  // Place next to the anchor and keep inside the window.
  $effect(() => {
    if (!anchor || !el) return;
    const r = el.getBoundingClientRect();
    const gap = 8;
    let x: number;
    let y: number;
    if (pos === "right") {
      x = anchor.right + gap;
      y = anchor.top + anchor.height / 2 - r.height / 2;
    } else {
      x = anchor.left + anchor.width / 2 - r.width / 2;
      y = pos === "bottom" ? anchor.bottom + gap : anchor.top - r.height - gap;
      if (pos === "top" && y < 4) y = anchor.bottom + gap;
    }
    at = {
      x: Math.round(Math.min(Math.max(4, x), window.innerWidth - r.width - 4)),
      y: Math.round(Math.min(Math.max(4, y), window.innerHeight - r.height - 4)),
    };
  });
</script>

<svelte:document onpointerover={over} onpointerdown={hide} onwheel={hide} onkeydown={hide} />
<svelte:window onblur={hide} />

{#if anchor}
  <div
    bind:this={el}
    role="tooltip"
    class="pointer-events-none fixed z-[70] rounded-md bg-[#1c1c1e] px-2.5 py-1 text-[12px] font-medium whitespace-nowrap text-ink
      shadow-[0_8px_24px_-6px_rgb(0_0_0/0.7)] ring-1 ring-white/[0.08]"
    style:left="{at.x}px"
    style:top="{at.y}px"
    transition:pop={{
      scale: 0.92,
      x: pos === "right" ? -4 : 0,
      y: pos === "right" ? 0 : pos === "bottom" ? -4 : 4,
      origin: pos === "right" ? "left center" : pos === "bottom" ? "top center" : "bottom center",
    }}
  >
    {text}
  </div>
{/if}
