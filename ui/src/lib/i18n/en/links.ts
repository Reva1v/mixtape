// Shared YouTube Music links: the search card (lib/ui/LinkCard.svelte), «Share» in the menus
// (lib/ui/share.ts) and the browser extension block in Settings (IntegrationsSettings.svelte).
export const links = {
  link: {
    track: "Track from link",
    album: "Album from link",
    playlist: "Playlist from link",
    artist: "Artist from link",
    loadFailed: "Couldn't load the track's details",
    openFailed: "Couldn't open the link",
  },
  share: {
    track: "Share track",
    album: "Share album",
    playlist: "Share playlist",
    artist: "Share artist",
    copied: "Link copied",
    copyFailed: "Couldn't copy the link",
  },
  extension: {
    title: "Browser extension",
    name: "Open YouTube Music links in Mixtape",
    hint: "Links to YouTube Music from chats and other sites open in Mixtape instead of the browser.",
    openFolder: "Open folder",
    openFailed: "Couldn't open the folder",
    steps: [
      "Open chrome://extensions (Chrome) or edge://extensions (Edge).",
      "Turn on developer mode.",
      "Press «Load unpacked» and choose the folder with the extension.",
    ],
  },
};
