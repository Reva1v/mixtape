<script lang="ts" module>
  import { cubicBezier } from "motion";

  /** Layer pose: translate (% of the layer), rotation (deg), scale. */
  type Pose = [tx: number, ty: number, rot: number, scale: number];
  type Layer = { period: number; from: Pose; to: Pose; alpha: number };

  // The two layers of the old CSS backdrop: `drift-a` 40 s and `drift-b` 52 s, eased in
  // and out, back and forth; the second one screened over the first at half strength.
  const LAYERS: Layer[] = [
    { period: 40_000, from: [-50, -50, 0, 1.1], to: [-47, -53, 20, 1.25], alpha: 1 },
    { period: 52_000, from: [-53, -47, 170, 1.2], to: [-50, -50, 140, 1.05], alpha: 0.5 },
  ];
  const ease = cubicBezier(0.42, 0, 0.58, 1);
  /** Layer side: 130vmax, as before. */
  const LAYER_VMAX = 1.3;
  /** Blur and saturation the layers had, in CSS px of the window-sized layer. */
  const BLUR = 12;
  const SATURATE = 1.6;
  const BG = "#070709";
  /** Canvas pixels per CSS pixel: the picture is a 64 px thumbnail blown up ~40×, so
   * there is no detail to lose, and the browser smooths the last upscale. */
  const RES = 1 / 4;
  /** Redraws per second while drifting; one step moves the picture well under a pixel. */
  const FPS = 10;
  /** Side of the pre-filtered copy of the cover. */
  const BAKE = 256;

  function pose(l: Layer, t: number): Pose {
    // `alternate`: forwards on even cycles, backwards on odd ones.
    const cycle = t / l.period;
    const n = Math.floor(cycle);
    const p = ease(n % 2 === 0 ? cycle - n : 1 - (cycle - n));
    return l.from.map((a, i) => a + (l.to[i] - a) * p) as Pose;
  }
</script>

<script lang="ts">
  import { untrack } from "svelte";
  // Full-screen player backdrop: the cover's 64 px thumbnail stretched over the window in
  // two slowly drifting, blurred, saturated layers. It used to be two window-sized
  // <img> layers with a CSS blur and `mix-blend-mode: screen`, recomposited on every
  // display frame. Here the cover is blurred and saturated once into a small bitmap, and
  // both layers are drawn into a quarter-resolution canvas ten times a second, only
  // while `running`. The picture is the same; the GPU redraws it 10 times a second
  // instead of 60–144, without a blur or blend pass.
  let { src, running }: { src: string | undefined; running: boolean } = $props();

  let canvas = $state<HTMLCanvasElement>();
  let width = $state(0);
  let height = $state(0);
  let baked = $state.raw<HTMLCanvasElement | null>(null);

  /** Time along the drift, advanced only while running (the CSS version started at 0). */
  let elapsed = 0;

  // Blur + saturate the cover once per picture and window size. The blur radius is scaled
  // from the window-sized layer down to the bitmap.
  $effect(() => {
    const url = src;
    const side = LAYER_VMAX * Math.max(width, height);
    if (!url || !side) return;
    let stale = false;
    const img = new Image();
    img.decoding = "async";
    img.onload = () => {
      if (stale) return;
      const c = document.createElement("canvas");
      c.width = c.height = BAKE;
      const ctx = c.getContext("2d")!;
      // `object-fit: cover`: the centred square of the picture.
      const s = Math.min(img.naturalWidth, img.naturalHeight);
      ctx.filter = `blur(${((BLUR * BAKE) / side).toFixed(2)}px) saturate(${SATURATE})`;
      ctx.imageSmoothingQuality = "high";
      ctx.drawImage(img, (img.naturalWidth - s) / 2, (img.naturalHeight - s) / 2, s, s, 0, 0, BAKE, BAKE);
      baked = c;
    };
    img.src = url;
    return () => {
      stale = true;
      img.onload = null;
    };
  });

  function draw() {
    const ctx = canvas?.getContext("2d");
    if (!ctx || !width || !height) return;
    const W = canvas!.width;
    const H = canvas!.height;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = "source-over";
    ctx.fillStyle = BG;
    ctx.fillRect(0, 0, W, H);
    if (!baked) return;
    const k = W / width;
    const L = LAYER_VMAX * Math.max(width, height);
    ctx.imageSmoothingQuality = "high";
    for (const l of LAYERS) {
      const [tx, ty, rot, scale] = pose(l, elapsed);
      // CSS: box at (50%, 50%), L×L, transform-origin at its centre, then
      // translate(tx%, ty%) rotate() scale().
      ctx.setTransform(k, 0, 0, k, 0, 0);
      ctx.translate(width / 2 + L / 2 + (tx / 100) * L, height / 2 + L / 2 + (ty / 100) * L);
      ctx.rotate((rot * Math.PI) / 180);
      ctx.scale(scale, scale);
      ctx.globalAlpha = l.alpha;
      ctx.globalCompositeOperation = l.alpha < 1 ? "screen" : "source-over";
      ctx.drawImage(baked, -L / 2, -L / 2, L, L);
    }
  }

  // Canvas size follows the window.
  $effect(() => {
    if (!canvas || !width || !height) return;
    canvas.width = Math.max(1, Math.round(width * RES));
    canvas.height = Math.max(1, Math.round(height * RES));
    untrack(draw);
  });

  // Drift while running; otherwise hold still on the last pose.
  $effect(() => {
    void baked;
    untrack(draw);
    if (!running) return;
    let last = performance.now();
    const id = window.setInterval(() => {
      const now = performance.now();
      // A long gap (timers throttled) does not jump the picture ahead.
      elapsed += Math.min(now - last, 250);
      last = now;
      draw();
    }, 1000 / FPS);
    return () => clearInterval(id);
  });
</script>

<div class="absolute inset-0" bind:clientWidth={width} bind:clientHeight={height}>
  <canvas bind:this={canvas} class="block size-full"></canvas>
</div>
