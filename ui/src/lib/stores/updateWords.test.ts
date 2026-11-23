import { describe, expect, it } from "vitest";
import { en } from "$lib/i18n/en";
import { ru } from "$lib/i18n/ru";
import { uk } from "$lib/i18n/uk";
import { updateWords } from "./updateWords";

describe("updateWords", () => {
  it("speaks of Mixtape standalone", () => {
    const w = updateWords(false, ru.updates);
    expect(w.available("0.4.6")).toBe("Доступна версия Mixtape 0.4.6");
    expect(w.install).toBe("Обновить");
  });

  it("speaks of the island inside Droplet Island", () => {
    const w = updateWords(true, ru.updates);
    expect(w.available("1.22.4")).toBe(
      "Droplet Island 1.22.4 доступен — с ним придёт новый Mixtape",
    );
    expect(w.install).toBe("Обновить остров");
  });

  it("has the island wording in every language", () => {
    for (const lang of [en, uk, ru]) {
      const own = updateWords(false, lang.updates);
      const island = updateWords(true, lang.updates);
      expect(island.install).not.toBe(own.install);
      expect(island.available("1.2.3")).toContain("Droplet Island 1.2.3");
      expect(island.restartHint).not.toBe(own.restartHint);
    }
  });
});
