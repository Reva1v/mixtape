// Motion design: every movement is a physical spring (motion.dev), never a linear or
// stock ease. Springs drive Svelte transitions through a sampled easing curve, and
// gestures (press, hover) through Motion's WAAPI animations, so the work stays on the
// compositor.

import { animate, hover, press, spring } from "motion";
import type { Attachment } from "svelte/attachments";
import type { TransitionConfig } from "svelte/transition";

export type Spring = { stiffness: number; damping: number; mass?: number };

export const springs = {
  /** Buttons and small UI: quick, a hint of bounce. */
  snappy: { stiffness: 520, damping: 32 },
  /** Panels, menus, screens: settles without visible overshoot. */
  smooth: { stiffness: 340, damping: 36 },
  /** Large surfaces like the full-screen player. */
  gentle: { stiffness: 220, damping: 30 },
  /** Release after a press: springs back with a small overshoot. */
  bounce: { stiffness: 620, damping: 18 },
  /** Soft release for navigation icons: no overshoot, eases back. */
  soft: { stiffness: 380, damping: 30 },
} satisfies Record<string, Spring>;

const reduced = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Turns a spring into a Svelte transition duration + easing curve (may overshoot 1). */
export function springCurve(s: Spring): { duration: number; easing: (t: number) => number } {
  const gen = spring({ keyframes: [0, 1], ...s });
  const duration = parseFloat(String(gen)) || 400;
  return { duration, easing: (t) => (t >= 1 ? 1 : gen.next(t * duration).value) };
}

const curves = new Map<Spring, ReturnType<typeof springCurve>>();
function curve(s: Spring) {
  let c = curves.get(s);
  if (!c) curves.set(s, (c = springCurve(s)));
  return c;
}

/** Exits are short and never bounce: things leave faster than they arrive. */
const exitEase = (t: number) => 1 - (1 - t) ** 3;

type PopOpts = { scale?: number; y?: number; x?: number; spring?: Spring; origin?: string };

/** Fade + slight scale/offset in, quick fade out. For menus, dialogs, toasts. */
export function pop(node: Element, opts: PopOpts = {}, ctx?: { direction: "in" | "out" | "both" }): TransitionConfig {
  const { scale = 0.96, y = 0, x = 0, origin } = opts;
  if (origin) (node as HTMLElement).style.transformOrigin = origin;
  if (reduced()) return { duration: 120, css: (t) => `opacity:${t}` };
  const out = ctx?.direction === "out";
  const { duration, easing } = out ? { duration: 140, easing: exitEase } : curve(opts.spring ?? springs.snappy);
  return {
    duration,
    easing,
    css: (t, u) =>
      `opacity:${Math.min(1, t * 1.6)};transform:translate(${u * x}px,${u * y}px) scale(${scale + (1 - scale) * t})`,
  };
}

/**
 * Dialog entrance: fades in and rises by whole pixels. No scaling — a scaled layer is
 * drawn as a bitmap, so its text looks soft until the spring settles.
 */
export function rise(_node: Element, opts: { y?: number } = {}, ctx?: { direction: "in" | "out" | "both" }): TransitionConfig {
  if (reduced()) return { duration: 120, css: (t) => `opacity:${t}` };
  const y = opts.y ?? 8;
  const out = ctx?.direction === "out";
  const { duration, easing } = out ? { duration: 140, easing: exitEase } : curve(springs.snappy);
  return { duration, easing, css: (t, u) => `opacity:${Math.min(1, t * 1.6)};transform:translateY(${Math.round(u * y)}px)` };
}

/** Entrance for whole screens: content rises a few pixels and fades in. */
export function enter(_node: Element, opts: { y?: number } = {}): TransitionConfig {
  if (reduced()) return { duration: 0 };
  const { duration, easing } = curve(springs.smooth);
  const y = opts.y ?? 8;
  return { duration, easing, css: (t, u) => `opacity:${Math.min(1, t * 1.4)};transform:translateY(${u * y}px)` };
}

