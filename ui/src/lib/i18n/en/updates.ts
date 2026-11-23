// Update check and install (Settings → Updates, the "new version" toast).
export const updates = {
  title: "Updates",
  version: (v: string) => `Mixtape ${v}`,
  auto: "Check for updates automatically",
  autoHint: "On start and once a day. You decide when to install.",
  check: "Check now",
  checking: "Checking…",
  latest: "You have the latest version",
  available: (v: string) => `Mixtape ${v} is available`,
  install: "Update",
  installing: (pct: number | null) => (pct === null ? "Downloading the update…" : `Downloading the update… ${pct}%`),
  restartHint: "The app restarts after the update and continues where you left off.",
  ytdlp: {
    title: "yt-dlp",
    /** The version in use and where it came from. */
    inUse: (version: string | null, updatedOn: string | null) =>
      `${version ?? "version unknown"} · ${updatedOn ? `updated ${updatedOn}` : "bundled"}`,
    upToDate: "Up to date",
    pending: (latest: string, date: string) => `New version ${latest} will install itself after ${date}`,
    pendingManual: (latest: string) => `New version ${latest} is out; automatic update is off`,
    installed: (version: string) => `Installed ${version}`,
    revert: "Use the bundled one",
    reverting: "Switching…",
    auto: "Update yt-dlp automatically",
    autoHint: "5 days after release, after the signature is checked. You can go back to the bundled one.",
    /** Toasts. */
    toastPending: (latest: string, installed: string, date: string) =>
      `A new yt-dlp ${latest} is out (you have ${installed}). It will install itself after ${date}`,
    toastManual: (latest: string, installed: string) =>
      `A new yt-dlp ${latest} is out (you have ${installed}). It doesn't install itself: ask Claude to check and update it`,
    toastInstalled: (version: string) => `yt-dlp ${version} installed`,
    toastRolledBack: (bad: string, restored: string) => `The new yt-dlp ${bad} didn't work, went back to ${restored}`,
  },
};
