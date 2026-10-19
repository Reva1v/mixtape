// Transition between tracks (Settings → Playback).
import type { smart as en } from "../en/smart";

export const smart: typeof en = {
  transition: {
    title: "Переход между треками",
    subtitle: "Как один трек сменяет другой",
    gapless: "Без паузы",
    smooth: "Плавный",
    smart: "Умный",
    beta: "бета",
    gaplessDesc: "Следующий трек начинается сразу после конца предыдущего",
    smoothDesc: "Конец трека плавно смешивается с началом следующего",
    smartDesc: "Сведение как у диджея: в долю, с подогнанным темпом и сменой басов",
    albumNote: "Треки одного альбома всегда идут без паузы.",
    smartNote: "Каждый трек один раз анализируется в фоне, до этого — обычный плавный переход.",
  },
};
