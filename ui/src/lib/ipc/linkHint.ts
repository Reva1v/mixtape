// A cheap pre-check before asking Rust whether the search text is a YouTube Music link
// (`link_parse`): ordinary typing shouldn't cost an IPC round trip per keystroke. It may
// say yes to something Rust then rejects (an 11-letter word), never no to a real link.

/** The text mentions a YouTube host (`youtube.com`, `music.youtube.com`, `youtu.be`). */
export const hasYoutubeHost = (text: string) => /youtu/i.test(text);

/** Any YouTube host (`youtube.com`, `music.youtube.com`, `youtu.be`, …) or a bare id
 * (video id, `PL…`, `MPREb_…`, `UC…`) that is the whole text. */
export function mayBeLink(text: string): boolean {
  const s = text.trim();
  if (!s) return false;
  if (hasYoutubeHost(s)) return true;
  // 11 is a video id; playlist, album and channel ids are longer. No spaces: a sentence
  // with a long word in it isn't an id.
  return /^[A-Za-z0-9_-]{11,64}$/.test(s);
}
