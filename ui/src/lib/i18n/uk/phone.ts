// Settings → «Phone»: stats sync with Mixtape on the phone (lib/ui/PhoneSettings.svelte).
import type { phone as en } from "../en/phone";

export const phone: typeof en = {
  phone: {
    title: "Телефон",
    notPaired: "Телефон не зв'язано",
    notPairedHint: "Зв'яжіть Mixtape на телефоні, щоб об'єднати статистику прослуховувань. Обмін іде напряму через домашню мережу або Tailscale, без хмари.",
    pair: "Зв'язати телефон",
    scanTitle: "Відскануйте код телефоном",
    scanHint: "На телефоні: Профіль → «Синхронізація з ПК» → «Сканувати QR-код». Телефон має бути в одній мережі з цим ПК або в Tailscale.",
    expiresIn: (time: string) => `Код дійсний ще ${time}`,
    expired: "Код застарів",
    newCode: "Показати новий код",
    cancel: "Скасувати",
    firewall: "Брандмауер Windows може не пустити телефон",
    firewallHint: "Дозвольте вхідні з'єднання до Mixtape у приватних мережах. Windows попросить підтвердження.",
    firewallAllow: "Дозволити в брандмауері",
    firewallAdded: "Правило брандмауера додано",
    pairedWith: (name: string) => `Зв'язано: ${name}`,
    lastExchange: (when: string) => `Останній обмін: ${when}`,
    neverExchanged: "Обміну ще не було: відкрийте Mixtape на телефоні",
    notListening: "Синхронізація недоступна: порт 47821 зайнятий",
    unpair: "Відв'язати",
    unpairConfirm: "Відв'язати телефон? Вже отримані прослуховування залишаться.",
    unpaired: "Телефон відв'язано",
    paired: (name: string) => `Телефон зв'язано: ${name}`,
    failed: "Не вдалося зв'язати телефон",
  },
};
