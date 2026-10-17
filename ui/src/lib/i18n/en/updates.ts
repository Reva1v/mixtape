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
};
