// Shared layout state: the main scroll container (virtual lists measure against it),
// side panels, and requests to focus the search field.

class Layout {
  scrollEl = $state<HTMLElement | undefined>();
  drawer = $state<"queue" | null>(null);
  /** Lyrics beside the full-screen player (US5); hidden whenever the player opens. */
  lyrics = $state(false);
  #fullscreen = $state(false);
  /** Bumped to ask the search screen to focus its input. */
  searchFocus = $state(0);
  shortcutsOpen = $state(false);
  /** Header tint of the current page (an artist's picture); `null` follows the track. */
  pageGlow = $state<string | null>(null);

  get fullscreen() {
    return this.#fullscreen;
  }
  set fullscreen(v: boolean) {
    if (v && !this.#fullscreen) this.lyrics = false;
    this.#fullscreen = v;
  }

  toggleDrawer(panel: "queue") {
    this.drawer = this.drawer === panel ? null : panel;
  }

  /** Opens the full-screen player with lyrics, or toggles them when it is open. */
  toggleLyrics() {
    if (!this.fullscreen) {
      this.fullscreen = true;
      this.lyrics = true;
    } else this.lyrics = !this.lyrics;
  }
}

export const layout = new Layout();
