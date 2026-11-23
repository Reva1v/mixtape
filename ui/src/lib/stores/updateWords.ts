// Which words the app-update row and toast use: inside Droplet Island an update is the island's
// (the host's `update_check` and `app_version` speak of the island), and it brings the new
// Mixtape; standalone nothing changes. Apart from the store so the choice can be tested.

export type UpdateWords = {
  available: (v: string) => string;
  install: string;
  restartHint: string;
};

type Dictionary = UpdateWords & { island: UpdateWords };

/** The island's wording when `embedded`, else Mixtape's own. */
export function updateWords(embedded: boolean, u: Dictionary): UpdateWords {
  const w = embedded ? u.island : u;
  return { available: w.available, install: w.install, restartHint: w.restartHint };
}
