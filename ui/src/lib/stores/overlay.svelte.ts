// Global popups: one context menu at a time, and a toast stack.

import type { Component } from "svelte";

export type MenuItem =
  | {
      label: string;
      icon?: Component;
      action: () => void;
      danger?: boolean;
      disabled?: boolean;
      /** The chosen option of a dropdown (`Select`): drawn with a check mark. */
      checked?: boolean;
    }
  | "separator";

class MenuState {
  items = $state.raw<MenuItem[]>([]);
  x = $state(0);
  y = $state(0);
  open = $state(false);

  show(items: MenuItem[], x: number, y: number) {
    this.items = items;
    this.x = x;
    this.y = y;
    this.open = true;
  }

  /** Opens next to an element, e.g. a "⋯" button. */
  showAt(items: MenuItem[], el: Element) {
    const r = el.getBoundingClientRect();
    this.show(items, r.right, r.bottom + 4);
  }

  /** Opens under an element, left edges aligned (dropdowns). */
  showUnder(items: MenuItem[], el: Element) {
    const r = el.getBoundingClientRect();
    this.show(items, r.left, r.bottom + 4);
  }

  close() {
    this.open = false;
  }
}

export const menu = new MenuState();

export type ToastAction = { label: string; run: () => void };
export type Toast = { id: number; text: string; level: "info" | "error"; action?: ToastAction };

/** More than this on screen at once and the oldest goes. */
const MAX_TOASTS = 3;

class Toasts {
  list = $state<Toast[]>([]);
  #next = 1;
  #timers = new Map<number, ReturnType<typeof setTimeout>>();

  /**
   * `action` adds a button to the toast (e.g. «Подключить»); clicking it dismisses the toast.
   * The same message again only keeps the one on screen up longer.
   */
  push(text: string, level: Toast["level"] = "info", ms = 3500, action?: ToastAction) {
    const same = this.list.find((t) => t.text === text && t.level === level);
    if (same) {
      this.#arm(same.id, ms);
      return;
    }
    const id = this.#next++;
    this.list.push({ id, text, level, action });
    this.#arm(id, ms);
    if (this.list.length > MAX_TOASTS) this.dismiss(this.list[0].id);
  }

  dismiss(id: number) {
    clearTimeout(this.#timers.get(id));
    this.#timers.delete(id);
    this.list = this.list.filter((t) => t.id !== id);
  }

  #arm(id: number, ms: number) {
    clearTimeout(this.#timers.get(id));
    this.#timers.set(
      id,
      setTimeout(() => this.dismiss(id), ms),
    );
  }
}

export const toasts = new Toasts();

/** A text input dialog (window.prompt does not exist in our webview). */
class Prompt {
  open = $state(false);
  title = $state("");
  value = $state("");
  #resolve: ((v: string | null) => void) | null = null;

  ask(title: string, initial = ""): Promise<string | null> {
    this.#resolve?.(null);
    this.title = title;
    this.value = initial;
    this.open = true;
    return new Promise((r) => (this.#resolve = r));
  }

  finish(ok: boolean) {
    const v = ok ? this.value.trim() || null : null;
    this.open = false;
    this.#resolve?.(v);
    this.#resolve = null;
  }
}

export const prompt = new Prompt();
