// Settings → «Last.fm and Discord» (lib/ui/IntegrationsSettings.svelte).
import type { integrations as en } from "../en/integrations";

export const integrations: typeof en = {
  integrations: {
    title: "Last.fm і Discord",
    lastfm: "Last.fm",
    unavailable: "Скроблінг недоступний у цій збірці Mixtape.",
    hint: "Скроблінг прослуханого: трек зараховується після половини тривалості або 4 хвилин.",
    connect: "Підключити",
    pendingHint: "Дозвольте доступ на сторінці Last.fm, що відкрилася в браузері, і натисніть кнопку.",
    openAgain: "Відкрити сторінку знову",
    allowed: "Я дозволив доступ",
    connectedAs: (name: string) => `Підключено як ${name}`,
    connectedHint: "Треки скроблюються після половини тривалості або 4 хвилин.",
    disconnect: "Відключити",
    connected: (name: string) => `Last.fm підключено: ${name}`,
    disconnected: "Last.fm відключено",
    failed: "Не вдалося підключити Last.fm",
    discord: "Показувати в Discord, що грає",
    discordHint: "Поки грає музика, у статусі Discord видно «Слухає» і трек. Застосунок Discord має бути запущений.",
  },
};
