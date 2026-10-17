// Tray icon, start with Windows and the mini player (Settings → Window, the mini strip).
import type { tray as en } from "../en/tray";

export const tray: typeof en = {
  tray: {
    section: "Окно",
    closeToTray: "Сворачивать в трей при закрытии",
    closeToTrayHint:
      "Кнопка закрытия прячет Mixtape у часов, а музыка играет дальше. Выйти можно из меню значка в трее",
    autostart: "Запускать вместе с Windows",
    autostartHint: "Открывать Mixtape после входа в Windows",
    autostartOff: "Выключено",
    autostartOn: "Открывать окно",
    autostartHidden: "Скрыто в трее",
    autostartFailed: "Не удалось изменить автозапуск",
    miniPlayer: "Мини-плеер",
    fullWindow: "Полное окно",
    menu: "Меню Mixtape",
    show: "Показать Mixtape",
    hide: "Скрыть Mixtape",
    quit: "Выйти",
  },
};
