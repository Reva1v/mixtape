// Sidebar icon sets, picked in Settings → Appearance. Outline (Lucide) is the default;
// Solid is the original Solar set, Rounded is Phosphor's filled one.
import type { Component } from "svelte";
import type { IconStyle } from "$lib/ipc/bindings/IconStyle";
import LucideHome from "~icons/lucide/house";
import LucideSearch from "~icons/lucide/search";
import LucideLibrary from "~icons/lucide/library-big";
import LucideDownloads from "~icons/lucide/arrow-down-to-line";
import LucideStats from "~icons/lucide/chart-column";
import LucideSettings from "~icons/lucide/settings";
import SolarHome from "~icons/solar/home-2-bold";
import SolarSearch from "~icons/solar/magnifier-bold";
import SolarLibrary from "~icons/solar/book-bookmark-bold";
import SolarDownloads from "~icons/solar/download-minimalistic-bold";
import SolarStats from "~icons/solar/chart-2-bold";
import SolarSettings from "~icons/solar/settings-bold";
import PhHome from "~icons/ph/house-fill";
import PhSearch from "~icons/ph/magnifying-glass-fill";
import PhLibrary from "~icons/ph/vinyl-record-fill";
import PhDownloads from "~icons/ph/arrow-circle-down-fill";
import PhStats from "~icons/ph/chart-bar-fill";
import PhSettings from "~icons/ph/gear-six-fill";

export type NavIcon = "home" | "search" | "library" | "downloads" | "stats" | "settings";

export const navIcons: Record<IconStyle, Record<NavIcon, Component>> = {
  outline: {
    home: LucideHome,
    search: LucideSearch,
    library: LucideLibrary,
    downloads: LucideDownloads,
    stats: LucideStats,
    settings: LucideSettings,
  },
  solid: {
    home: SolarHome,
    search: SolarSearch,
    library: SolarLibrary,
    downloads: SolarDownloads,
    stats: SolarStats,
    settings: SolarSettings,
  },
  rounded: {
    home: PhHome,
    search: PhSearch,
    library: PhLibrary,
    downloads: PhDownloads,
    stats: PhStats,
    settings: PhSettings,
  },
};
