// Transition between tracks (Settings → Playback): one choice of gapless, crossfade or
// smart transitions.

export const smart = {
  transition: {
    title: "Transition between tracks",
    gapless: "Gapless",
    smooth: "Crossfade",
    smart: "Smart (beta)",
    gaplessHint: "The next track starts right where the last one ends.",
    smoothHint: "The end of a track blends into the start of the next one. Tracks of one album always play gapless.",
    smartHint:
      "Tracks blend like a DJ mix: on the beat, with matched tempo and a bass swap; shorter when the keys clash. Each track is analysed once in the background; until then the usual crossfade plays. Tracks of one album always play gapless.",
  },
};