/** Plain opacity on a spring, for backdrops. */
export function fadeSpring(_node: Element, _opts?: unknown, ctx?: { direction: "in" | "out" | "both" }): TransitionConfig {
  const { duration, easing } = ctx?.direction === "out" ? { duration: 160, easing: exitEase } : curve(springs.smooth);
  return { duration, easing: (t) => Math.min(1, easing(t)), css: (t) => `opacity:${t}` };
}

/**
 * Press feedback: shrinks while held and springs back on release. Replaces CSS
 * `active:scale-*`, which snaps linearly.
 */
export function pressable(amount = 0.92, release: Spring = springs.bounce): Attachment<HTMLElement> {
  return (el) => {
    if (reduced()) return;
    return press(el, (target) => {
      if ((target as HTMLButtonElement).disabled || target.getAttribute("aria-current") === "page") return;
      animate(target, { scale: amount }, { type: "spring", ...springs.snappy });
      // Drop the leftover transform once settled, so the element leaves its GPU layer
      // and is drawn with normal anti-aliasing again.
      return () =>
        void animate(target, { scale: 1 }, { type: "spring", ...release }).finished.then(() => {
          (target as HTMLElement).style.removeProperty("scale");
          (target as HTMLElement).style.removeProperty("transform");
        });
    });
  };
}

/** Hover lift for cards: rises a little and settles back, on a spring. */
export function liftable(px = 3): Attachment<HTMLElement> {
  return (el) => {
    if (reduced()) return;
    return hover(el, (target) => {
      animate(target, { y: -px }, { type: "spring", ...springs.smooth });
      return () =>
        void animate(target, { y: 0 }, { type: "spring", ...springs.smooth }).finished.then(() =>
          (target as HTMLElement).style.removeProperty("transform"),
        );
    });
  };
}

/** Animates an element's width with a spring; resolves when it settles. */
export function springWidth(el: HTMLElement, to: number | string, s: Spring = springs.smooth) {
  if (reduced()) {
    el.style.width = typeof to === "number" ? `${to}px` : to;
    return Promise.resolve();
  }
  return animate(el, { width: to }, { type: "spring", ...s }).finished;
}

export { animate };

/**
 * Makes plain CSS transitions spring-shaped: `--ease-spring` / `--ease-out-soft` and
 * Tailwind's default timing function become `linear()` curves sampled from springs.
 */
export function installSpringCss() {
  const root = document.documentElement.style;
  const css = (s: Spring) => {
    const [ms, ...rest] = String(spring({ keyframes: [0, 1], ...s })).split(" ");
    return { ms, ease: rest.join(" ") };
  };
  const smooth = css(springs.smooth);
  const snappy = css(springs.snappy);
  root.setProperty("--ease-out-soft", smooth.ease);
  root.setProperty("--spring-smooth-ms", smooth.ms);
  root.setProperty("--ease-snappy", snappy.ease);
  root.setProperty("--default-transition-timing-function", smooth.ease);
  root.setProperty("--default-transition-duration", "220ms");
}

/** Bottom sheet: slides up from below on a gentle spring, drops away quickly. */
export function sheet(_node: Element, _opts?: unknown, ctx?: { direction: "in" | "out" | "both" }): TransitionConfig {
  if (reduced()) return { duration: 120, css: (t) => `opacity:${t}` };
  const out = ctx?.direction === "out";
  const { duration, easing } = out ? { duration: 220, easing: (t: number) => t * t } : curve(springs.smooth);
  return { duration, easing, css: (t, u) => `transform:translateY(${u * 100}%)` };
}

/** Non-passive wheel listener, so the handler can stop the page from scrolling. */
export function onWheel(handler: (e: WheelEvent) => void): Attachment<HTMLElement> {
  return (el) => {
    el.addEventListener("wheel", handler, { passive: false });
    return () => el.removeEventListener("wheel", handler);
  };
}
