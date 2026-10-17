// Tray icon, start with Windows and the mini player (Settings → Window, the mini strip).
import type { tray as en } from "../en/tray";

export const tray: typeof en = {
  tray: {
    section: "Вікно",
    closeToTray: "Згортати в трей під час закриття",
    closeToTrayHint:
      "Кнопка закриття ховає Mixtape біля годинника, а музика грає далі. Вийти можна з меню значка в треї",
    autostart: "Запускати з Windows",
    autostartHint: "Відкривати Mixtape після входу в Windows",
    autostartOff: "Вимкнено",
    autostartOn: "Відкривати вікно",
    autostartHidden: "Приховано в треї",
    autostartFailed: "Не вдалося змінити автозапуск",
    miniPlayer: "Мініплеєр",
    fullWindow: "Повне вікно",
    menu: "Меню Mixtape",
    show: "Показати Mixtape",
    hide: "Сховати Mixtape",
    quit: "Вийти",
  },
};
