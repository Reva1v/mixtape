// Whether anything is worth drawing: the page is on screen (not minimized or hidden; the
// core also marks the webview invisible when the window is minimized) and no modal
// covers it. Progress bars, equalizers and the full-screen backdrop stop while it is
// false and catch up when it turns true again.

class Visibility {
  /** The page is shown (`document.visibilityState`). */
  page = $state(typeof document === "undefined" || document.visibilityState === "visible");
  /** Modals open right now; their blurred backdrop is redrawn whenever anything under it moves. */
  modals = $state(0);

  /** Animate things on the main view. */
  get live() {
    return this.page && this.modals === 0;
  }
}

export const visible = new Visibility();

if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => (visible.page = document.visibilityState === "visible"));
}
