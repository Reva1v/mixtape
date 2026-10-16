// Shared layout state: the main scroll container (virtual lists measure against it)
// and which side panels are open.

class Layout {
  scrollEl = $state<HTMLElement | undefined>();
  drawer = $state<"queue" | "lyrics" | null>(null);
  fullscreen = $state(false);

  toggleDrawer(panel: "queue" | "lyrics") {
    this.drawer = this.drawer === panel ? null : panel;
  }
}

export const layout = new Layout();
