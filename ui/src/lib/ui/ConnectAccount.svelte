<script lang="ts">
  // Connect a YouTube Music account by pasting browser cookies (T065). The core accepts
  // Netscape cookies.txt and EditThisCookie JSON; it validates them and stores them locally.
  import { downloads } from "$lib/stores/downloads.svelte";
  import { toasts } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n/ru";
  import Modal from "./Modal.svelte";
  import IconFile from "~icons/solar/file-text-linear";

  let text = $state("");
  let error = $state("");
  let busy = $state(false);
  let fileInput = $state<HTMLInputElement>();

  // Start clean every time the dialog opens.
  $effect(() => {
    if (!downloads.connectOpen) return;
    text = "";
    error = "";
    busy = false;
  });

  async function pickFile(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    if (file.size > 1024 * 1024) {
      error = "Файл слишком большой — нужен экспорт cookies, а не что-то другое";
      return;
    }
    text = await file.text();
    error = "";
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!text.trim() || busy) return;
    busy = true;
    error = "";
    try {
      await downloads.connect(text);
      downloads.connectOpen = false;
      toasts.push(downloads.account.name ? `Подключено: ${downloads.account.name}` : "YouTube Music подключён");
    } catch (err) {
      error = err instanceof Error && err.message ? err.message : "Не удалось подключить аккаунт";
    } finally {
      busy = false;
    }
  }
</script>

<Modal open={downloads.connectOpen} title="Подключение YouTube Music" onclose={() => (downloads.connectOpen = false)} width={520}>
  <ol class="mb-4 list-decimal space-y-1.5 pl-5 text-[13px] leading-snug text-ink-2 marker:text-ink-3">
    <li>Откройте окно инкогнито и войдите в <span class="text-ink">music.youtube.com</span>.</li>
    <li>
      Экспортируйте cookies этого сайта расширением вроде <span class="text-ink">«Get cookies.txt LOCALLY»</span> или
      <span class="text-ink">EditThisCookie</span>.
    </li>
    <li>Закройте окно инкогнито, не выходя из аккаунта, и вставьте текст ниже или выберите файл.</li>
  </ol>

  <form onsubmit={submit}>
    <textarea
      bind:value={text}
      spellcheck="false"
      placeholder="# Netscape HTTP Cookie File&#10;.youtube.com&#9;TRUE&#9;/&#9;TRUE&#9;…"
      aria-label="Cookies"
      class="h-36 w-full resize-none rounded-md bg-fill-2 px-3 py-2.5 font-mono text-[12px] leading-relaxed text-ink outline-none ring-1 ring-line
        placeholder:text-ink-3 focus:ring-white/40"
    ></textarea>
    <input bind:this={fileInput} type="file" accept=".txt,.json,text/plain,application/json" class="hidden" onchange={pickFile} />

    {#if error}
      <p class="mt-2 text-[13px] leading-snug text-danger">{error}</p>
    {/if}
    <p class="mt-2 text-[12px] leading-snug text-ink-3">Cookies хранятся только на этом компьютере в зашифрованном виде.</p>

    <div class="mt-5 flex items-center gap-2">
      <button
        type="button"
        class="flex h-9 items-center gap-2 rounded-full bg-fill-2 px-4 text-[13px] font-medium text-ink transition-colors hover:bg-fill-3"
        onclick={() => fileInput?.click()}
      >
        <IconFile class="size-4" />Выбрать файл…
      </button>
      <div class="flex-1"></div>
      <button
        type="button"
        class="h-9 rounded-full px-4 text-[13px] text-ink-2 hover:bg-fill-2"
        onclick={() => (downloads.connectOpen = false)}
      >
        {t.common.cancel}
      </button>
      <button
        type="submit"
        disabled={!text.trim() || busy}
        class="h-9 rounded-full bg-ink px-5 text-[13px] font-semibold text-black transition-[background-color,opacity] hover:bg-[#dcdcdc] disabled:opacity-40 disabled:hover:bg-ink"
      >
        {busy ? "Проверяем…" : "Подключить"}
      </button>
    </div>
  </form>
</Modal>
