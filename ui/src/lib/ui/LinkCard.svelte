<script lang="ts">
  // The card a pasted YouTube Music link turns into at the top of the search. A track gets
  // its title and artists once `link_track` answers, and the buttons Играть / Далее /
  // В очередь (state and ordering: linkCard.svelte.ts); an album, playlist or artist link
  // is a card that opens the page. Look: the catalog's top result.
  import type { Component } from "svelte";
  import { router } from "$lib/stores/router.svelte";
  import { menu } from "$lib/stores/overlay.svelte";
  import { t } from "$lib/i18n";
  import Cover from "./Cover.svelte";
  import ArtistLinks from "./ArtistLinks.svelte";
  import { trackMenu } from "./trackMenu";
  import type { LinkAction, LinkCardState } from "./linkCardState.svelte";
  import IconLink from "~icons/solar/link-linear";
  import IconAlbum from "~icons/solar/vinyl-linear";
  import IconPlaylist from "~icons/solar/playlist-linear";
  import IconArtist from "~icons/solar/microphone-3-linear";
  import IconPlay from "~icons/solar/play-bold";
  import IconNext from "~icons/solar/list-arrow-up-linear";
  import IconQueue from "~icons/solar/list-arrow-down-linear";
  import IconOpen from "~icons/solar/alt-arrow-right-linear";
  import IconLoader from "~icons/lucide/loader-circle";

  /** `raw`: what was typed, shown until the track's own data arrives. */
  let { card, raw }: { card: LinkCardState; raw: string } = $props();

  const link = $derived(card.link);
  const phase = $derived(card.phase);
  const label = $derived(link ? t.link[link.kind] : "");
  const icons: Record<string, Component> = {
    track: IconLink,
    album: IconAlbum,
    playlist: IconPlaylist,
    artist: IconArtist,
  };

  const actions: { id: LinkAction; label: () => string; icon: Component }[] = [
    { id: "now", label: () => t.player.play, icon: IconPlay },
    { id: "next", label: () => t.track.playNext, icon: IconNext },
    { id: "end", label: () => t.track.addToQueue, icon: IconQueue },
  ];

  const tile = "grid shrink-0 place-items-center bg-fill-2 text-ink-2 ring-1 ring-line";
</script>

{#if link}
  <section class="px-7 pb-6" aria-label={label}>
    {#if link.kind === "track"}
      <div
        class="flex w-full max-w-[520px] flex-col gap-4 rounded-xl bg-fill-1 p-4 ring-1 ring-line"
        role="group"
        aria-busy={phase.status === "loading"}
        oncontextmenu={(e) => {
          if (phase.status !== "ready") return;
          e.preventDefault();
          menu.show(trackMenu(phase.track), e.clientX, e.clientY);
        }}
      >
        <div class="flex items-center gap-5">
          {#if phase.status === "ready"}
            <Cover coverKey={phase.track.coverKey} size={96} rounded="md" />
          {:else}
            <span
              class="{tile} size-24 rounded-md {phase.status === 'loading' ? 'animate-pulse' : ''}"
            >
              <IconLink class="size-8" />
            </span>
          {/if}
          <div class="min-w-0 flex-1">
            {#if phase.status === "ready"}
              <div class="truncate text-[22px] font-bold">{phase.track.title}</div>
              <div class="mt-1 truncate text-[13.5px] text-ink-3">
                {label} · <ArtistLinks artists={phase.track.artists} />
              </div>
            {:else}
              <div class="truncate text-[22px] font-bold">{label}</div>
              <div class="mt-1 truncate text-[13.5px] text-ink-3" title={raw}>{raw}</div>
              {#if phase.status === "loading"}
                <div class="mt-2 flex items-center gap-1.5 text-[12.5px] text-ink-3">
                  <IconLoader class="size-3.5 animate-spin" />{t.catalog.loading}
                </div>
              {:else if phase.status === "failed"}
                <div class="mt-2 text-[12.5px] text-ink-2" role="status">{t.link.loadFailed}</div>
              {/if}
            {/if}
          </div>
        </div>
        <div class="flex flex-wrap gap-2">
          {#each actions as a (a.id)}
            {@const Icon = a.icon}
            {@const main = a.id === "now"}
            <button
              type="button"
              aria-disabled={!!card.busy}
              class="flex h-10 items-center gap-2 rounded-full px-4 text-[13.5px] font-semibold transition-colors outline-none focus-visible:ring-2 focus-visible:ring-white/60
                {main
                ? 'bg-ink text-black hover:bg-[#dcdcdc]'
                : 'bg-fill-2 text-ink ring-1 ring-line hover:bg-fill-3'}
                {card.busy && card.busy !== a.id ? 'opacity-50' : ''}"
              onclick={() => void card.act(a.id)}
            >
              {#if card.busy === a.id}<IconLoader class="size-4 animate-spin" />{:else}<Icon
                  class="size-4"
                />{/if}
              {a.label()}
            </button>
          {/each}
        </div>
      </div>
    {:else}
      {@const Icon = icons[link.kind]}
      <button
        type="button"
        class="group flex w-full max-w-[520px] cursor-pointer items-center gap-5 rounded-xl bg-fill-1 p-4 text-left ring-1 ring-line transition-colors outline-none hover:bg-fill-2 focus-visible:ring-2 focus-visible:ring-white/60"
        onclick={() =>
          router.go({ name: link.kind as "album" | "playlist" | "artist", id: link.id })}
      >
        <span class="{tile} size-24 {link.kind === 'artist' ? 'rounded-full' : 'rounded-md'}">
          <Icon class="size-8" />
        </span>
        <span class="min-w-0 flex-1">
          <span class="block truncate text-[22px] font-bold">{label}</span>
          <span class="mt-1 block truncate text-[13.5px] text-ink-3" title={raw}>{raw}</span>
        </span>
        <IconOpen class="size-5 shrink-0 text-ink-3 transition-colors group-hover:text-ink" />
      </button>
    {/if}
  </section>
{/if}
