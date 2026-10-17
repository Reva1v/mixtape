/** Ukrainian and Russian plural form: slavic(1, "трек", "трека", "треков") → "трек". */
export function slavic(n: number, one: string, few: string, many: string): string {
  const m10 = Math.abs(n) % 10;
  const m100 = Math.abs(n) % 100;
  if (m10 === 1 && m100 !== 11) return one;
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return few;
  return many;
}

/** English plural form: english(2, "track") → "tracks". */
export const english = (n: number, one: string, other = `${one}s`) => (n === 1 ? one : other);
