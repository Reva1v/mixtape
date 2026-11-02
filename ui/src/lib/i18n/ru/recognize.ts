// «Распознать трек» (lib/ui/Recognize.svelte, specs/002).
import type { recognize as en } from "../en/recognize";

export const recognize: typeof en = {
  recognize: {
    open: "Распознать трек",
    title: "Распознать трек",
    source: "Что слушать",
    computer: "Звук компьютера",
    microphone: "Микрофон",
    listening: "Слушаю…",
    computerHint: "Включите песню на этом компьютере",
    microphoneHint: "Поднесите микрофон ближе к музыке",
    computerQuiet: "На компьютере сейчас ничего не играет",
    microphoneQuiet: "Почти ничего не слышно: поднесите микрофон ближе",
    cancel: "Отмена",
    play: "Включить",
    album: "Альбом",
    artist: "Исполнитель",
    again: "Ещё раз",
    searchYouTube: "Искать в YouTube Music",
    notOnYouTube: "В YouTube Music не нашёлся",
    nothing: "Не удалось распознать",
    nothingComputer: "Попробуйте ещё раз, пока песня играет",
    nothingMicrophone: "Попробуйте ещё раз ближе к музыке или погромче",
    denied: "Нужен доступ к микрофону",
    deniedHint:
      "Разрешите приложениям использовать микрофон в настройках Windows. Он слушает только пока идёт распознавание, отправляется лишь короткий отпечаток звука",
    settings: "Настройки",
    noDevice: "Нет звукового устройства",
    noOutputHint: "В Windows нет устройства вывода, которое можно слушать",
    noMicHint: "Подключите микрофон или слушайте звук компьютера",
    offline: "Нет сети",
    offlineHint: "Для распознавания нужен интернет",
    limited: "Слишком много попыток",
    limitedHint: "Shazam просит подождать. Попробуйте через минуту",
    failed: "Распознавание не удалось",
  },
};
