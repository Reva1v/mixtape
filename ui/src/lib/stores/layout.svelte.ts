// Shared layout state: the main scroll container (virtual lists measure against it),
// side panels, and requests to focus the search field.

class Layout {
  scrollEl = $state<HTMLElement | undefined>();
  drawer = $state<"queue" | "lyrics" | null>(null);
  fullscreen = $state(false);
  /** Bumped to ask the search screen to focus its input. */
  searchFocus = $state(0);

  toggleDrawer(panel: "queue" | "lyrics") {
    this.drawer = this.drawer === panel ? null : panel;
  }
}

export const layout = new Layout();
