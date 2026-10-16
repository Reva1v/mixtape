// "Add to playlist" picker: which tracks are waiting for a playlist choice.

class PlaylistPicker {
  trackIds = $state.raw<string[] | null>(null);

  open(trackIds: string[]) {
    this.trackIds = trackIds;
  }
  close() {
    this.trackIds = null;
  }
}

export const playlistPicker = new PlaylistPicker();
