// Transition between tracks (Settings → Playback): one choice of gapless, crossfade or
// smart transitions.

export const smart = {
  transition: {
    title: "Transition between tracks",
    subtitle: "How one track hands over to the next",
    gapless: "Gapless",
    smooth: "Crossfade",
    smart: "Smart",
    beta: "beta",
    gaplessDesc: "The next track starts right where the last one ends",
    smoothDesc: "The end of a track blends into the start of the next",
    smartDesc: "Blends like a DJ: on the beat, with matched tempo and a bass swap",
    albumNote: "Tracks of one album always play gapless.",
    smartNote: "Each track is analysed once in the background; until then the crossfade plays.",
  },
};
