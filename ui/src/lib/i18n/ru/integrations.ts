// Settings → «Last.fm and Discord» (lib/ui/IntegrationsSettings.svelte).
import type { integrations as en } from "../en/integrations";

export const integrations: typeof en = {
  integrations: {
    title: "Last.fm и Discord",
    lastfm: "Last.fm",
    unavailable: "Скробблинг недоступен в этой сборке Mixtape.",
    hint: "Скробблинг прослушанного: трек засчитывается после половины длительности или 4 минут.",
    connect: "Подключить",
    pendingHint: "Разрешите доступ на странице Last.fm, которая открылась в браузере, и нажмите кнопку.",
    openAgain: "Открыть страницу снова",
    allowed: "Я разрешил доступ",
    connectedAs: (name: string) => `Подключено как ${name}`,
    connectedHint: "Треки скробблятся после половины длительности или 4 минут.",
    disconnect: "Отключить",
    connected: (name: string) => `Last.fm подключён: ${name}`,
    disconnected: "Last.fm отключён",
    failed: "Не удалось подключить Last.fm",
    discord: "Показывать в Discord, что играет",
    discordHint: "Пока играет музыка, в статусе Discord видно «Слушает» и трек. Приложение Discord должно быть запущено.",
  },
};
