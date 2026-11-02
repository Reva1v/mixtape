// «Распознать трек» (lib/ui/Recognize.svelte, specs/002).
import type { recognize as en } from "../en/recognize";

export const recognize: typeof en = {
  recognize: {
    open: "Розпізнати трек",
    title: "Розпізнати трек",
    source: "Що слухати",
    computer: "Звук комп'ютера",
    microphone: "Мікрофон",
    listening: "Слухаю…",
    computerHint: "Увімкніть пісню на цьому комп'ютері",
    microphoneHint: "Піднесіть мікрофон ближче до музики",
    computerQuiet: "На комп'ютері зараз нічого не грає",
    microphoneQuiet: "Майже нічого не чути: піднесіть мікрофон ближче",
    cancel: "Скасувати",
    play: "Увімкнути",
    album: "Альбом",
    artist: "Виконавець",
    again: "Ще раз",
    searchYouTube: "Шукати в YouTube Music",
    notOnYouTube: "У YouTube Music не знайшовся",
    nothing: "Не вдалося розпізнати",
    nothingComputer: "Спробуйте ще раз, поки пісня грає",
    nothingMicrophone: "Спробуйте ще раз ближче до музики або гучніше",
    denied: "Потрібен доступ до мікрофона",
    deniedHint:
      "Дозвольте застосункам використовувати мікрофон у налаштуваннях Windows. Він слухає лише поки триває розпізнавання, надсилається тільки короткий відбиток звуку",
    settings: "Налаштування",
    noDevice: "Немає звукового пристрою",
    noOutputHint: "У Windows немає пристрою виведення, який можна слухати",
    noMicHint: "Підключіть мікрофон або слухайте звук комп'ютера",
    offline: "Немає мережі",
    offlineHint: "Для розпізнавання потрібен інтернет",
    limited: "Забагато спроб",
    limitedHint: "Shazam просить зачекати. Спробуйте за хвилину",
    failed: "Розпізнавання не вдалося",
  },
};
