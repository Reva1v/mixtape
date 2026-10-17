// Settings → «Last.fm and Discord» (lib/ui/IntegrationsSettings.svelte).
export const integrations = {
  integrations: {
    title: "Last.fm and Discord",
    lastfm: "Last.fm",
    unavailable: "Scrobbling isn't available in this build of Mixtape.",
    hint: "Scrobble what you listen to: a track counts after half of it or 4 minutes have played.",
    connect: "Connect",
    pendingHint: "Allow access on the Last.fm page that opened in your browser, then press the button.",
    openAgain: "Open the page again",
    allowed: "I've allowed access",
    connectedAs: (name: string) => `Connected as ${name}`,
    connectedHint: "Tracks are scrobbled after half of their length or 4 minutes.",
    disconnect: "Disconnect",
    connected: (name: string) => `Last.fm connected: ${name}`,
    disconnected: "Last.fm disconnected",
    failed: "Couldn't connect Last.fm",
    discord: "Show what's playing in Discord",
    discordHint: "While music plays, your Discord status shows «Listening to» with the track. The Discord app must be running.",
  },
};
