// Transition between tracks (Settings → Playback).
import type { smart as en } from "../en/smart";

export const smart: typeof en = {
  transition: {
    title: "Перехід між треками",
    subtitle: "Як один трек змінює інший",
    gapless: "Без паузи",
    smooth: "Плавний",
    smart: "Розумний",
    beta: "бета",
    gaplessDesc: "Наступний трек починається одразу після кінця попереднього",
    smoothDesc: "Кінець треку плавно змішується з початком наступного",
    smartDesc: "Зведення як у діджея: у долю, з підігнаним темпом і заміною басів",
    albumNote: "Треки одного альбому завжди йдуть без паузи.",
    smartNote: "Кожен трек один раз аналізується у фоні, доти — звичайний плавний перехід.",
  },
};
