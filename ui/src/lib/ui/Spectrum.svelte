<script lang="ts">
  // Background spectrum of the full-screen player: 16 soft columns of light rising from
  // the bottom edge plus a low glow that breathes with the bass. White at low opacity, so
  // it tints the artwork glow rather than drawing on top of it.
  //
  // Levels arrive at ~30 Hz (spectrum_subscribe, only while this is mounted) and are eased
  // toward in requestAnimationFrame. The canvas is drawn at an eighth of the CSS size and
  // upscaled, which blurs the columns for free. The loop stops once everything is dark.
  import { Channel, invoke } from "$lib/ipc";

  const BANDS = 16;
  /** Canvas pixels per CSS pixel: low on purpose, the upscale is the blur. */
  const SCALE = 0.125;
  /** Easing time constant toward the latest frame, ms. */
  const TAU = 70;

  let canvas: HTMLCanvasElement | undefined = $state();
  let reduced = $state(matchMedia("(prefers-reduced-motion: reduce)").matches);

  $effect(() => {
    const mq = matchMedia("(prefers-reduced-motion: reduce)");
    const on = () => (reduced = mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  });

  $effect(() => {
    const el = canvas;
    if (reduced || !el) return;
    const ctx = el.getContext("2d");
    if (!ctx) return;

    const target = new Float32Array(BANDS);
    const cur = new Float32Array(BANDS);
    let raf = 0;
    let last = 0;
    let w = 0;
    let h = 0;

    const resize = () => {
      w = Math.max(1, Math.round(el.clientWidth * SCALE));
      h = Math.max(1, Math.round(el.clientHeight * SCALE));
      el.width = w;
      el.height = h;
      draw();
    };
    const ro = new ResizeObserver(resize);
    ro.observe(el);

    function draw() {
      ctx!.clearRect(0, 0, w, h);
      // Bass glow: a wide ellipse just below the bottom edge.
      const bass = (cur[0] + cur[1] + cur[2] + cur[3]) / 4;
      if (bass > 0.01) {
        const g = ctx!.createRadialGradient(w / 2, h * 1.05, 0, w / 2, h * 1.05, h * 0.9);
        g.addColorStop(0, `rgb(255 255 255 / ${0.1 * bass * bass})`);
        g.addColorStop(1, "rgb(255 255 255 / 0)");
        ctx!.fillStyle = g;
        ctx!.fillRect(0, 0, w, h);
      }
      const col = w / BANDS;
      const bar = col * 0.56;
      for (let i = 0; i < BANDS; i++) {
        const v = cur[i];
        if (v < 0.005) continue;
        const bh = h * 0.42 * v;
        const x = i * col + (col - bar) / 2;
        const g = ctx!.createLinearGradient(0, h, 0, h - bh);
        g.addColorStop(0, `rgb(255 255 255 / ${0.06 + 0.12 * v})`);
        g.addColorStop(1, "rgb(255 255 255 / 0)");
        ctx!.fillStyle = g;
        ctx!.fillRect(x, h - bh, bar, bh);
      }
    }

    function frame(now: number) {
      const k = 1 - Math.exp(-Math.min(100, now - last) / TAU);
      last = now;
      let live = false;
      for (let i = 0; i < BANDS; i++) {
        cur[i] += (target[i] - cur[i]) * k;
        if (cur[i] < 0.001 && target[i] === 0) cur[i] = 0;
        if (cur[i] > 0 || target[i] > 0) live = true;
      }
      draw();
      raf = live ? requestAnimationFrame(frame) : 0;
    }

    const channel = new Channel<number[]>();
    channel.onmessage = (bands) => {
      for (let i = 0; i < BANDS; i++) target[i] = bands[i] ?? 0;
      if (!raf) {
        last = performance.now();
        raf = requestAnimationFrame(frame);
      }
    };
    invoke("spectrum_subscribe", { channel }).catch(() => {});

    return () => {
      channel.onmessage = () => {};
      invoke("spectrum_unsubscribe").catch(() => {});
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  });
</script>

{#if !reduced}
  <canvas bind:this={canvas} class="spectrum pointer-events-none absolute inset-0 size-full" aria-hidden="true"></canvas>
{/if}

<style>
  .spectrum {
    mix-blend-mode: screen;
  }
  @media (prefers-reduced-motion: reduce) {
    .spectrum {
      display: none;
    }
  }
</style>
