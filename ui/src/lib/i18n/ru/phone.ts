// Settings → «Phone»: stats sync with Mixtape on the phone (lib/ui/PhoneSettings.svelte).
import type { phone as en } from "../en/phone";

export const phone: typeof en = {
  phone: {
    title: "Телефон",
    notPaired: "Телефон не связан",
    notPairedHint: "Свяжите Mixtape на телефоне, чтобы объединить статистику прослушиваний. Обмен идёт напрямую через домашнюю сеть или Tailscale, без облака.",
    pair: "Связать телефон",
    scanTitle: "Отсканируйте код телефоном",
    scanHint: "На телефоне: Профиль → «Синхронизация с ПК» → «Сканировать QR-код». Телефон должен быть в одной сети с этим ПК или в Tailscale.",
    expiresIn: (time: string) => `Код действует ещё ${time}`,
    expired: "Код устарел",
    newCode: "Показать новый код",
    cancel: "Отмена",
    firewall: "Брандмауэр Windows может не пустить телефон",
    firewallHint: "Разрешите входящие соединения к Mixtape в частных сетях. Windows попросит подтверждение.",
    firewallAllow: "Разрешить в брандмауэре",
    firewallAdded: "Правило брандмауэра добавлено",
    pairedWith: (name: string) => `Связано: ${name}`,
    lastExchange: (when: string) => `Последний обмен: ${when}`,
    neverExchanged: "Обмена ещё не было: откройте Mixtape на телефоне",
    notListening: "Синхронизация недоступна: порт 47821 занят",
    unpair: "Отвязать",
    unpairConfirm: "Отвязать телефон? Уже полученные прослушивания останутся.",
    unpaired: "Телефон отвязан",
    paired: (name: string) => `Телефон связан: ${name}`,
    failed: "Не удалось связать телефон",
  },
};
